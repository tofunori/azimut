//! The IGS antenna file igs20.atx, kept in Azimut's cache so that base antenna models are
//! available without a manual download. It is fetched from files.igs.org the first time an
//! antenna not embedded in Azimut needs it (see `azimut_solve::antex`), then reused offline.
//!
//! The file is `igs20.atx` in `AZIMUT_CACHE_DIR` when set, else in the platform's cache
//! folder: `%LOCALAPPDATA%\azimut` on Windows, `~/Library/Caches/azimut` on macOS,
//! `$XDG_CACHE_HOME/azimut` or `~/.cache/azimut` elsewhere.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use azimut_solve::antex;

/// Environment variable naming the cache folder (tests, portable installs).
pub const ENV_DIR: &str = "AZIMUT_CACHE_DIR";
/// Environment variable replacing the download URL (tests, mirrors).
pub const ENV_URL: &str = "AZIMUT_ANTEX_URL";
/// The IGS file, always the latest version of igs20.atx.
pub const URL: &str = "https://files.igs.org/pub/station/general/igs20.atx";

const FILE_NAME: &str = "igs20.atx";

/// The cached file, or `None` when no cache folder can be found.
pub fn path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = if let Some(d) = env(ENV_DIR) {
        d
    } else if cfg!(windows) {
        env("LOCALAPPDATA")?.join("azimut")
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Caches/azimut")
    } else {
        env("XDG_CACHE_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".cache")))?
            .join("azimut")
    };
    Some(dir.join(FILE_NAME))
}

/// The download URL: `AZIMUT_ANTEX_URL` when set, else [`URL`].
pub fn url() -> String {
    std::env::var(ENV_URL)
        .ok()
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| URL.to_string())
}

/// The cached file's text when it is a complete ANTEX file.
pub fn read_cached(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().filter(|t| antex::is_antex(t))
}

/// Download igs20.atx into `path`, replacing the cached copy only once the new one is
/// complete. Returns its text.
pub fn download(path: &Path) -> Result<String> {
    let url = url();
    let dir = path.parent().context("cache file without a folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut builder = ureq::AgentBuilder::new()
        .user_agent(concat!("azimut/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(300));
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    if let Some(p) = azimut_csrs::proxy_for(&url, env) {
        builder = builder.proxy(ureq::Proxy::new(&p).with_context(|| format!("proxy {p}"))?);
    }
    let reply = builder.build().get(&url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => anyhow::anyhow!("{url}: HTTP {code}"),
        // ureq names the URL itself.
        e => anyhow::anyhow!("{e}"),
    })?;
    let part = path.with_extension("atx.part");
    let mut file = std::fs::File::create(&part).with_context(|| format!("creating {}", part.display()))?;
    std::io::copy(&mut reply.into_reader(), &mut file)
        .and_then(|_| file.flush())
        .map_err(|e| {
            let _ = std::fs::remove_file(&part);
            anyhow::anyhow!("{url}: {e}")
        })?;
    drop(file);
    let text = std::fs::read_to_string(&part).unwrap_or_default();
    if !antex::is_antex(&text) {
        let _ = std::fs::remove_file(&part);
        bail!("{url} did not send a complete ANTEX file");
    }
    std::fs::rename(&part, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(text)
}

/// Where the model of an antenna comes from, for `azimut antex` and `process`.
pub enum Source {
    /// The cached file already held it.
    Cached(PathBuf),
    /// The file was downloaded (first use, or the cached copy lacked the antenna).
    Downloaded(PathBuf),
}

impl Source {
    pub fn path(&self) -> &Path {
        match self {
            Source::Cached(p) | Source::Downloaded(p) => p,
        }
    }
}

/// igs20.atx holding `antenna`: the cached copy, else a fresh download (the IGS adds
/// antennas to the file, so a cached copy without the antenna is refreshed once). Offline
/// with no usable copy, the error says how to give the file by hand.
pub fn for_antenna(antenna: &str) -> Result<Source> {
    let path = path().context(
        "no cache folder found for igs20.atx (set AZIMUT_CACHE_DIR, or give the file with --antex)",
    )?;
    let cached = read_cached(&path);
    if cached.as_deref().is_some_and(|t| antex::holds(t, antenna)) {
        return Ok(Source::Cached(path));
    }
    println!(
        "   downloading igs20.atx (IGS antenna models, about 60 MB) from {} into {}",
        url(),
        path.display()
    );
    let text = match download(&path) {
        Ok(text) => text,
        Err(e) => {
            let fallback = match cached {
                Some(_) => format!("the cached copy {} has no model for {antenna}", path.display()),
                None => "no copy is cached yet".to_string(),
            };
            bail!(
                "the base antenna {antenna} needs its phase-centre model, which Azimut does not \
                 embed, and igs20.atx could not be downloaded ({e:#}); {fallback}. Download \
                 {URL} on a connected computer and give it with --antex, or copy it to {}",
                path.display()
            );
        }
    };
    if !antex::holds(&text, antenna) {
        bail!(
            "igs20.atx ({}) has no model for {antenna}: check the antenna type in the CSRS-PPP \
             summary and the RINEX header, or give a file that holds it with --antex",
            path.display()
        );
    }
    Ok(Source::Downloaded(path))
}
