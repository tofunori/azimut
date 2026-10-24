//! Receiver antenna models embedded in Azimut: the Emlid Reach RS2, RS2+ and RS3 entries of
//! the IGS file igs20.atx, unchanged (see SOURCES.md, ANTEX). With them, a base on one of
//! these antennas positioned by CSRS-PPP needs no `--antex`.

use std::io;
use std::path::{Path, PathBuf};

/// The embedded ANTEX file: receiver antennas only, no satellite entries.
pub const EMBEDDED: &str = include_str!("../data/emlid_igs20.atx");

/// File name used when the embedded models are written for RTKLIB.
pub const FILE_NAME: &str = "emlid_igs20.atx";

/// Antenna types with radome held in [`EMBEDDED`], e.g. `"EML_REACH_RS2   NONE"`.
pub fn embedded_types() -> impl Iterator<Item = &'static str> {
    EMBEDDED
        .lines()
        .filter(|l| {
            l.get(60..)
                .is_some_and(|label| label.starts_with("TYPE / SERIAL NO"))
        })
        .map(|l| l[..20].trim_end())
}

/// True when the embedded models hold this antenna type with radome. Spacing is ignored:
/// CSRS-PPP and RINEX headers do not always pad the radome to column 17.
pub fn is_embedded(antenna: &str) -> bool {
    let key = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let wanted = key(antenna);
    embedded_types().any(|t| key(t) == wanted)
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
}
