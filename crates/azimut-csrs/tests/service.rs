//! The client against a local stand-in for CSRS-PPP that answers like the real service.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use azimut_csrs::{extract, Client, Error, ProcessType, RefSystem, Request, WaitOptions};

/// Same layout as a real version 4 summary; values are made up.
const SUM: &str = "\
VER 4.14.11 (2024-11-29)
RNX BASE2070.24O
MKR BASE
MOD STATIC
ANT NOT FOUND
ARP    0.000    0.000    0.000
IAR 97.50%
POS CRD  SYST        EPOCH          A_PRIORI         ESTIMATED       DIFF SIGMA(95%) CORRELATIONS
POS   X IGS20 24:207:72285       1196000.0000      1196000.5000     0.5000     0.0057  1.0000
POS LAT IGS20 24:207:72285    46 20 56.90000    46 20 56.84444    -1.7150     0.0052  1.0000
POS LON IGS20 24:207:72285   -72 33  4.40000   -72 33  4.44444    -0.9500     0.0043  0.0109  1.0000
POS HGT IGS20 24:207:72285           33.7062           31.2040    -2.5022     0.0195  0.2589 -0.0595  1.0000
";

const KEY: &str = "3Ny0To_m6Wmjr28aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaacps3FG";

struct Seen {
    method: String,
    target: String,
    headers: String,
    body: Vec<u8>,
}

/// Serve `answer(method, target)` on a local port; returns the base URL and every request seen.
fn serve(answer: impl Fn(&str, &str) -> (u16, Vec<u8>) + Send + 'static) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/CSRS-PPP/service", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let target = parts.next().unwrap_or_default().to_string();
            let mut headers = String::new();
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
                headers.push_str(&h);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let (status, reply) = answer(&method, &target);
            log.lock().unwrap().push(Seen {
                method,
                target,
                headers,
                body,
            });
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            )
            .unwrap();
            stream.write_all(&reply).unwrap();
        }
    });
    (url, seen)
}

fn results_zip(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, text) in entries {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn rinex(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("BASE2070.24O");
    std::fs::write(
        &path,
        "     3.04           OBSERVATION DATA    M                   RINEX VERSION / TYPE\n",
    )
    .unwrap();
    path
}

fn fast() -> WaitOptions {
    WaitOptions {
        interval: Duration::from_millis(5),
        timeout: Duration::from_secs(10),
    }
}

#[test]
fn submit_wait_download_extract() {
    let polls = Arc::new(Mutex::new(0));
    let p = polls.clone();
    let archive = results_zip(&[("BASE2070.sum", SUM), ("BASE2070.pos", "pos"), ("errors.txt", "")]);
    let (url, seen) = serve(move |method, target| match (method, target) {
        ("POST", t) if t.ends_with("/CSRS-PPP/service/submit") => (200, format!("{KEY}\n").into_bytes()),
        ("GET", t) if t.ends_with(&format!("/results/status?id={KEY}")) => {
            let mut n = p.lock().unwrap();
            *n += 1;
            (200, if *n < 3 { "processing" } else { "done" }.into())
        }
        ("GET", t) if t.ends_with(&format!("/results/file?id={KEY}")) => (200, archive.clone()),
        _ => (404, b"not found".to_vec()),
    });
    let tmp = tempfile::tempdir().unwrap();
    let client = Client::with_base_url(&url).unwrap();
    let req = Request {
        rinex: &rinex(tmp.path()),
        email: "lab@example.org",
        process_type: ProcessType::Static,
        system: RefSystem::Itrf,
    };

    let key = client.submit(&req).unwrap();
    assert_eq!(key, KEY);
    let mut waits = 0;
    client.wait(&key, &fast(), |_| waits += 1).unwrap();
    assert_eq!(waits, 2);
    let zip = client.download(&key).unwrap();
    let out = tmp.path().join("ppp");
    let r = extract(&key, &zip, &out).unwrap();
    assert_eq!(r.files.len(), 3);
    assert_eq!(r.summary_path, out.join("BASE2070.sum"));
    assert_eq!(r.summary.rinex.as_deref(), Some("BASE2070.24O"));
    let p = r.summary.position(None).unwrap();
    assert_eq!(p.system, "IGS20");
    assert_eq!(p.h_ell_m, 31.204);

    let seen = seen.lock().unwrap();
    let submit = &seen[0];
    assert_eq!(submit.method, "POST");
    assert!(submit
        .headers
        .contains("User-Agent: Mozilla/5.0 (Python CSRS-PPP access)"));
    assert!(submit
        .headers
        .contains("Content-Type: multipart/form-data; boundary="));
    let body = String::from_utf8_lossy(&submit.body);
    for field in [
        "name=\"return_email\"\r\n\r\nlab@example.org\r\n",
        "name=\"user_name\"\r\n\r\nlab@example.org\r\n",
        "name=\"ppp_access\"\r\n\r\nnobrowser_status\r\n",
        "name=\"process_type\"\r\n\r\nStatic\r\n",
        "name=\"sysref\"\r\n\r\nITRF\r\n",
        "name=\"rfile_upload\"; filename=\"BASE2070.24O\"",
        "OBSERVATION DATA",
    ] {
        assert!(body.contains(field), "{field:?} missing in\n{body}");
    }
    assert!(seen.iter().all(|s| !s.target.is_empty()));
}

#[test]
fn refusal_carries_the_body() {
    let (url, _) = serve(|_, _| (200, b"<html><p>The RINEX file is not valid</p></html>".to_vec()));
    let tmp = tempfile::tempdir().unwrap();
    let req = Request {
        rinex: &rinex(tmp.path()),
        email: "lab@example.org",
        process_type: ProcessType::Kinematic,
        system: RefSystem::Nad83,
    };
    let e = Client::with_base_url(&url).unwrap().submit(&req).unwrap_err();
    assert!(
        matches!(&e, Error::Rejected { status: 200, body } if body.contains("not valid")),
        "{e}"
    );
}

#[test]
fn unknown_status_is_an_error() {
    let (url, _) = serve(|_, _| (200, b"error".to_vec()));
    let e = Client::with_base_url(&url)
        .unwrap()
        .wait(KEY, &fast(), |_| {})
        .unwrap_err();
    assert!(matches!(&e, Error::Status { text, .. } if text == "error"), "{e}");
}

#[test]
fn timeout_keeps_the_key() {
    let (url, _) = serve(|_, _| (200, b"processing".to_vec()));
    let opts = WaitOptions {
        interval: Duration::from_millis(5),
        timeout: Duration::from_millis(30),
    };
    let e = Client::with_base_url(&url)
        .unwrap()
        .wait(KEY, &opts, |_| {})
        .unwrap_err();
    assert!(matches!(&e, Error::Timeout { key, .. } if key == KEY), "{e}");
    assert!(e.to_string().contains(KEY));
}

#[test]
fn download_must_be_a_zip() {
    let (url, _) = serve(|_, _| (200, b"<html>expired</html>".to_vec()));
    let e = Client::with_base_url(&url).unwrap().download(KEY).unwrap_err();
    assert!(e.to_string().contains("expired"), "{e}");
}

#[test]
fn archive_paths_stay_in_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let zip = results_zip(&[("sub/dir/X.sum", SUM), ("X.pos", "pos")]);
    let r = extract(KEY, &zip, &out).unwrap();
    assert_eq!(r.files, vec![out.join("X.sum"), out.join("X.pos")]);
    let zip = results_zip(&[("../../escape.sum", SUM)]);
    let e = extract(KEY, &zip, &out).unwrap_err();
    assert!(e.to_string().contains("unsafe file name"), "{e}");
    assert!(!tmp.path().join("escape.sum").exists());
}

#[test]
fn missing_summary_reports_errors_txt() {
    let tmp = tempfile::tempdir().unwrap();
    let zip = results_zip(&[("errors.txt", "Not enough observations\n")]);
    let e = extract(KEY, &zip, tmp.path()).unwrap_err();
    assert!(e.to_string().contains("Not enough observations"), "{e}");
    assert!(extract(KEY, b"not a zip", tmp.path()).is_err());
}
