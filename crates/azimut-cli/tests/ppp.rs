//! `azimut ppp` and the MCP `submit_ppp` tool against a local stand-in for CSRS-PPP
//! (`AZIMUT_CSRS_URL`). Every run gets its own settings folder (`AZIMUT_CONFIG_DIR`), so the
//! user's remembered e-mail is never read or written.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const KEY: &str = "3Ny0To_m6Wmjr28aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaacps3FG";

/// Same layout as a real version 4 summary; values are made up.
const SUM: &str = "\
VER 4.14.11 (2024-11-29)
RNX BASE2070.24O
MOD STATIC
ANT NOT FOUND
ARP    0.000    0.000    0.000
POS LAT IGS20 24:207:72285    46 20 56.90000    46 20 56.84444    -1.7150     0.0052  1.0000
POS LON IGS20 24:207:72285   -72 33  4.40000   -72 33  4.44444    -0.9500     0.0043  0.0109  1.0000
POS HGT IGS20 24:207:72285           33.7062           31.2040    -2.5022     0.0195  0.2589 -0.0595  1.0000
";

/// Answer submissions with KEY, status with `status`, results with a zip holding SUM.
fn serve(status: &'static str) -> String {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("BASE2070.sum", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(SUM.as_bytes()).unwrap();
    zip.start_file("errors.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"WARNING: no Galileo antenna calibration, GPS values used\n")
        .unwrap();
    let archive = zip.finish().unwrap().into_inner();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/CSRS-PPP/service", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut length = 0;
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            let reply: Vec<u8> = if line.contains("/submit") {
                KEY.into()
            } else if line.contains("/results/status") {
                status.into()
            } else {
                archive.clone()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            )
            .unwrap();
            stream.write_all(&reply).unwrap();
        }
    });
    url
}

fn azimut(url: &str, args: &[&str]) -> Output {
    let settings = tempfile::tempdir().unwrap();
    azimut_with(url, settings.path(), args)
}

fn azimut_with(url: &str, settings: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_azimut"))
        .args(args)
        .env("AZIMUT_CSRS_URL", url)
        .env("AZIMUT_CONFIG_DIR", settings)
        .env_remove("AZIMUT_CSRS_EMAIL")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn submit_and_save_results() {
    let url = serve("done");
    let tmp = tempfile::tempdir().unwrap();
    let rinex = tmp.path().join("BASE2070.24O");
    std::fs::write(&rinex, "RINEX").unwrap();
    let out = tmp.path().join("ppp");
    let o = azimut(
        &url,
        &[
            "ppp",
            rinex.to_str().unwrap(),
            "--email",
            "lab@example.org",
            "--out",
            out.to_str().unwrap(),
        ],
    );
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(stdout.contains(KEY), "{stdout}");
    assert!(stdout.contains("46.349123"), "{stdout}");
    assert!(stdout.contains("--base-ppp"), "{stdout}");
    assert!(
        stdout.contains("CSRS-PPP notes (errors.txt):\n     WARNING: no Galileo antenna calibration"),
        "{stdout}"
    );
    assert!(out.join("BASE2070.sum").is_file());
    assert!(out.join("BASE2070_csrs_ppp.zip").is_file());
}

#[test]
fn email_is_required() {
    let url = serve("done");
    let tmp = tempfile::tempdir().unwrap();
    let rinex = tmp.path().join("BASE2070.24O");
    std::fs::write(&rinex, "RINEX").unwrap();
    let o = azimut(&url, &["ppp", rinex.to_str().unwrap()]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("AZIMUT_CSRS_EMAIL"));
}

#[test]
fn failed_processing_is_reported() {
    let url = serve("error");
    let tmp = tempfile::tempdir().unwrap();
    let o = azimut(
        &url,
        &["ppp", "--resume", KEY, "--out", tmp.path().to_str().unwrap()],
    );
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("unexpected answer \"error\""), "{err}");
}

#[test]
fn remembered_email_is_used_and_a_bad_one_refused() {
    let url = serve("done");
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join("settings");
    std::fs::create_dir_all(&settings).unwrap();
    std::fs::write(
        settings.join("config.json"),
        r#"{ "csrs_email": "lab@example.org" }"#,
    )
    .unwrap();
    let rinex = tmp.path().join("BASE2070.24O");
    std::fs::write(&rinex, "RINEX").unwrap();
    let out = tmp.path().join("ppp");
    let args = ["ppp", rinex.to_str().unwrap(), "--out", out.to_str().unwrap()];
    let o = azimut_with(&url, &settings, &args);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.join("BASE2070.sum").is_file());

    let o = azimut_with(
        &url,
        &settings,
        &[&args[..], &["--email", "not-an-address"]].concat(),
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("is not an e-mail address"));
}

/// One JSON-RPC exchange per call with `azimut mcp`; returns the tool's structured result.
struct Mcp {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    id: u64,
}

impl Mcp {
    fn start(url: &str, settings: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_azimut"))
            .arg("mcp")
            .env("AZIMUT_CSRS_URL", url)
            .env("AZIMUT_CONFIG_DIR", settings)
            .env_remove("AZIMUT_CSRS_EMAIL")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Mcp {
            child,
            stdin,
            stdout,
            id: 0,
        }
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        self.id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": self.id, "method": "tools/call",
                          "params": { "name": tool, "arguments": args } });
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "server closed");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get("id") == Some(&json!(self.id)) {
                return v["result"]["structuredContent"].clone();
            }
        }
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn mcp_submit_asks_email_then_confirmation_then_returns_the_summary() {
    let url = serve("done");
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join("settings");
    let rinex = tmp.path().join("BASE2070.24O");
    std::fs::write(&rinex, "RINEX").unwrap();
    let out = tmp.path().join("ppp");
    let base = json!({ "rinex": rinex, "out": out });
    let mut s = Mcp::start(&url, &settings);

    let r = s.call("submit_ppp", base.clone());
    assert_eq!(r["status"], "needs_decision", "{r}");
    assert_eq!(r["questions"][0]["id"], "email");

    let mut with_email = base.clone();
    with_email["email"] = json!("lab@example.org");
    let r = s.call("submit_ppp", with_email);
    assert_eq!(r["status"], "needs_confirmation", "{r}");
    assert!(!out.exists(), "nothing sent before the confirmation");
    let saved = std::fs::read_to_string(settings.join("config.json")).unwrap();
    assert!(saved.contains("lab@example.org"), "{saved}");

    // The remembered e-mail is used; the confirmation sends the file.
    let mut confirmed = base.clone();
    confirmed["confirm"] = json!(true);
    let r = s.call("submit_ppp", confirmed);
    assert_eq!(r["status"], "ok", "{r}");
    assert_eq!(r["tracking_key"], KEY);
    assert!(r["summary_file"].as_str().unwrap().ends_with("BASE2070.sum"));
    assert!(r["coordinates_refer_to"]
        .as_str()
        .unwrap()
        .contains("phase centre"));
}

#[test]
fn mcp_failed_resume_is_not_pending() {
    let url = serve("error");
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Mcp::start(&url, &tmp.path().join("settings"));
    let r = s.call(
        "submit_ppp",
        json!({ "out": tmp.path().join("ppp"), "resume": KEY }),
    );
    assert_eq!(r["status"], "failed", "{r}");
    assert_eq!(r["tracking_key"], KEY);
}

#[cfg(unix)]
#[test]
fn remembered_email_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let url = serve("done");
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join("settings");
    let rinex = tmp.path().join("BASE2070.24O");
    std::fs::write(&rinex, "RINEX").unwrap();
    let mut s = Mcp::start(&url, &settings);
    s.call(
        "submit_ppp",
        json!({ "rinex": rinex, "out": tmp.path().join("ppp"), "email": "lab@example.org" }),
    );
    let mode = std::fs::metadata(settings.join("config.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
