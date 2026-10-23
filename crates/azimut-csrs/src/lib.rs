//! # azimut-csrs
//!
//! Submission of a base-station RINEX to NRCan CSRS-PPP, wait for the result and read the
//! precise position from the summary it returns.
//!
//! CSRS-PPP has no documented public API. This crate speaks the interface used by NRCan's own
//! `csrs_ppp_auto.py` script: submit the RINEX as a form, receive a tracking key, poll its
//! status, download a zip of the results. Everything that depends on that interface (URL,
//! form fields, user agent, status words, archive contents) lives in this file, so that a
//! change on NRCan's side is fixed here only. See SOURCES.md (CSRS-PPP service).
//!
//! The summary is read with [`azimut_io::csrs_ppp`], the same reader as `--base-ppp`.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use azimut_io::csrs_ppp::{self, PppSummary};

/// Service root used by `csrs_ppp_auto.py`.
pub const BASE_URL: &str = "https://webapp.csrs-scrs.nrcan-rncan.gc.ca/CSRS-PPP/service";
/// Environment variable that replaces [`BASE_URL`], for tests or if NRCan moves the service.
pub const URL_ENV: &str = "AZIMUT_CSRS_URL";
/// User agent of `csrs_ppp_auto.py`; the service answers it with plain-text keys and status.
const USER_AGENT: &str = "Mozilla/5.0 (Python CSRS-PPP access)";
/// Largest results archive accepted (a day at 1 s gives a few megabytes).
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// Errors raised by azimut-csrs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{url}: {reason}")]
    Http { url: String, reason: String },
    #[error("CSRS-PPP refused the submission (HTTP {status}): {body}")]
    Rejected { status: u16, body: String },
    #[error("CSRS-PPP status of {key}: unexpected answer {text:?}")]
    Status { key: String, text: String },
    #[error(
        "CSRS-PPP still processing {key} after {} s; fetch the results later with this key",
        waited.as_secs()
    )]
    Timeout { key: String, waited: Duration },
    #[error("CSRS-PPP results of {key}: {reason}")]
    Results { key: String, reason: String },
    #[error("CSRS-PPP request: {0}")]
    Request(String),
    #[error(transparent)]
    Summary(#[from] azimut_io::Error),
}

fn io(path: &Path, source: std::io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Processing mode (`process_type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessType {
    Static,
    Kinematic,
}

/// Reference system of the results (`sysref`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefSystem {
    /// ITRF at the epoch of the observations (labelled `IGS20` or similar in the summary).
    Itrf,
    /// NAD83(CSRS), at the current epoch, heights in CGVD2013.
    Nad83,
}

/// What to submit.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    /// RINEX observation file, sent as is (compressed files are accepted by the service).
    pub rinex: &'a Path,
    /// E-mail of the user's CSRS-PPP account.
    pub email: &'a str,
    pub process_type: ProcessType,
    pub system: RefSystem,
}

/// Form fields of a submission, in the order `csrs_ppp_auto.py` sends them, file excluded.
/// `nad83_epoch` and `v_datum` are always sent; the service reads them only for NAD83.
pub fn form_fields(req: &Request) -> Result<Vec<(&'static str, String)>, Error> {
    let email = req.email.trim();
    let valid = email.contains('@')
        && !email
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"');
    if !valid {
        return Err(Error::Request(format!("{email:?} is not an e-mail address")));
    }
    let process_type = match req.process_type {
        ProcessType::Static => "Static",
        ProcessType::Kinematic => "Kinematic",
    };
    let sysref = match req.system {
        RefSystem::Itrf => "ITRF",
        RefSystem::Nad83 => "NAD83",
    };
    Ok(vec![
        ("return_email", email.to_string()),
        ("user_name", email.to_string()),
        ("cmd_process_type", "std".into()),
        ("ppp_access", "nobrowser_status".into()),
        ("language", "en".into()),
        ("process_type", process_type.into()),
        ("sysref", sysref.into()),
        ("nad83_epoch", "CURR".into()),
        ("v_datum", "cgvd2013".into()),
        ("output_pdf", "lite".into()),
    ])
}

/// Field holding the RINEX file.
const FILE_FIELD: &str = "rfile_upload";

/// `multipart/form-data` body (RFC 7578): text fields, then the file. Returns the content
/// type with its boundary, and the body.
pub fn multipart(fields: &[(&str, String)], file_name: &str, data: &[u8]) -> (String, Vec<u8>) {
    // A boundary that does not occur in the file.
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ u64::from(std::process::id());
    let boundary = loop {
        let b = format!("azimut-{seed:016x}");
        if !data.windows(b.len()).any(|w| w == b.as_bytes()) {
            break b;
        }
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    };
    // Quotes and line breaks would end the header.
    let file_name: String = file_name
        .chars()
        .map(|c| if c == '"' || c.is_control() { '_' } else { c })
        .collect();
    let mut body = Vec::with_capacity(data.len() + 2048);
    for (name, value) in fields {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{FILE_FIELD}\"; \
             filename=\"{file_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Tracking key from the submission reply: HTTP 200 and a plain-text key, nothing else.
/// Keys seen are about 64 letters, digits and `_`; URL-safe characters only are accepted, so
/// that the key goes into a query string unchanged.
pub fn tracking_key(status: u16, body: &str) -> Result<String, Error> {
    let key = body.trim();
    let url_safe = key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'));
    if status == 200 && (16..=256).contains(&key.len()) && url_safe {
        Ok(key.to_string())
    } else {
        Err(Error::Rejected {
            status,
            body: key.to_string(),
        })
    }
}

/// Processing state of a submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Processing,
    Done,
}

/// Status from the plain-text reply. Anything but `processing` or `done` (the service
/// answers `error` for an unknown key) is an error carrying the text received.
pub fn parse_status(key: &str, text: &str) -> Result<Status, Error> {
    match text.trim() {
        t if t.eq_ignore_ascii_case("processing") => Ok(Status::Processing),
        t if t.eq_ignore_ascii_case("done") => Ok(Status::Done),
        t => Err(Error::Status {
            key: key.to_string(),
            text: t.to_string(),
        }),
    }
}

/// Polling of [`Client::wait`].
#[derive(Debug, Clone, Copy)]
pub struct WaitOptions {
    pub interval: Duration,
    pub timeout: Duration,
}

impl Default for WaitOptions {
    /// Every 10 s, for at most 15 min. A 4 to 8 h base session at 30 s takes about a minute.
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(10),
            timeout: Duration::from_secs(15 * 60),
        }
    }
}

/// Files extracted from a results archive.
#[derive(Debug, Clone)]
pub struct Results {
    /// Every file written, in archive order.
    pub files: Vec<PathBuf>,
    /// The summary (`*.sum`) among them.
    pub summary_path: PathBuf,
    pub summary: PppSummary,
    /// `errors.txt` when not empty: the service's warnings (e.g. a missing antenna
    /// calibration) even when it produced a solution.
    pub notes: Option<String>,
}

/// Write the files of a results archive into `dir` and read its summary. Archive paths are
/// flattened to their file names, so nothing is written outside `dir`. `key` is only used in
/// error messages.
pub fn extract(key: &str, archive: &[u8], dir: &Path) -> Result<Results, Error> {
    let bad = |reason: String| Error::Results {
        key: key.to_string(),
        reason,
    };
    let mut zip =
        zip::ZipArchive::new(Cursor::new(archive)).map_err(|e| bad(format!("not a zip archive ({e})")))?;
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let mut files: Vec<PathBuf> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| bad(format!("archive entry {i}: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.to_owned()))
        else {
            return Err(bad(format!("unsafe file name {:?} in the archive", entry.name())));
        };
        let path = dir.join(name);
        if files.contains(&path) {
            return Err(bad(format!("{} appears twice in the archive", path.display())));
        }
        let mut out = std::fs::File::create(&path).map_err(|e| io(&path, e))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| io(&path, e))?;
        files.push(path);
    }
    let sums: Vec<&PathBuf> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sum")))
        .collect();
    // The service writes its diagnosis and warnings in errors.txt.
    let notes = files
        .iter()
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("errors.txt"))
        })
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let summary_path = match sums.as_slice() {
        [one] => (*one).clone(),
        [] => {
            return Err(bad(match notes {
                Some(t) => format!("no summary (.sum) in the archive; errors.txt says: {t}"),
                None => "no summary (.sum) in the archive".into(),
            }));
        }
        _ => return Err(bad("several summaries (.sum) in the archive".into())),
    };
    let summary = csrs_ppp::read(&summary_path)?;
    Ok(Results {
        files,
        summary_path,
        summary,
        notes,
    })
}

/// Connection to the CSRS-PPP service.
pub struct Client {
    agent: ureq::Agent,
    base_url: String,
}

impl Client {
    /// The official service, or the URL in `AZIMUT_CSRS_URL`. Proxies are taken from the
    /// usual environment variables.
    pub fn new() -> Result<Self, Error> {
        match std::env::var(URL_ENV) {
            Ok(url) if !url.trim().is_empty() => Self::with_base_url(url.trim()),
            _ => Self::with_base_url(BASE_URL),
        }
    }

    /// A service at `url`. Fails only on a malformed proxy variable.
    pub fn with_base_url(url: &str) -> Result<Self, Error> {
        let mut builder = ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(300))
            .timeout_write(Duration::from_secs(300));
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(p) = proxy_for(url, env) {
            let proxy = ureq::Proxy::new(&p).map_err(|e| Error::Request(format!("proxy {p}: {e}")))?;
            builder = builder.proxy(proxy);
        }
        Ok(Self {
            agent: builder.build(),
            base_url: url.trim_end_matches('/').to_string(),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Submit a RINEX; returns the tracking key.
    pub fn submit(&self, req: &Request) -> Result<String, Error> {
        let fields = form_fields(req)?;
        let data = std::fs::read(req.rinex).map_err(|e| io(req.rinex, e))?;
        let name = req
            .rinex
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "base.obs".into());
        let (content_type, body) = multipart(&fields, &name, &data);
        let url = format!("{}/submit", self.base_url);
        let reply = self
            .agent
            .post(&url)
            .set("Content-Type", &content_type)
            .send_bytes(&body);
        let (status, text) = text_reply(&url, reply)?;
        tracking_key(status, &text)
    }

    /// Current state of a submission.
    pub fn status(&self, key: &str) -> Result<Status, Error> {
        let url = format!("{}/results/status?id={key}", self.base_url);
        let (status, text) = text_reply(&url, self.agent.get(&url).call())?;
        if status != 200 {
            return Err(Error::Http {
                url,
                reason: format!("HTTP {status}: {}", text.trim()),
            });
        }
        parse_status(key, &text)
    }

    /// Poll until the submission is done. `on_poll` receives the time waited after each
    /// `processing` answer.
    pub fn wait(
        &self,
        key: &str,
        opts: &WaitOptions,
        mut on_poll: impl FnMut(Duration),
    ) -> Result<(), Error> {
        let start = Instant::now();
        loop {
            if self.status(key)? == Status::Done {
                return Ok(());
            }
            let waited = start.elapsed();
            on_poll(waited);
            if waited >= opts.timeout {
                return Err(Error::Timeout {
                    key: key.to_string(),
                    waited,
                });
            }
            std::thread::sleep(opts.interval.min(opts.timeout - waited));
        }
    }

    /// The results archive (zip) of a finished submission.
    pub fn download(&self, key: &str) -> Result<Vec<u8>, Error> {
        let url = format!("{}/results/file?id={key}", self.base_url);
        let reply = match self.agent.get(&url).call() {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                return Err(Error::Http {
                    url,
                    reason: format!("HTTP {code}: {}", text.trim()),
                });
            }
            Err(e) => return Err(http_error(&url, e)),
        };
        let mut bytes = Vec::new();
        reply
            .into_reader()
            .take(MAX_ARCHIVE_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::Http {
                url: url.clone(),
                reason: e.to_string(),
            })?;
        if !bytes.starts_with(b"PK") {
            return Err(Error::Results {
                key: key.to_string(),
                reason: format!(
                    "the service sent no zip archive: {}",
                    String::from_utf8_lossy(&bytes[..bytes.len().min(2000)]).trim()
                ),
            });
        }
        Ok(bytes)
    }
}

/// Proxy for `url` from the usual variables (`HTTPS_PROXY` or `HTTP_PROXY` by scheme, then
/// `ALL_PROXY`, upper or lower case), unless its host matches `NO_PROXY`. ureq 2 reads these
/// variables without the scheme or `NO_PROXY`, hence this.
pub fn proxy_for(url: &str, env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let var = |name: &str| env(name).or_else(|| env(&name.to_ascii_lowercase()));
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    }
    .to_ascii_lowercase();
    if let Some(list) = var("NO_PROXY") {
        let bypass = list.split(',').map(|e| e.trim().to_ascii_lowercase()).any(|e| {
            let e = e.trim_start_matches("*.").trim_start_matches('.');
            !e.is_empty() && (e == "*" || host == e || host.ends_with(&format!(".{e}")))
        });
        if bypass {
            return None;
        }
    }
    let by_scheme = if scheme.eq_ignore_ascii_case("https") {
        "HTTPS_PROXY"
    } else {
        "HTTP_PROXY"
    };
    var(by_scheme).or_else(|| var("ALL_PROXY"))
}

/// Status and text of a reply, error statuses included.
fn text_reply(url: &str, reply: Result<ureq::Response, ureq::Error>) -> Result<(u16, String), Error> {
    let r = match reply {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => return Err(http_error(url, e)),
    };
    let status = r.status();
    let text = r.into_string().map_err(|e| Error::Http {
        url: url.to_string(),
        reason: e.to_string(),
    })?;
    Ok((status, text))
}

fn http_error(url: &str, e: ureq::Error) -> Error {
    Error::Http {
        url: url.to_string(),
        reason: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(email: &str, system: RefSystem) -> Request<'_> {
        Request {
            rinex: Path::new("BASE2070.24O"),
            email,
            process_type: ProcessType::Static,
            system,
        }
    }

    #[test]
    fn form_has_the_script_fields() {
        let f = form_fields(&req(" lab@example.org ", RefSystem::Itrf)).unwrap();
        let get = |k: &str| f.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("return_email"), Some("lab@example.org"));
        assert_eq!(get("user_name"), Some("lab@example.org"));
        assert_eq!(get("cmd_process_type"), Some("std"));
        assert_eq!(get("ppp_access"), Some("nobrowser_status"));
        assert_eq!(get("language"), Some("en"));
        assert_eq!(get("process_type"), Some("Static"));
        assert_eq!(get("sysref"), Some("ITRF"));
        assert_eq!(get("nad83_epoch"), Some("CURR"));
        assert_eq!(get("v_datum"), Some("cgvd2013"));
        assert_eq!(get("output_pdf"), Some("lite"));
        let n = form_fields(&req("a@b.c", RefSystem::Nad83)).unwrap();
        assert!(n.contains(&("sysref", "NAD83".into())));
    }

    #[test]
    fn email_cannot_break_the_form() {
        for bad in ["", "lab", "a@b\r\nX: y", "a b@c.d", "a\"@b.c"] {
            assert!(form_fields(&req(bad, RefSystem::Itrf)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn multipart_body_layout() {
        let fields = vec![("sysref", "ITRF".to_string())];
        let (ct, body) = multipart(&fields, "BA\"SE.24O", b"RINEX DATA");
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert_eq!(
            text,
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"sysref\"\r\n\r\nITRF\r\n\
                 --{boundary}\r\nContent-Disposition: form-data; name=\"rfile_upload\"; \
                 filename=\"BA_SE.24O\"\r\nContent-Type: application/octet-stream\r\n\r\n\
                 RINEX DATA\r\n--{boundary}--\r\n"
            )
        );
    }

    #[test]
    fn boundary_avoids_the_file() {
        let (ct, _) = multipart(&[], "x", b"");
        let first = ct.rsplit('=').next().unwrap().to_string();
        let data = format!("....{first}....");
        let (ct2, _) = multipart(&[], "x", data.as_bytes());
        assert!(!data.contains(ct2.rsplit('=').next().unwrap()));
    }

    #[test]
    fn key_or_rejection() {
        let key = "3Ny0To_m6Wmjr28aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaacps3FG";
        assert_eq!(tracking_key(200, &format!("{key}\n")).unwrap(), key);
        for (status, body) in [
            (200, "<html><body>Error</body></html>"),
            (200, "Invalid RINEX file"),
            (200, ""),
            (500, key),
        ] {
            match tracking_key(status, body) {
                Err(Error::Rejected { status: s, body: b }) => {
                    assert_eq!(s, status);
                    assert_eq!(b, body.trim());
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn proxy_by_scheme_and_no_proxy() {
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        let env = vars(&[
            ("HTTPS_PROXY", "http://proxy:3128"),
            ("no_proxy", "localhost,127.0.0.1,.example.org"),
        ]);
        assert_eq!(proxy_for(BASE_URL, env).as_deref(), Some("http://proxy:3128"));
        assert_eq!(proxy_for("http://127.0.0.1:8080/x", env), None);
        assert_eq!(proxy_for("https://a.example.org/x", env), None);
        assert_eq!(proxy_for("http://other:80/x", env), None);
        let all = vars(&[("all_proxy", "socks5://p:1080")]);
        assert_eq!(proxy_for("http://h/x", all).as_deref(), Some("socks5://p:1080"));
        assert_eq!(proxy_for(BASE_URL, vars(&[])), None);
    }

    #[test]
    fn status_words() {
        assert_eq!(parse_status("k", "processing\n").unwrap(), Status::Processing);
        assert_eq!(parse_status("k", "done").unwrap(), Status::Done);
        let e = parse_status("k", "error").unwrap_err();
        assert!(
            matches!(e, Error::Status { ref text, .. } if text == "error"),
            "{e}"
        );
    }
}
