//! `--basemap` with a local stand-in tile server: the tiles end up inside the report, and a
//! server that cannot be reached leaves the report without a background.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

/// Smallest PNG: one transparent pixel.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00,
    0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01,
    0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// Serve PNG tiles with `content_type`; returns the URL template and the paths asked for.
fn serve(content_type: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/tiles/{{z}}/{{x}}/{{y}}.png",
        listener.local_addr().unwrap()
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
            }
            log.lock()
                .unwrap()
                .push(line.split(' ').nth(1).unwrap_or_default().to_string());
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                PNG.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(PNG).unwrap();
        }
    });
    (url, seen)
}

fn azimut(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_azimut"))
        .args(args)
        .env_remove("AZIMUT_BASEMAP")
        .env_remove("AZIMUT_BASEMAP_ATTRIBUTION")
        .env("NO_PROXY", "127.0.0.1")
        .output()
        .unwrap()
}

/// A synthetic flight shifted with `extra` arguments; returns the HTML report and stdout.
fn shifted(tmp: &Path, extra: &[&str]) -> (String, String) {
    let flight = tmp.join("DJI_202606151400_001_Demo");
    let o = azimut(&["demo", flight.to_str().unwrap(), "--captures", "12"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = tmp.join("out");
    let mut args = vec!["shift", flight.to_str().unwrap(), "--enu", "0.42,-0.18,0.65"];
    args.extend(["--out", out.to_str().unwrap()]);
    args.extend(extra);
    let o = azimut(&args);
    let stdout = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        o.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let html = std::fs::read_to_string(out.join("DJI_202606151400_001_Demo_report.html")).unwrap();
    (html, stdout)
}

#[test]
fn tiles_are_embedded_with_their_credit() {
    let (url, seen) = serve("image/png");
    let tmp = tempfile::tempdir().unwrap();
    let (html, stdout) = shifted(
        tmp.path(),
        &["--basemap", &url, "--basemap-attribution", "Test imagery & co"],
    );
    let asked = seen.lock().unwrap().len();
    assert!(asked > 0 && asked <= 48, "{asked} tiles");
    assert_eq!(html.matches("<image ").count(), asked);
    assert!(html.contains("href=\"data:image/png;base64,iVBORw0KGgo"));
    assert!(
        html.contains("Background: Test imagery &amp; co"),
        "credit missing"
    );
    assert!(html.contains("class=\"map sat\""));
    assert!(stdout.contains("basemap:"), "{stdout}");
    // Still nothing outside the page.
    assert!(!html.contains("http://127.0.0.1"));
}

#[test]
fn unreachable_server_leaves_the_report_without_background() {
    // Bind then drop: nothing listens on that port any more.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let url = format!("http://127.0.0.1:{port}/{{z}}/{{x}}/{{y}}.png");
    let tmp = tempfile::tempdir().unwrap();
    let (html, stdout) = shifted(tmp.path(), &["--basemap", &url, "--basemap-attribution", "Test"]);
    assert!(stdout.contains("basemap left out"), "{stdout}");
    assert!(!html.contains("<image "));
    assert!(!html.contains("Background:"));
    assert!(html.contains("<svg viewBox=\"0 0 560 420\""));
}

#[test]
fn a_page_that_is_not_an_image_is_refused() {
    let (url, _) = serve("text/html");
    let tmp = tempfile::tempdir().unwrap();
    let (html, stdout) = shifted(tmp.path(), &["--basemap", &url, "--basemap-attribution", "Test"]);
    assert!(stdout.contains("not an image"), "{stdout}");
    assert!(!html.contains("<image "));
}

#[test]
fn a_tile_url_needs_a_credit() {
    let tmp = tempfile::tempdir().unwrap();
    let flight = tmp.path().join("f");
    std::fs::create_dir(&flight).unwrap();
    let o = azimut(&[
        "shift",
        flight.to_str().unwrap(),
        "--enu",
        "0,0,0",
        "--basemap",
        "https://tiles.example.org/{z}/{x}/{y}.png",
    ]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("--basemap-attribution"));
}
