//! igs20.atx in Azimut's cache: `azimut antex` and `process` against a local stand-in for
//! files.igs.org (`AZIMUT_ANTEX_URL`). Every run gets its own cache folder
//! (`AZIMUT_CACHE_DIR`), so the user's cached file is never read or written.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// The embedded Emlid entries plus a Trimble antenna, as a small stand-in for igs20.atx.
fn igs20() -> String {
    let emlid = include_str!("../../azimut-solve/data/emlid_igs20.atx");
    let start = emlid[..emlid.find("START OF ANTENNA").unwrap()]
        .rfind('\n')
        .unwrap()
        + 1;
    let end = start + emlid[start..].find("END OF ANTENNA").unwrap();
    let end = end + emlid[end..].find('\n').unwrap() + 1;
    let trimble = emlid[start..end].replace("EML_REACH_RS2   NONE", "TRM59800.00     NONE");
    format!("{emlid}{trimble}")
}

/// Serve `body` to every request; returns the URL and the request count.
fn serve(status: u16, body: String) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/pub/station/general/igs20.atx",
        listener.local_addr().unwrap()
    );
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut h = String::new();
                reader.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
            }
            counter.fetch_add(1, Ordering::SeqCst);
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (url, hits)
}

/// A URL nothing listens on.
fn offline() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/igs20.atx", listener.local_addr().unwrap());
    drop(listener);
    url
}

fn azimut(url: &str, cache: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_azimut"))
        .args(args)
        .env("AZIMUT_ANTEX_URL", url)
        .env("AZIMUT_CACHE_DIR", cache)
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn downloads_once_then_works_offline() {
    let cache = tempfile::tempdir().unwrap();
    let (url, hits) = serve(200, igs20());
    let o = azimut(&url, cache.path(), &["antex"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("(downloaded)"), "{}", text(&o));
    assert_eq!(
        std::fs::read_to_string(cache.path().join("igs20.atx")).unwrap(),
        igs20()
    );
    assert!(!cache.path().join("igs20.atx.part").exists());

    let o = azimut(
        &offline(),
        cache.path(),
        &["antex", "--antenna", "TRM59800.00 NONE"],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("(cached)"), "{}", text(&o));
    assert!(text(&o).contains("model found"), "{}", text(&o));
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let o = azimut(&url, cache.path(), &["antex", "--update"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn unknown_antenna_is_refused() {
    let cache = tempfile::tempdir().unwrap();
    let (url, _) = serve(200, igs20());
    let o = azimut(
        &url,
        cache.path(),
        &["antex", "--antenna", "ACME_X1         NONE"],
    );
    assert!(!o.status.success());
    assert!(text(&o).contains("no model for ACME_X1"), "{}", text(&o));
}

#[test]
fn embedded_antenna_needs_no_download() {
    let cache = tempfile::tempdir().unwrap();
    let o = azimut(
        &offline(),
        cache.path(),
        &["antex", "--antenna", "EML_REACH_RS2 NONE"],
    );
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains("embedded"), "{}", text(&o));
    assert!(!cache.path().join("igs20.atx").exists());
}

#[test]
fn a_page_that_is_not_antex_is_not_cached() {
    let cache = tempfile::tempdir().unwrap();
    let (url, _) = serve(200, "<html>maintenance</html>".into());
    let o = azimut(&url, cache.path(), &["antex"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("complete ANTEX"), "{}", text(&o));
    assert!(!cache.path().join("igs20.atx").exists());
    assert!(!cache.path().join("igs20.atx.part").exists());

    let (url, _) = serve(404, "Not found".into());
    let o = azimut(&url, cache.path(), &["antex"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("HTTP 404"), "{}", text(&o));
}

/// CSRS-PPP summary of a base on a Trimble antenna (not embedded), reference point.
const SUM: &str = "\
VER 4.14.11 (2024-11-29)
RNX BASE2070.24O
MOD STATIC
ANT TRM59800.00     NONE
ARP    0.000    0.000    0.000
POS LAT IGS20 24:207:72285    46 20 56.90000    46 20 56.84444    -1.7150     0.0052  1.0000
POS LON IGS20 24:207:72285   -72 33  4.40000   -72 33  4.44444    -0.9500     0.0043  0.0109  1.0000
POS HGT IGS20 24:207:72285           33.7062           31.2040    -2.5022     0.0195  0.2589 -0.0595  1.0000
";

/// `process` with that summary on an empty folder (no flight), stand-in RTKLIB programs.
fn process(url: &str, cache: &Path) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let rtklib = tmp.path().join("rtklib");
    std::fs::create_dir_all(&rtklib).unwrap();
    for exe in ["convbin", "rnx2rtkp", "convbin.exe", "rnx2rtkp.exe"] {
        std::fs::write(rtklib.join(exe), "").unwrap();
    }
    let flights = tmp.path().join("flights");
    std::fs::create_dir_all(&flights).unwrap();
    let sum = tmp.path().join("BASE2070.sum");
    std::fs::write(&sum, SUM).unwrap();
    let out = tmp.path().join("out");
    azimut(
        url,
        cache,
        &[
            "process",
            flights.to_str().unwrap(),
            "--base-ppp",
            sum.to_str().unwrap(),
            "--rtklib",
            rtklib.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ],
    )
}

#[test]
fn process_fetches_the_base_antenna_model() {
    let cache = tempfile::tempdir().unwrap();
    let (url, hits) = serve(200, igs20());
    let o = process(&url, cache.path());
    let t = text(&o);
    assert!(
        t.contains("TRM59800.00     NONE from igs20.atx (downloaded"),
        "{t}"
    );
    assert!(!t.contains("--antex (e.g. igs20.atx)"), "{t}");
    let o = process(&offline(), cache.path());
    assert!(text(&o).contains("from igs20.atx (cached"), "{}", text(&o));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn process_offline_says_how_to_give_the_file() {
    let cache = tempfile::tempdir().unwrap();
    let o = process(&offline(), cache.path());
    assert!(!o.status.success());
    let t = text(&o);
    assert!(t.contains("could not be downloaded"), "{t}");
    assert!(t.contains("--antex"), "{t}");
    assert!(
        t.contains(&cache.path().join("igs20.atx").display().to_string()),
        "{t}"
    );
}
