//! Receiver antenna models embedded in Azimut: the Emlid Reach RS2, RS2+ and RS3 entries of
//! the IGS file igs20.atx, unchanged (see SOURCES.md, ANTEX). With them, a base on one of
//! these antennas positioned by CSRS-PPP needs no `--antex`.

use std::io;
use std::path::{Path, PathBuf};

/// The embedded ANTEX file: receiver antennas only, no satellite entries.
pub const EMBEDDED: &str = include_str!("../data/emlid_igs20.atx");

/// File name used when the embedded models are written for RTKLIB.
pub const FILE_NAME: &str = "emlid_igs20.atx";

/// Antenna types with radome in an ANTEX text, e.g. `"EML_REACH_RS2   NONE"`. Satellite
/// entries are included (their type is the block, e.g. `"BLOCK IIF"`).
pub fn types(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .filter(|l| {
            l.get(60..)
                .is_some_and(|label| label.starts_with("TYPE / SERIAL NO"))
        })
        .map(|l| l.get(..20).unwrap_or(l).trim_end())
}

/// True when an ANTEX text holds this antenna type with radome. Spacing is ignored:
/// CSRS-PPP and RINEX headers do not always pad the radome to column 17.
pub fn holds(text: &str, antenna: &str) -> bool {
    let key = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let wanted = key(antenna);
    types(text).any(|t| key(t) == wanted)
}

/// True when the text looks like a complete ANTEX file: the version line first and at least
/// one antenna, the last one closed.
pub fn is_antex(text: &str) -> bool {
    let first = text.lines().next().unwrap_or_default();
    first
        .get(60..)
        .is_some_and(|label| label.starts_with("ANTEX VERSION / SYST"))
        && text.contains("START OF ANTENNA")
        && text.trim_end().ends_with("END OF ANTENNA")
}

/// Antenna types with radome held in [`EMBEDDED`], e.g. `"EML_REACH_RS2   NONE"`.
pub fn embedded_types() -> impl Iterator<Item = &'static str> {
    types(EMBEDDED)
}

/// True when the embedded models hold this antenna type with radome (see [`holds`]).
pub fn is_embedded(antenna: &str) -> bool {
    holds(EMBEDDED, antenna)
}

/// Write the embedded models into `dir` for RTKLIB and return the file path.
pub fn write_embedded(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE_NAME);
    std::fs::write(&path, EMBEDDED)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_the_three_emlid_antennas() {
        let types: Vec<_> = embedded_types().collect();
        assert_eq!(
            types,
            [
                "EML_REACH_RS2   NONE",
                "EML_REACH_RS2+  NONE",
                "EML_REACH_RS3   NONE"
            ]
        );
        assert_eq!(EMBEDDED.matches("START OF ANTENNA").count(), 3);
        assert_eq!(EMBEDDED.matches("END OF ANTENNA").count(), 3);
    }

    #[test]
    fn matching_ignores_spacing() {
        assert!(is_embedded("EML_REACH_RS2   NONE"));
        assert!(is_embedded("EML_REACH_RS2 NONE"));
        assert!(is_embedded("EML_REACH_RS2+  NONE"));
        assert!(!is_embedded("EML_REACH_RS4   NONE"));
        assert!(!is_embedded("TRM59800.00     NONE"));
    }

    #[test]
    fn embedded_file_is_complete() {
        assert!(is_antex(EMBEDDED));
        assert!(!is_antex(&EMBEDDED[..EMBEDDED.len() / 2]));
        assert!(!is_antex("<html>Not found</html>"));
    }
}
