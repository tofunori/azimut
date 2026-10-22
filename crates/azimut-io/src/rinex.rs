//! Fast RINEX summary: header, first and last epoch, without reading the whole file.
//!
//! Reads RINEX 2.x, 3.x and 4.x observation headers, the first epochs after the header and
//! the last epoch from the end of the file. Compact RINEX (Hatanaka) headers are read too;
//! their epochs come from `TIME OF FIRST/LAST OBS` when present. Gzip or Unix-compressed
//! files are recognised but not opened. See SOURCES.md (RINEX).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::headers::CountingReader;
use crate::time::seconds_since_gps_epoch;

/// Largest header accepted, bytes (guards against reading a non-RINEX file to the end).
const MAX_HEADER: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RinexKind {
    Observation,
    Navigation,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    None,
    /// Compact RINEX (Hatanaka): header readable, epochs encoded.
    Hatanaka,
    /// gzip or Unix compress: not opened.
    Archive,
}

/// Where the first/last epoch times come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpochSource {
    /// Epoch lines in the data.
    Data,
    /// `TIME OF FIRST OBS` / `TIME OF LAST OBS` header records.
    Header,
    Unknown,
}

/// What Azimut needs to know about one RINEX file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RinexSummary {
    pub path: PathBuf,
    pub size: u64,
    pub bytes_read: u64,
    pub compression: Compression,
    pub version: Option<f64>,
    pub kind: Option<RinexKind>,
    pub marker: Option<String>,
    pub receiver: Option<String>,
    pub antenna_type: Option<String>,
    /// Antenna height, east, north offsets from the marker, metres.
    pub antenna_delta_hen_m: Option<[f64; 3]>,
    /// Approximate marker position, ECEF metres.
    pub approx_xyz_m: Option<[f64; 3]>,
    /// Time system of the epochs (`GPS` when absent, as RINEX specifies for GPS files).
    pub time_system: Option<String>,
    /// Recording interval, seconds (header, or estimated from the first epochs).
    pub interval_s: Option<f64>,
    /// First and last epoch, seconds since the GPS epoch, read as written.
    pub first_epoch_s: Option<f64>,
    pub last_epoch_s: Option<f64>,
    pub epochs_from: EpochSource,
    /// Frequency band digits per system letter, e.g. `G -> ["1", "2"]`.
    pub bands: BTreeMap<char, Vec<String>>,
    pub error: Option<String>,
}

impl RinexSummary {
    pub fn duration_s(&self) -> Option<f64> {
        Some(self.last_epoch_s? - self.first_epoch_s?)
    }

    /// True when every system carries only one frequency band.
    pub fn single_frequency(&self) -> bool {
        !self.bands.is_empty() && self.bands.values().all(|b| b.len() <= 1)
    }

    /// Bands as a short text, e.g. `G 1/2, R 1/2, E 1/5`.
    pub fn bands_label(&self) -> String {
        self.bands
            .iter()
            .map(|(s, b)| format!("{s} {}", b.join("/")))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn empty(path: &Path, size: u64) -> Self {
        RinexSummary {
            path: path.to_path_buf(),
            size,
            bytes_read: 0,
            compression: Compression::None,
            version: None,
            kind: None,
            marker: None,
            receiver: None,
            antenna_type: None,
            antenna_delta_hen_m: None,
            approx_xyz_m: None,
            time_system: None,
            interval_s: None,
            first_epoch_s: None,
            last_epoch_s: None,
            epochs_from: EpochSource::Unknown,
            bands: BTreeMap::new(),
            error: None,
        }
    }
}

/// Lower-case extension after removing `.gz`, `.z` or `.zip`, and whether one was removed.
pub fn inner_extension(path: &Path) -> (Option<String>, bool) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let mut parts: Vec<&str> = name.rsplitn(3, '.').collect();
    let archived = matches!(parts.first(), Some(&("gz" | "z" | "zip")));
    if archived {
        parts.remove(0);
    }
    (
        parts
            .first()
            .filter(|_| name.contains('.'))
            .map(|s| s.to_string()),
        archived,
    )
}

/// True for RINEX observation file names: `.obs`, `.26o`, `.26d` (Hatanaka), `.crx`,
/// `_MO.rnx` (RINEX 3 long names), also when gzipped.
pub fn is_obs_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let (ext, _) = inner_extension(path);
    match ext.as_deref() {
        Some("obs" | "crx") => true,
        Some("rnx") => name.contains("_mo."),
        Some(e) if e.len() == 3 && e[..2].chars().all(|c| c.is_ascii_digit()) => {
            matches!(&e[2..], "o" | "d")
        }
        _ => false,
    }
}

/// True for RINEX navigation file names: `.nav`, `.26n/g/l/p/c`, `_MN.rnx`.
pub fn is_nav_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let (ext, _) = inner_extension(path);
    match ext.as_deref() {
        Some("nav") => true,
        Some("rnx") => name.contains("_mn.") || name.contains("_gn.") || name.contains("_rn."),
        Some(e) if e.len() == 3 && e[..2].chars().all(|c| c.is_ascii_digit()) => {
            matches!(&e[2..], "n" | "g" | "l" | "p" | "c" | "h" | "q")
        }
        _ => false,
    }
}

fn v3_epoch_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^>\s*(\d{4})\s+(\d{1,2})\s+(\d{1,2})\s+(\d{1,2})\s+(\d{1,2})\s+(\d{1,2}\.\d+)\s+(\d)")
            .expect("valid regex")
    })
}

fn v2_epoch_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // RINEX 2 epoch line: " yy mm dd hh mm ss.sssssss  f nnn" followed by satellites.
    RE.get_or_init(|| {
        Regex::new(r"^ (\d{2}) ([ \d]\d) ([ \d]\d) ([ \d]\d) ([ \d]\d) ([ \d]\d\.\d{7})  (\d)([ \d]{3})((?:[GRESJCI ]\d{2})*)")
            .expect("valid regex")
    })
}

fn to_gps_seconds(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: f64) -> Option<f64> {
    let whole = s.floor();
    let base = NaiveDate::from_ymd_opt(y, mo, d)?.and_hms_opt(h, mi, whole as u32)?;
    Some(seconds_since_gps_epoch(base) + (s - whole))
}

fn year_from_two_digits(yy: i32) -> i32 {
    if yy < 80 {
        2000 + yy
    } else {
        1900 + yy
    }
}

/// Epoch time from one data line, when it is an epoch header line. Returns the time and,
/// for RINEX 2, satellite system letters listed on the line.
fn epoch_line(line: &str, version: f64) -> Option<(f64, Vec<char>)> {
    if version >= 3.0 {
        let c = v3_epoch_re().captures(line)?;
        let t = to_gps_seconds(
            c[1].parse().ok()?,
            c[2].parse().ok()?,
            c[3].parse().ok()?,
            c[4].parse().ok()?,
            c[5].parse().ok()?,
            c[6].parse().ok()?,
        )?;
        Some((t, vec![]))
    } else {
        let c = v2_epoch_re().captures(line)?;
        let f = |i: usize| c[i].trim().parse::<u32>().ok();
        let t = to_gps_seconds(
            year_from_two_digits(c[1].trim().parse().ok()?),
            f(2)?,
            f(3)?,
            f(4)?,
            f(5)?,
            c[6].trim().parse().ok()?,
        )?;
        let sats: Vec<char> = c
            .get(9)
            .map(|m| {
                m.as_str()
                    .as_bytes()
                    .chunks(3)
                    .map(|ch| if ch[0] == b' ' { 'G' } else { ch[0] as char })
                    .collect()
            })
            .unwrap_or_default();
        Some((t, sats))
    }
}

fn label(line: &str) -> &str {
    line.get(60..).unwrap_or("").trim()
}

fn content(line: &str) -> &str {
    line.get(..60.min(line.len())).unwrap_or(line)
}

fn floats(s: &str) -> Vec<f64> {
    s.split_whitespace()
        .filter_map(|t| t.replace(['D', 'd'], "E").parse().ok())
        .collect()
}

fn header_time(line: &str) -> Option<(f64, Option<String>)> {
    let c = content(line);
    let p: Vec<&str> = c.split_whitespace().collect();
    if p.len() < 6 {
        return None;
    }
    let t = to_gps_seconds(
        p[0].parse().ok()?,
        p[1].parse().ok()?,
        p[2].parse().ok()?,
        p[3].parse().ok()?,
        p[4].parse().ok()?,
        p[5].parse().ok()?,
    )?;
    Some((t, p.get(6).map(|s| s.to_string())))
}

/// Summarise one RINEX file. Never fails: problems are recorded in `error`.
pub fn summarize(path: &Path) -> RinexSummary {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut s = RinexSummary::empty(path, size);
    if inner_extension(path).1 {
        s.compression = Compression::Archive;
        s.error = Some("compressed archive (.gz/.Z): decompress it to let Azimut check it".into());
        return s;
    }
    if let Err(e) = read_into(path, &mut s) {
        s.error = Some(e);
    }
    s
}

fn read_into(path: &Path, s: &mut RinexSummary) -> Result<(), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut r = BufReader::with_capacity(16 * 1024, CountingReader::new(file));
    let mut line = String::new();
    let mut header_bytes = 0u64;
    let mut systems_v2: BTreeSet<char> = BTreeSet::new();
    let mut obs_codes: BTreeMap<char, BTreeSet<String>> = BTreeMap::new();
    let mut current_sys: Option<char> = None;
    let mut v2_types: BTreeSet<String> = BTreeSet::new();
    let mut header_first = None;
    let mut header_last = None;
    let mut ended = false;

    loop {
        line.clear();
        let n = r.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        header_bytes += n as u64;
        if header_bytes > MAX_HEADER {
            return Err("no END OF HEADER found: not a RINEX file?".into());
        }
        let l = line.trim_end_matches(['\r', '\n']);
        let lab = label(l);
        let c = content(l);
        match lab {
            "CRINEX VERS   / TYPE" => s.compression = Compression::Hatanaka,
            "RINEX VERSION / TYPE" => {
                s.version = c.get(..9).and_then(|v| v.trim().parse().ok());
                s.kind = Some(match c.get(20..21).unwrap_or(" ") {
                    "O" => RinexKind::Observation,
                    "N" | "G" | "H" | "L" | "C" => RinexKind::Navigation,
                    _ => RinexKind::Other,
                });
                if let Some(sys) = c.get(40..41).and_then(|x| x.chars().next()) {
                    if sys.is_ascii_alphabetic() && sys != 'M' {
                        systems_v2.insert(sys);
                    } else if sys == ' ' {
                        systems_v2.insert('G');
                    }
                }
            }
            "MARKER NAME" => s.marker = Some(c.trim().to_string()).filter(|x| !x.is_empty()),
            "REC # / TYPE / VERS" => {
                s.receiver = c
                    .get(20..40)
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
            }
            "ANT # / TYPE" => {
                s.antenna_type = c
                    .get(20..40)
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
            }
            "ANTENNA: DELTA H/E/N" => {
                let v = floats(c);
                if v.len() >= 3 {
                    s.antenna_delta_hen_m = Some([v[0], v[1], v[2]]);
                }
            }
            "APPROX POSITION XYZ" => {
                let v = floats(c);
                if v.len() >= 3 && (v[0] != 0.0 || v[1] != 0.0 || v[2] != 0.0) {
                    s.approx_xyz_m = Some([v[0], v[1], v[2]]);
                }
            }
            "INTERVAL" => s.interval_s = floats(c).first().copied().filter(|x| *x > 0.0),
            "TIME OF FIRST OBS" => {
                if let Some((t, sys)) = header_time(l) {
                    header_first = Some(t);
                    s.time_system = sys.or_else(|| s.time_system.clone());
                }
            }
            "TIME OF LAST OBS" => header_last = header_time(l).map(|x| x.0),
            "SYS / # / OBS TYPES" => {
                let first = c.chars().next().unwrap_or(' ');
                if first != ' ' {
                    current_sys = Some(first);
                }
                if let Some(sys) = current_sys {
                    let codes = c.get(7..).unwrap_or("");
                    let set = obs_codes.entry(sys).or_default();
                    for code in codes.split_whitespace() {
                        set.insert(code.to_string());
                    }
                }
            }
            "# / TYPES OF OBSERV" => {
                let rest = c.get(6..).unwrap_or("");
                for t in rest.split_whitespace() {
                    v2_types.insert(t.to_string());
                }
            }
            "END OF HEADER" => {
                ended = true;
                break;
            }
            _ => {}
        }
    }
    if !ended {
        return Err("no END OF HEADER found: not a RINEX file?".into());
    }
    let version = s.version.ok_or("no RINEX VERSION / TYPE record")?;

    // Frequency bands per system.
    if version >= 3.0 {
        for (sys, codes) in &obs_codes {
            let bands: BTreeSet<String> = codes
                .iter()
                .filter_map(|c| c.get(1..2).map(str::to_string))
                .collect();
            s.bands.insert(*sys, bands.into_iter().collect());
        }
    }

    // First epochs, when the data is plain RINEX observation data.
    let mut first_epochs: Vec<f64> = Vec::new();
    if s.compression == Compression::None && s.kind == Some(RinexKind::Observation) {
        let mut scanned = 0u64;
        while first_epochs.len() < 3 && scanned < 512 * 1024 {
            line.clear();
            let n = r.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            scanned += n as u64;
            if let Some((t, sats)) = epoch_line(line.trim_end_matches(['\r', '\n']), version) {
                first_epochs.push(t);
                systems_v2.extend(sats);
            }
        }
    }
    if version < 3.0 {
        let bands: BTreeSet<String> = v2_types
            .iter()
            .filter_map(|t| t.get(1..2).map(str::to_string))
            .collect();
        if !bands.is_empty() {
            for sys in &systems_v2 {
                s.bands.insert(*sys, bands.iter().cloned().collect());
            }
        }
    }

    // Last epoch from the end of the file.
    let mut last = None;
    if !first_epochs.is_empty() {
        let len = s.size;
        let mut tail = 64 * 1024u64;
        while last.is_none() {
            let start = len.saturating_sub(tail);
            r.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
            let mut buf = Vec::with_capacity((len - start) as usize);
            r.by_ref()
                .take(len - start)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&buf);
            last = text
                .lines()
                .rev()
                .find_map(|l| epoch_line(l, version).map(|x| x.0));
            if start == 0 || tail >= 4 * 1024 * 1024 {
                break;
            }
            tail *= 4;
        }
    }

    if let Some(&f) = first_epochs.first() {
        s.first_epoch_s = Some(f);
        s.last_epoch_s = last.or(Some(f));
        s.epochs_from = EpochSource::Data;
        if s.interval_s.is_none() && first_epochs.len() >= 2 {
            let d: Vec<f64> = first_epochs
                .windows(2)
                .map(|w| w[1] - w[0])
                .filter(|d| *d > 0.0)
                .collect();
            s.interval_s = d
                .into_iter()
                .reduce(f64::min)
                .map(|x| (x * 1000.0).round() / 1000.0);
        }
    } else if let Some(f) = header_first {
        s.first_epoch_s = Some(f);
        s.last_epoch_s = header_last;
        s.epochs_from = EpochSource::Header;
    }
    s.bytes_read = r.get_ref().bytes_read();
    Ok(())
}

/// WGS84 geodetic to ECEF, metres.
pub fn geodetic_to_ecef(lat_deg: f64, lon_deg: f64, h_m: f64) -> [f64; 3] {
    const A: f64 = 6_378_137.0;
    const F: f64 = 1.0 / 298.257_223_563;
    let e2 = F * (2.0 - F);
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let n = A / (1.0 - e2 * lat.sin().powi(2)).sqrt();
    [
        (n + h_m) * lat.cos() * lon.cos(),
        (n + h_m) * lat.cos() * lon.sin(),
        (n * (1.0 - e2) + h_m) * lat.sin(),
    ]
}

pub fn distance_m(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(is_obs_name(Path::new("BASE1660.26o")));
        assert!(is_obs_name(Path::new("base.26D.gz")));
        assert!(is_obs_name(Path::new(
            "TROI00CAN_R_20261660000_01D_01S_MO.crx.gz"
        )));
        assert!(is_obs_name(Path::new("X_R_20261660000_01H_01S_MO.rnx")));
        assert!(is_obs_name(Path::new("DJI_20260615_PPKOBS.obs")));
        assert!(!is_obs_name(Path::new("X_R_20261660000_01H_MN.rnx")));
        assert!(is_nav_name(Path::new("X_R_20261660000_01H_MN.rnx")));
        assert!(is_nav_name(Path::new("brdc1660.26n")));
        assert!(is_nav_name(Path::new("DJI_PPKNAV.nav")));
        assert!(!is_nav_name(Path::new("photo.jpg")));
    }

    #[test]
    fn two_digit_years() {
        assert_eq!(year_from_two_digits(26), 2026);
        assert_eq!(year_from_two_digits(80), 1980);
    }

    #[test]
    fn ecef_of_equator() {
        let p = geodetic_to_ecef(0.0, 0.0, 0.0);
        assert!((p[0] - 6_378_137.0).abs() < 1e-6 && p[1].abs() < 1e-6 && p[2].abs() < 1e-6);
    }
}
