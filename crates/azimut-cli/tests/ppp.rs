//! `azimut ppp` against a local stand-in for CSRS-PPP (`AZIMUT_CSRS_URL`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};

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
    Command::new(env!("CARGO_BIN_EXE_azimut"))
        .args(args)
        .env("AZIMUT_CSRS_URL", url)
        .env_remove("AZIMUT_CSRS_EMAIL")
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
