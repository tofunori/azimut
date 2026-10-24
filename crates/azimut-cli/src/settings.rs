//! Azimut's user settings: a small JSON file holding the e-mail of the CSRS-PPP account, so
//! that it is asked only once.
//!
//! The file is `config.json` in `AZIMUT_CONFIG_DIR` when set, else in the platform's
//! configuration folder: `%APPDATA%\azimut` on Windows, `~/Library/Application Support/azimut`
//! on macOS, `$XDG_CONFIG_HOME/azimut` or `~/.config/azimut` elsewhere.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Environment variable naming the settings folder (tests, portable installs).
pub const ENV_DIR: &str = "AZIMUT_CONFIG_DIR";

const FILE_NAME: &str = "config.json";
const EMAIL_KEY: &str = "csrs_email";

/// The settings file, or `None` when no configuration folder can be found.
pub fn path() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let dir = if let Some(d) = env(ENV_DIR) {
        d
    } else if cfg!(windows) {
        env("APPDATA")?.join("azimut")
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support/azimut")
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))?
            .join("azimut")
    };
    Some(dir.join(FILE_NAME))
}

fn load() -> Value {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// The remembered CSRS-PPP e-mail, if any.
pub fn csrs_email() -> Option<String> {
    load()
        .get(EMAIL_KEY)
        .and_then(Value::as_str)
        .filter(|e| valid_email(e))
        .map(String::from)
}

/// Remember the CSRS-PPP e-mail, keeping the file's other entries. Returns the file path.
pub fn save_csrs_email(email: &str) -> Result<PathBuf> {
    let path = path().context("no configuration folder found (set AZIMUT_CONFIG_DIR)")?;
    let mut v = load();
    v[EMAIL_KEY] = json!(email);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&v)? + "\n";
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    // The e-mail is personal: readable by its owner only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restricting {}", path.display()))?;
    }
    Ok(path)
}

/// A plausible e-mail address: one `@` with text on both sides and a dot in the domain, no
/// spaces. CSRS-PPP checks the account itself.
pub fn valid_email(e: &str) -> bool {
    match e.split_once('@') {
        Some((user, domain)) => {
            !user.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !domain.contains('@')
                && !e.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_check() {
        assert!(valid_email("lab@example.org"));
        for bad in [
            "",
            "lab",
            "lab@",
            "@example.org",
            "lab@example",
            "la b@example.org",
            "a@b@c.org",
            "lab@.org",
        ] {
            assert!(!valid_email(bad), "{bad}");
        }
    }
}
