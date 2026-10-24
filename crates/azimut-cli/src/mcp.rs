//! `azimut mcp`: a Model Context Protocol server on stdio, so that an agent (Claude, Codex...)
//! can run Azimut for a user.
//!
//! Messages are newline-delimited JSON-RPC 2.0 on stdin/stdout; logs go to stderr. The server
//! answers `initialize`, `ping`, `tools/list`, `tools/call`, `prompts/list` and `prompts/get`.
//! See the MCP specification (SOURCES.md, MCP server).
//!
//! The server never decides what only the user can decide. A tool that lacks such an input
//! (base coordinates and their frame, which base to use, in place or copies, consent to
//! modify images) does nothing and returns `status: "needs_decision"` with the questions to
//! ask. Writing into images needs a dry run first and then `confirm: true`.
//!
//! `submit_ppp`, `process_ppk`, `shift_rtk`, `write_images` and `restore_images` run this same `azimut`
//! program as a child process, so their console output never reaches the protocol stream;
//! the server reads back the JSON reports they write.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use azimut_export::images;
use azimut_export::report::FlightReport;
use azimut_io::csrs_ppp;
use azimut_io::scan::{scan, ScanOptions, Severity};
use azimut_solve::FlightSolution;
use serde_json::{json, Value};

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "\
Azimut computes camera positions of DJI drone flights (PPK against a base station, or a \
correction of RTK flights) and writes them for photogrammetry software. Work step by step \
and ask the user at each decision; never guess a value that only the user knows.
1. Call scan_folder first and summarise its findings (incomplete captures, RTK status, base \
coverage). Ask how to handle errors and warnings before going on.
2. Base coordinates: ask where they come from (a CSRS-PPP .sum file, a submission of the base \
RINEX to CSRS-PPP with submit_ppp, known coordinates with their reference frame and epoch, or \
the approximate RINEX header position for relative work only). Never invent coordinates, a \
frame, an antenna height, an antenna type or an e-mail address.
3. Heights: ask whether orthometric heights (CGVD2013) are wanted; the geoid grid must match \
the frame (ca_nrc_CGG2013an83.tif for NAD83(CSRS), ca_nrc_CGG2013ai08.tif for ITRF/IGS).
4. After process_ppk or shift_rtk, report the summary and every 'attention' point as given.
5. Images are modified only after: asking in place or copies, a dry run of write_images shown \
to the user, and the user's explicit yes (confirm: true).
When a tool returns status 'needs_decision' or 'needs_confirmation', ask the user its \
questions and call it again with the answers.";

const WORKFLOW_PROMPT: &str = "\
Help me compute the camera positions of my DJI flights with Azimut. Go step by step and ask me \
before each decision:
1. Scan the folder I give you (scan_folder) and summarise what you find: flights, captures, \
RTK status, base files and how well they cover each flight, warnings and errors.
2. Ask me how the base position is known (CSRS-PPP summary, a submission of the base RINEX to \
CSRS-PPP, known coordinates with frame and epoch, or approximate). Submit with submit_ppp after \
my yes, or read a summary with read_ppp_summary, and tell me which point it refers to.
3. Ask whether I want CGVD2013 heights, then run process_ppk (or shift_rtk for an RTK flight).
4. Summarise the report: fix rate, uncertainties, problems and every point to check.
5. Ask whether to write the positions into the images, in place or into copies; show me the \
dry run and wait for my yes before writing.
6. Tell me which files to import into PIX4Dmatic and how (the _pix4dmatic.txt file).";

/// Run the server until stdin closes.
pub fn serve() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(
                    &mut out,
                    &error_response(Value::Null, -32700, &format!("parse error: {e}")),
                )?;
                continue;
            }
        };
        if let Some(reply) = handle(&msg, &mut out) {
            send(&mut out, &reply)?;
        }
    }
    Ok(())
}

fn send(out: &mut impl Write, v: &Value) -> std::io::Result<()> {
    writeln!(out, "{v}")?;
    out.flush()
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Answer one message; `None` for notifications.
fn handle(msg: &Value, out: &mut impl Write) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let id = id?; // notifications (initialized, cancelled...) get no answer
    let result = match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            let version = PROTOCOL_VERSIONS
                .iter()
                .find(|v| **v == asked)
                .copied()
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false }, "prompts": { "listChanged": false } },
                "serverInfo": { "name": "azimut", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tool_list() }),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let progress = params.pointer("/_meta/progressToken").cloned();
            match call_tool(name, &args, progress, out) {
                Ok(v) => tool_result(v, false),
                Err(e) => tool_result(json!({ "status": "error", "error": format!("{e:#}") }), true),
            }
        }
        "prompts/list" => json!({ "prompts": [{
            "name": "ppk_workflow",
            "description": "Step-by-step PPK of DJI flights with Azimut, asking the user at each decision.",
        }]}),
        "prompts/get" => {
            if params.get("name").and_then(Value::as_str) != Some("ppk_workflow") {
                return Some(error_response(id, -32602, "unknown prompt"));
            }
            json!({
                "description": "Step-by-step PPK with Azimut",
                "messages": [{ "role": "user", "content": { "type": "text", "text": WORKFLOW_PROMPT } }],
            })
        }
        _ => return Some(error_response(id, -32601, &format!("unknown method {method}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn tool_result(v: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&v).unwrap_or_default();
    json!({ "content": [{ "type": "text", "text": text }], "structuredContent": v, "isError": is_error })
}

fn needs(status: &str, reason: &str, questions: Vec<Value>) -> Value {
    json!({ "status": status, "reason": reason, "questions": questions })
}

fn question(id: &str, text: &str, options: &[&str]) -> Value {
    if options.is_empty() {
        json!({ "id": id, "question": text })
    } else {
        json!({ "id": id, "question": text, "options": options })
    }
}

fn tool_list() -> Value {
    let triple = json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 });
    let paths = json!({ "type": "array", "items": { "type": "string" } });
    json!([
        {
            "name": "scan_folder",
            "description": "Inventory and diagnose a folder of DJI flights (headers only, fast): flights, captures, MRK and RTK status, raw GNSS logs, base RINEX files and their coverage, findings. Call this first.",
            "inputSchema": { "type": "object", "properties": {
                "dir": { "type": "string", "description": "Folder: SD card copy, mission folder or campaign." },
                "base_files": { "description": "Base RINEX files or folders outside DIR.", "type": "array", "items": { "type": "string" } }
            }, "required": ["dir"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "read_ppp_summary",
            "description": "Read an NRCan CSRS-PPP summary (.sum) of a base station: position per reference system and epoch, 95 % uncertainty, antenna handling and which point the coordinates refer to.",
            "inputSchema": { "type": "object", "properties": {
                "file": { "type": "string" }
            }, "required": ["file"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "submit_ppp",
            "description": "Submit a base-station RINEX to NRCan CSRS-PPP, wait for the result and return its summary (position per reference system, 95 % uncertainty, which point it refers to) and the .sum file for process_ppk. Sends the file to NRCan: without confirm: true it only asks for confirmation. Without an e-mail (argument, AZIMUT_CSRS_EMAIL or remembered) it asks for it; an e-mail given here is remembered. If the wait ends first, returns status 'pending' with the tracking key: call again with resume.",
            "inputSchema": { "type": "object", "properties": {
                "rinex": { "type": "string", "description": "Base RINEX observation file, as recorded." },
                "out": { "type": "string", "description": "Folder for the CSRS-PPP results." },
                "email": { "type": "string", "description": "E-mail of the user's CSRS-PPP account; ask the user, never guess." },
                "sysref": { "type": "string", "enum": ["itrf", "nad83"], "description": "ITRF at the observation epoch (default), or NAD83(CSRS) with CGVD2013 heights." },
                "mode": { "type": "string", "enum": ["static", "kinematic"] },
                "wait_minutes": { "type": "number", "description": "Longest wait for the result (default 15). Lower it if the client stops long tool calls." },
                "resume": { "type": "string", "description": "Tracking key of an earlier submission: fetch its results instead of submitting." },
                "confirm": { "type": "boolean" }
            }, "required": ["out"] },
            "annotations": { "openWorldHint": true }
        },
        {
            "name": "process_ppk",
            "description": "PPK of every flight in DIR against a base station (RTKLIB-EX). Writes positions CSV, OpenDroneMap geo.txt, PIX4Dmatic geolocation files and import notes, solution JSON and a quality report (JSON and HTML) per flight into OUT, and returns the report summaries. Does not modify images. Returns needs_decision when the base coordinates or their frame are not given.",
            "inputSchema": { "type": "object", "properties": {
                "dir": { "type": "string" },
                "out": { "type": "string", "description": "Output folder." },
                "base_files": paths,
                "base_position": { "type": "object", "description": "Where the base coordinates come from: {\"ppp_summary\": \"file.sum\", \"ppp_system\": optional}, or {\"lat\", \"lon\", \"h\" (ellipsoidal), \"frame\" (frame and epoch)}, or {\"rinex_header\": true} for relative work only (approximate)." },
                "antenna_height": { "type": "number", "description": "Base antenna height above the mark, m (with antenna_type), when not taken from the RINEX header." },
                "antenna_type": { "type": "string" },
                "antex": { "type": "string", "description": "ANTEX file (e.g. igs20.atx)." },
                "geoid": { "type": "string", "description": "Geoid grid (PROJ GeoTIFF) for orthometric heights, e.g. ca_nrc_CGG2013an83.tif." },
                "rtklib_dir": { "type": "string" },
                "roles": paths,
                "report_lang": { "type": "string", "enum": ["en", "fr"], "description": "Language of the HTML report (use the user's)." }
            }, "required": ["dir", "out"] }
        },
        {
            "name": "shift_rtk",
            "description": "Correct the real-time (RTK) positions of the MRK by the difference between the base position the aircraft used and the correct one (e.g. from CSRS-PPP). Writes the same outputs and report as process_ppk. Returns needs_decision when the shift or the frame is not given.",
            "inputSchema": { "type": "object", "properties": {
                "dir": { "type": "string" },
                "out": { "type": "string" },
                "from": triple,
                "to": triple,
                "to_ppp": { "type": "string", "description": "CSRS-PPP summary giving the corrected base position instead of 'to'." },
                "ppp_system": { "type": "string" },
                "enu": triple,
                "frame": { "type": "string" },
                "geoid": { "type": "string" },
                "roles": paths,
                "report_lang": { "type": "string", "enum": ["en", "fr"] }
            }, "required": ["dir", "out"] }
        },
        {
            "name": "write_images",
            "description": "Write the positions of a processed flight (its _solution.json) into the images: EXIF GPS and DJI XMP position and accuracy, heights ellipsoidal. Mode in_place (original bytes saved for restore_images) or copy (originals untouched). Without confirm: true it only does a dry run and asks for confirmation.",
            "inputSchema": { "type": "object", "properties": {
                "solution": { "type": "string" },
                "mode": { "type": "string", "enum": ["in_place", "copy"] },
                "images_dir": { "type": "string" },
                "copy_unpositioned": { "type": "boolean" },
                "confirm": { "type": "boolean" }
            }, "required": ["solution"] },
            "annotations": { "destructiveHint": true }
        },
        {
            "name": "restore_images",
            "description": "Undo an in-place write_images from its _image_edits.json record.",
            "inputSchema": { "type": "object", "properties": {
                "record": { "type": "string" }
            }, "required": ["record"] }
        },
        {
            "name": "read_report",
            "description": "Read a flight's quality report (_report.json), including the per-exposure table.",
            "inputSchema": { "type": "object", "properties": {
                "file": { "type": "string" }
            }, "required": ["file"] },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

fn s<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|v| !v.is_empty())
}

fn strings(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn triple(args: &Value, key: &str) -> anyhow::Result<Option<String>> {
    let Some(v) = args.get(key) else {
        return Ok(None);
    };
    let a: Vec<f64> = v
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    anyhow::ensure!(a.len() == 3, "{key} needs three numbers");
    Ok(Some(format!("{},{},{}", a[0], a[1], a[2])))
}

fn call_tool(
    name: &str,
    args: &Value,
    progress: Option<Value>,
    out: &mut impl Write,
) -> anyhow::Result<Value> {
    match name {
        "scan_folder" => scan_folder(args),
        "read_ppp_summary" => read_ppp_summary(args),
        "submit_ppp" => submit_ppp(args, progress, out),
        "process_ppk" => process_ppk(args, progress, out),
        "shift_rtk" => shift_rtk(args, progress, out),
        "write_images" => write_images(args, progress, out),
        "restore_images" => {
            let record = s(args, "record").ok_or_else(|| anyhow::anyhow!("record is required"))?;
            let run = run_azimut(&["restore-images".into(), record.into()], progress, out)?;
            Ok(json!({ "status": run.status(), "log": run.log }))
        }
        "read_report" => {
            let file = s(args, "file").ok_or_else(|| anyhow::anyhow!("file is required"))?;
            let text = std::fs::read_to_string(file)?;
            Ok(serde_json::from_str(&text)?)
        }
        _ => anyhow::bail!("unknown tool {name}"),
    }
}

fn scan_folder(args: &Value) -> anyhow::Result<Value> {
    let dir = PathBuf::from(s(args, "dir").ok_or_else(|| anyhow::anyhow!("dir is required"))?);
    anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
    let report = scan(
        &dir,
        &ScanOptions {
            base: strings(args, "base_files")
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            ..Default::default()
        },
    );
    let flights: Vec<Value> = report
        .flights
        .iter()
        .map(|f| {
            let base = f.base.as_ref().map(|b| {
                let r = &report.rinex[b.rinex];
                json!({
                    "file": r.path, "marker": r.marker, "interval_s": r.interval_s,
                    "distance_km": b.distance_km, "exposures_outside": b.exposures_outside,
                    "margin_before_s": b.margin_before_s, "margin_after_s": b.margin_after_s,
                })
            });
            json!({
                "dir": f.dir,
                "images": f.photos.len(),
                "captures": f.captures.len(),
                "multispectral_captures": f.captures.iter().filter(|c| c.is_multispectral()).count(),
                "mrk_records": f.mrk.len(),
                "paired_exposures": f.matching.pairs.len(),
                "rtk": { "fixed": f.quality.fixed, "float": f.quality.float, "single": f.quality.single, "none": f.quality.no_position },
                "raw_gnss_logs": f.aux_files(azimut_io::scan::AuxKind::RawGnss),
                "base": base,
                "findings": f.findings.iter().map(|x| json!({
                    "severity": match x.severity { Severity::Error => "error", Severity::Warning => "warning", Severity::Info => "info" },
                    "message": x.message,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let rinex: Vec<Value> = report
        .rinex
        .iter()
        .map(|r| json!({ "file": r.path, "marker": r.marker, "interval_s": r.interval_s, "antenna_type": r.antenna_type }))
        .collect();
    let mut questions = Vec::new();
    for f in &report.flights {
        let rtk = f.quality.fixed + f.quality.float;
        if f.base.is_none() && rtk == 0 {
            questions.push(question(
                "base_files",
                &format!(
                    "No base RINEX covers {}, which has no RTK positions: where are the base station files?",
                    f.dir.display()
                ),
                &[],
            ));
        }
    }
    if report
        .flights
        .iter()
        .any(|f| f.findings.iter().any(|x| x.severity == Severity::Error))
    {
        questions.push(question(
            "errors",
            "The scan found errors (see findings). Fix them, or go on with the flights that can be processed?",
            &[],
        ));
    }
    Ok(json!({
        "status": if questions.is_empty() { "ok" } else { "needs_decision" },
        "root": report.root,
        "images": report.images,
        "flights": flights,
        "rinex": rinex,
        "questions": questions,
        "next": "process_ppk for flights without RTK (base needed); shift_rtk for RTK flights whose base position must be corrected.",
    }))
}

fn read_ppp_summary(args: &Value) -> anyhow::Result<Value> {
    let file = s(args, "file").ok_or_else(|| anyhow::anyhow!("file is required"))?;
    ppp_summary_value(Path::new(file))
}

fn ppp_summary_value(file: &Path) -> anyhow::Result<Value> {
    let sum = csrs_ppp::read(file)?;
    let point = match sum.point() {
        csrs_ppp::PppPoint::PhaseCentre => {
            "the antenna phase centre (no antenna type or height in the RINEX): not the ground mark"
        }
        csrs_ppp::PppPoint::ReferencePoint => "the antenna reference point (antenna calibrated, no height)",
        csrs_ppp::PppPoint::Mark => "the ground mark (antenna height and offsets applied)",
    };
    Ok(json!({
        "status": "ok",
        "summary": sum,
        "coordinates_refer_to": point,
        "note": "Use one of these positions with process_ppk (base_position.ppp_summary) or shift_rtk (to_ppp). With several systems, ask the user which one to use (ppp_system).",
    }))
}

fn submit_ppp(args: &Value, progress: Option<Value>, out: &mut impl Write) -> anyhow::Result<Value> {
    let out_dir = s(args, "out").ok_or_else(|| anyhow::anyhow!("out is required"))?;
    let mut v: Vec<String> = vec!["ppp".into()];
    if let Some(key) = s(args, "resume") {
        v.extend(["--resume".into(), key.into()]);
    } else {
        let rinex = s(args, "rinex").ok_or_else(|| anyhow::anyhow!("rinex (or resume) is required"))?;
        anyhow::ensure!(Path::new(rinex).is_file(), "{rinex} is not a file");
        let email = match s(args, "email") {
            Some(e) => {
                let e = e.trim();
                anyhow::ensure!(crate::settings::valid_email(e), "{e:?} is not an e-mail address");
                // The user gave it to the agent: remember it like a terminal answer.
                if crate::settings::csrs_email().as_deref() != Some(e) {
                    crate::settings::save_csrs_email(e)?;
                }
                e.to_string()
            }
            None => match std::env::var("AZIMUT_CSRS_EMAIL")
                .ok()
                .filter(|e| crate::settings::valid_email(e.trim()))
                .or_else(crate::settings::csrs_email)
            {
                Some(e) => e,
                None => {
                    return Ok(needs(
                        "needs_decision",
                        "no CSRS-PPP e-mail is known",
                        vec![question(
                            "email",
                            "What is the e-mail of your CSRS-PPP account (NRCan)? Azimut remembers it for the next submissions.",
                            &[],
                        )],
                    ))
                }
            },
        };
        if args.get("confirm").and_then(Value::as_bool) != Some(true) {
            return Ok(json!({
                "status": "needs_confirmation",
                "reason": "the base RINEX is about to be sent to NRCan CSRS-PPP",
                "questions": [question(
                    "confirm",
                    &format!("Send {rinex} to NRCan CSRS-PPP with the account {email}?"),
                    &["yes", "no"],
                )],
            }));
        }
        v.extend([rinex.into(), "--email".into(), email]);
        if let Some(sys) = s(args, "sysref") {
            v.extend(["--sysref".into(), sys.into()]);
        }
        if let Some(mode) = s(args, "mode") {
            v.extend(["--ppp-mode".into(), mode.into()]);
        }
    }
    if let Some(w) = args.get("wait_minutes").and_then(Value::as_f64) {
        v.extend(["--ppp-timeout".into(), w.to_string()]);
    }
    v.extend(["--out".into(), out_dir.into()]);

    let run = run_azimut(&v, progress, out)?;
    let key = run
        .log
        .lines()
        .find_map(|l| l.trim().strip_prefix("tracking key: "))
        .or_else(|| s(args, "resume"))
        .map(str::to_string);
    if run.code != Some(0) {
        // Only a wait that ran out is worth resuming; other errors are reported as failures.
        let pending = run.log.contains("CSRS-PPP still processing");
        return Ok(json!({
            "status": if pending { "pending" } else { "failed" },
            "tracking_key": key,
            "log": run.log,
            "next": if pending { "call submit_ppp again with resume: tracking_key, later" } else { "report the log to the user" },
        }));
    }
    // The summary this run wrote, as `azimut ppp` names it.
    let summary = run
        .log
        .lines()
        .find_map(|l| l.trim().strip_prefix("summary: "))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("azimut ppp did not name its summary file"))?;
    let mut v = ppp_summary_value(&summary)?;
    v["summary_file"] = json!(summary);
    v["tracking_key"] = json!(key);
    v["log"] = json!(run.log);
    v["note"] = json!("Use summary_file with process_ppk (base_position.ppp_summary). With several systems, ask the user which one to use (ppp_system).");
    Ok(v)
}

fn base_args(args: &Value) -> anyhow::Result<Result<Vec<String>, Value>> {
    let Some(bp) = args.get("base_position").filter(|v| v.is_object()) else {
        return Ok(Err(needs(
            "needs_decision",
            "the base coordinates are not given",
            vec![question(
                "base_position",
                "Where do the base coordinates come from: a CSRS-PPP summary (.sum file), a submission of the base RINEX to CSRS-PPP (submit_ppp), known coordinates (latitude, longitude, ellipsoidal height, with their frame and epoch), or the approximate RINEX header position (relative work only)?",
                &["ppp_summary", "submit_ppp", "known coordinates", "rinex_header"],
            )],
        )));
    };
    let mut v = Vec::new();
    if let Some(sum) = s(bp, "ppp_summary") {
        v.extend(["--base-ppp".into(), sum.into()]);
        if let Some(sys) = s(bp, "ppp_system") {
            v.extend(["--ppp-system".into(), sys.into()]);
        }
        return Ok(Ok(v));
    }
    let llh = (
        bp.get("lat").and_then(Value::as_f64),
        bp.get("lon").and_then(Value::as_f64),
        bp.get("h").and_then(Value::as_f64),
    );
    if let (Some(lat), Some(lon), Some(h)) = llh {
        let Some(frame) = s(bp, "frame") else {
            return Ok(Err(needs(
                "needs_decision",
                "the frame of the base coordinates is not given",
                vec![question(
                    "frame",
                    "In which reference frame and epoch are these base coordinates (e.g. \"NAD83(CSRS) v8, epoch 2010.0\" or \"ITRF2020, epoch 2024.6\")? Is the height ellipsoidal?",
                    &[],
                )],
            )));
        };
        v.extend([
            "--base-llh".into(),
            format!("{lat},{lon},{h}"),
            "--frame".into(),
            frame.into(),
        ]);
        return Ok(Ok(v));
    }
    if bp.get("rinex_header").and_then(Value::as_bool) == Some(true) {
        v.push("--approximate-base".into());
        return Ok(Ok(v));
    }
    anyhow::bail!("base_position must hold ppp_summary, or lat, lon, h and frame, or rinex_header: true")
}

fn common_args(args: &Value, v: &mut Vec<String>) {
    if let Some(g) = s(args, "geoid") {
        v.extend(["--geoid".into(), g.into()]);
    }
    if let Some(l) = s(args, "report_lang") {
        v.extend(["--report-lang".into(), l.into()]);
    }
    let roles = strings(args, "roles");
    if !roles.is_empty() {
        v.extend(["--roles".into(), roles.join(",")]);
    }
}

fn process_ppk(args: &Value, progress: Option<Value>, out: &mut impl Write) -> anyhow::Result<Value> {
    let dir = s(args, "dir").ok_or_else(|| anyhow::anyhow!("dir is required"))?;
    let out_dir = s(args, "out").ok_or_else(|| anyhow::anyhow!("out is required"))?;
    let mut v: Vec<String> = vec!["process".into(), dir.into(), "--out".into(), out_dir.into()];
    let base_files = strings(args, "base_files");

    // The base must cover every flight that has no RTK positions.
    let report = scan(
        Path::new(dir),
        &ScanOptions {
            base: base_files.iter().map(PathBuf::from).collect(),
            ..Default::default()
        },
    );
    let uncovered: Vec<String> = report
        .flights
        .iter()
        .filter(|f| f.base.is_none())
        .map(|f| f.dir.display().to_string())
        .collect();
    if !uncovered.is_empty() {
        return Ok(needs(
            "needs_decision",
            "no base RINEX covers some flights",
            vec![question(
                "base_files",
                &format!(
                    "No base RINEX covers {}. Where are the base station files?",
                    uncovered.join(", ")
                ),
                &[],
            )],
        ));
    }
    for b in &base_files {
        v.extend(["--base".into(), b.clone()]);
    }
    match base_args(args)? {
        Ok(b) => v.extend(b),
        Err(decision) => return Ok(decision),
    }
    let height = args.get("antenna_height").and_then(Value::as_f64);
    if let Some(h) = height {
        v.extend(["--antenna-height".into(), h.to_string()]);
    }
    if let Some(t) = s(args, "antenna_type") {
        anyhow::ensure!(
            height.is_some(),
            "antenna_type needs antenna_height (else both come from the RINEX header)"
        );
        v.extend(["--antenna-type".into(), t.into()]);
    }
    if let Some(a) = s(args, "antex") {
        v.extend(["--antex".into(), a.into()]);
    }
    if let Some(r) = s(args, "rtklib_dir") {
        v.extend(["--rtklib".into(), r.into()]);
    }
    common_args(args, &mut v);
    run_and_report(&v, Path::new(out_dir), progress, out)
}

fn shift_rtk(args: &Value, progress: Option<Value>, out: &mut impl Write) -> anyhow::Result<Value> {
    let dir = s(args, "dir").ok_or_else(|| anyhow::anyhow!("dir is required"))?;
    let out_dir = s(args, "out").ok_or_else(|| anyhow::anyhow!("out is required"))?;
    let mut v: Vec<String> = vec!["shift".into(), dir.into(), "--out".into(), out_dir.into()];
    let from = triple(args, "from")?;
    let to = triple(args, "to")?;
    let enu = triple(args, "enu")?;
    let to_ppp = s(args, "to_ppp");
    match (&from, &to, to_ppp, &enu) {
        (Some(f), Some(t), None, None) => v.extend(["--from".into(), f.clone(), "--to".into(), t.clone()]),
        (Some(f), None, Some(p), None) => {
            v.extend(["--from".into(), f.clone(), "--to-ppp".into(), p.into()]);
            if let Some(sys) = s(args, "ppp_system") {
                v.extend(["--ppp-system".into(), sys.into()]);
            }
        }
        (None, None, None, Some(e)) => v.extend(["--enu".into(), e.clone()]),
        _ => {
            return Ok(needs(
                "needs_decision",
                "the shift is not given",
                vec![question(
                    "shift",
                    "Which base coordinates did the aircraft use (as broadcast by the RTK base), and what are the correct coordinates of the same point (or a CSRS-PPP summary of the base)? Alternatively, the shift in East, North, Up metres.",
                    &[],
                )],
            ))
        }
    }
    match s(args, "frame") {
        Some(f) => v.extend(["--frame".into(), f.into()]),
        None if to_ppp.is_none() => {
            return Ok(needs(
                "needs_decision",
                "the frame of the corrected positions is not given",
                vec![question(
                    "frame",
                    "In which reference frame and epoch are the corrected base coordinates (e.g. \"NAD83(CSRS) v8, epoch 2010.0\")?",
                    &[],
                )],
            ))
        }
        None => {}
    }
    common_args(args, &mut v);
    run_and_report(&v, Path::new(out_dir), progress, out)
}

fn write_images(args: &Value, progress: Option<Value>, out: &mut impl Write) -> anyhow::Result<Value> {
    let solution = s(args, "solution").ok_or_else(|| anyhow::anyhow!("solution is required"))?;
    let sol: FlightSolution = serde_json::from_str(&std::fs::read_to_string(solution)?)?;
    let files: Vec<&azimut_solve::pipeline::FileRef> = sol.exposures.iter().flat_map(|e| &e.files).collect();
    let bytes: u64 = files
        .iter()
        .filter_map(|f| std::fs::metadata(&f.path).ok())
        .map(|m| m.len())
        .sum();
    let Some(mode) = s(args, "mode") else {
        return Ok(needs(
            "needs_decision",
            "the write mode is not given",
            vec![question(
                "mode",
                &format!(
                    "Write the positions into the original images (in_place, undoable with restore_images) or into copies (copy, about {:.1} GB more on disk, originals untouched)?",
                    bytes as f64 / 1e9
                ),
                &["in_place", "copy"],
            )],
        ));
    };
    let cli_mode = match mode {
        "in_place" => "in-place",
        "copy" => "copy",
        other => anyhow::bail!("mode must be in_place or copy, not {other}"),
    };
    if args.get("confirm").and_then(Value::as_bool) != Some(true) {
        let planned = images::plan_flight(&sol);
        let skipped: Vec<Value> = planned
            .skipped
            .iter()
            .take(20)
            .map(|(p, why)| json!({ "file": p, "reason": why }))
            .collect();
        let unpositioned = files.iter().filter(|f| f.camera.is_none()).count();
        let mut questions = vec![question(
            "confirm",
            &format!(
                "Write the positions into {} image(s) ({})? Only the EXIF GPS and DJI XMP position and accuracy fields change; heights are ellipsoidal in \"{}\".",
                planned.written.len(),
                if mode == "copy" { "copies" } else { "in place" },
                sol.frame
            ),
            &["yes", "no"],
        )];
        if mode == "copy" && unpositioned > 0 && args.get("copy_unpositioned").is_none() {
            questions.push(question(
                "copy_unpositioned",
                &format!("{unpositioned} image(s) have no position: copy them too (they keep the aircraft's own, less accurate position) or leave them out?"),
                &["copy them", "leave them out"],
            ));
        }
        return Ok(json!({
            "status": "needs_confirmation",
            "dry_run": { "would_write": planned.written.len(), "skipped": planned.skipped.len(), "skipped_examples": skipped, "image_bytes": bytes },
            "questions": questions,
        }));
    }
    let mut v: Vec<String> = vec![
        "write-images".into(),
        solution.into(),
        "--mode".into(),
        cli_mode.into(),
    ];
    if let Some(d) = s(args, "images_dir") {
        v.extend(["--images-dir".into(), d.into()]);
    }
    if args.get("copy_unpositioned").and_then(Value::as_bool) == Some(true) {
        v.push("--copy-unpositioned".into());
    }
    let run = run_azimut(&v, progress, out)?;
    Ok(json!({ "status": run.status(), "log": run.log }))
}

struct Run {
    code: Option<i32>,
    log: String,
}

impl Run {
    fn status(&self) -> &'static str {
        match self.code {
            Some(0) => "ok",
            Some(2) => "completed_with_problems",
            _ => "failed",
        }
    }
}

/// Run this program with `args`, sending progress notifications while it runs when the
/// client asked for them.
fn run_azimut(args: &[String], progress: Option<Value>, out: &mut impl Write) -> anyhow::Result<Run> {
    let exe = std::env::current_exe()?;
    let mut child = Command::new(exe)
        .args(args)
        .env("RUST_BACKTRACE", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let t_out = std::thread::spawn(move || {
        let mut b = String::new();
        let _ = so.read_to_string(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = String::new();
        let _ = se.read_to_string(&mut b);
        b
    });
    let start = Instant::now();
    let mut last = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if let Some(token) = &progress {
            if last.elapsed() >= Duration::from_secs(5) {
                last = Instant::now();
                let note = json!({ "jsonrpc": "2.0", "method": "notifications/progress", "params": {
                    "progressToken": token, "progress": start.elapsed().as_secs(),
                    "message": format!("azimut {} running ({} s)", args[0], start.elapsed().as_secs()),
                }});
                let _ = send(out, &note);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    let mut log: Vec<&str> = stdout.lines().chain(stderr.lines()).collect();
    if log.len() > 60 {
        log.drain(..log.len() - 60);
    }
    Ok(Run {
        code: status.code(),
        log: log.join("\n"),
    })
}

/// Run `process` or `shift`, then return the summaries of the reports it wrote.
fn run_and_report(
    args: &[String],
    out_dir: &Path,
    progress: Option<Value>,
    out: &mut impl Write,
) -> anyhow::Result<Value> {
    let started = SystemTime::now();
    let run = run_azimut(args, progress, out)?;
    let mut reports = Vec::new();
    if let Ok(rd) = std::fs::read_dir(out_dir) {
        let mut paths: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.to_string_lossy().ends_with("_report.json"))
            .filter(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .map(|t| t >= started)
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        for p in paths {
            let Ok(r) = std::fs::read_to_string(&p)
                .map_err(anyhow::Error::from)
                .and_then(|t| serde_json::from_str::<FlightReport>(&t).map_err(anyhow::Error::from))
            else {
                continue;
            };
            reports.push(json!({
                "flight": r.flight,
                "method": r.method,
                "frame": r.frame,
                "vertical_datum": r.vertical_datum,
                "summary": r.summary,
                "attention": r.attention,
                "report_json": p,
                "report_html": p.with_file_name(format!("{}_report.html", r.flight)),
                "solution": p.with_file_name(format!("{}_solution.json", r.flight)),
                "pix4dmatic_notes": p.with_file_name(format!("{}_pix4dmatic.txt", r.flight)),
            }));
        }
    }
    Ok(json!({ "status": run.status(), "reports": reports, "log": run.log }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_position_becomes_a_process_base_source() {
        let args = |bp: Value| base_args(&json!({ "base_position": bp })).unwrap().unwrap();
        assert_eq!(args(json!({ "rinex_header": true })), ["--approximate-base"]);
        assert_eq!(args(json!({ "ppp_summary": "b.sum" })), ["--base-ppp", "b.sum"]);
        assert_eq!(
            args(json!({ "lat": 46.0, "lon": -72.0, "h": 30.0, "frame": "ITRF2020" }))[0],
            "--base-llh"
        );
    }
}
