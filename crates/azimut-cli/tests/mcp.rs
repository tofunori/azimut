//! `azimut mcp` driven like an agent would: every stdout line is a JSON-RPC message, tools
//! ask for the user's decisions instead of guessing, and images are written only after a
//! dry run and a confirmation.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

struct Server {
    child: Child,
    // Settings folder of this server, so the user's own settings are never read.
    _settings: tempfile::TempDir,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Server {
    fn start() -> Self {
        let settings = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_azimut"))
            .arg("mcp")
            .env("AZIMUT_CONFIG_DIR", settings.path())
            .env_remove("AZIMUT_CSRS_EMAIL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server {
            child,
            _settings: settings,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn notify(&mut self, method: &str) {
        writeln!(self.stdin, "{}", json!({ "jsonrpc": "2.0", "method": method })).unwrap();
    }

    /// Send a request and return its result (or error), skipping progress notifications.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "server closed");
            let v: Value = serde_json::from_str(&line).expect("every stdout line is JSON-RPC");
            assert_eq!(v["jsonrpc"], "2.0");
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
            assert_eq!(v["method"], "notifications/progress");
        }
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let r = self.request("tools/call", json!({ "name": tool, "arguments": args }));
        let result = &r["result"];
        let structured = result["structuredContent"].clone();
        let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, structured, "text content mirrors the structured content");
        structured
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn azimut(args: &[&str]) {
    let st = Command::new(env!("CARGO_BIN_EXE_azimut"))
        .args(args)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "azimut {args:?}");
}

fn p(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn protocol_basics() {
    let mut s = Server::start();
    let init = s.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "azimut");
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("never guess"));
    s.notify("notifications/initialized");

    let tools = s.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "scan_folder",
        "read_ppp_summary",
        "process_ppk",
        "shift_rtk",
        "write_images",
        "restore_images",
        "read_report",
    ] {
        assert!(names.contains(&n), "{n}");
    }
    let prompt = s.request("prompts/get", json!({ "name": "ppk_workflow" }));
    assert!(prompt["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap()
        .contains("ask me"));
    assert_eq!(s.request("ping", json!({}))["result"], json!({}));
    assert_eq!(s.request("nope", json!({}))["error"]["code"], -32601);
    // An unknown older version is answered with one the server supports.
    let old = s.request("initialize", json!({ "protocolVersion": "1999-01-01" }));
    assert_eq!(old["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn asks_instead_of_guessing_then_writes_after_confirmation() {
    let tmp = tempfile::tempdir().unwrap();
    let flight = tmp.path().join("flight");
    let base = tmp.path().join("base").join("BASE_MO.rnx");
    let out = tmp.path().join("out");
    azimut(&["demo", &p(&flight), "--captures", "4"]);
    azimut(&["demo-base", &p(&base)]);

    let mut s = Server::start();
    s.request("initialize", json!({ "protocolVersion": "2025-06-18" }));

    let scan = s.call("scan_folder", json!({ "dir": p(&flight) }));
    assert_eq!(scan["flights"][0]["captures"], 4);
    assert_eq!(scan["flights"][0]["rtk"]["fixed"], 2);

    // PPK: no base file covers the flight, then no base coordinates: questions, no run.
    let r = s.call("process_ppk", json!({ "dir": p(&flight), "out": p(&out) }));
    assert_eq!(r["status"], "needs_decision");
    assert_eq!(r["questions"][0]["id"], "base_files");
    let r = s.call(
        "process_ppk",
        json!({ "dir": p(&flight), "out": p(&out), "base_files": [p(&base)] }),
    );
    assert_eq!(r["status"], "needs_decision");
    assert_eq!(r["questions"][0]["id"], "base_position");
    let r = s.call(
        "process_ppk",
        json!({ "dir": p(&flight), "out": p(&out), "base_files": [p(&base)],
                "base_position": { "lat": 46.349, "lon": -72.551, "h": 60.0 } }),
    );
    assert_eq!(
        r["questions"][0]["id"], "frame",
        "coordinates without a frame are not used"
    );
    assert!(!out.exists(), "nothing written while decisions are pending");

    // RTK shift: the frame is asked for, then the run returns the report summary.
    let r = s.call(
        "shift_rtk",
        json!({ "dir": p(&flight), "out": p(&out), "enu": [0.1, 0.2, 0.3] }),
    );
    assert_eq!(r["questions"][0]["id"], "frame");
    let r = s.call(
        "shift_rtk",
        json!({ "dir": p(&flight), "out": p(&out), "enu": [0.1, 0.2, 0.3], "frame": "NAD83(CSRS) v8, epoch 2010.0" }),
    );
    assert_eq!(r["status"], "ok", "{r}");
    let rep = &r["reports"][0];
    assert_eq!(rep["frame"], "NAD83(CSRS) v8, epoch 2010.0");
    assert_eq!(rep["summary"]["exposures"], 4);
    assert!(Path::new(rep["report_html"].as_str().unwrap()).exists());
    assert!(Path::new(rep["pix4dmatic_notes"].as_str().unwrap()).exists());
    let solution = rep["solution"].as_str().unwrap().to_string();

    // Images: mode asked, then a dry run waiting for confirmation, then the copies.
    let r = s.call("write_images", json!({ "solution": solution }));
    assert_eq!(r["questions"][0]["id"], "mode");
    let r = s.call("write_images", json!({ "solution": solution, "mode": "copy" }));
    assert_eq!(r["status"], "needs_confirmation");
    assert_eq!(r["dry_run"]["would_write"], 20, "{r}");
    let copies = tmp.path().join("copies");
    assert!(!copies.exists());
    let r = s.call(
        "write_images",
        json!({ "solution": solution, "mode": "copy", "images_dir": p(&copies), "confirm": true }),
    );
    assert_eq!(r["status"], "ok", "{r}");
    assert_eq!(std::fs::read_dir(&copies).unwrap().count(), 20);

    let full = s.call("read_report", json!({ "file": rep["report_json"] }));
    assert_eq!(full["exposures"].as_array().unwrap().len(), 4);
}
