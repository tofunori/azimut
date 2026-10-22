//! Reader for DJI `*.MRK` timestamp files.
//!
//! One line per exposure. DJI manuals describe 13 fields; real files tag most values with a
//! suffix (`,N`, `,E`, `,V`, `,Lat`, `,Lon`, `,Ellh`, `,Q`) and put latitude before
//! longitude, against the manual's numbering. This parser therefore reads values by their
//! suffix, never by column position. See SOURCES.md (MRK).

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Seconds in one GPS week.
pub const SECONDS_PER_WEEK: f64 = 604_800.0;

/// Solution status written in the `,Q` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtkQuality {
    /// 50
    Fixed,
    /// 32 to 49 (34 is the usual value)
    Float,
    /// 16
    Single,
    /// 0
    NoPosition,
    /// Any other code
    Unknown,
}

impl RtkQuality {
    /// Map DJI's numeric flag. DJI documents float as the range 32 to 49, not only 34.
    pub fn from_flag(flag: u16) -> Self {
        match flag {
            50 => RtkQuality::Fixed,
            32..=49 => RtkQuality::Float,
            16 => RtkQuality::Single,
            0 => RtkQuality::NoPosition,
            _ => RtkQuality::Unknown,
        }
    }
}

/// One exposure record from an MRK file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MrkRecord {
    /// Photo series number (field 1).
    pub index: u32,
    /// GPS time of week, seconds.
    pub tow: f64,
    /// GPS week number.
    pub week: u32,
    /// Antenna phase centre to camera (CMOS) centre, millimetres, North-East-Down.
    /// The third value is positive downward.
    pub offset_ned_mm: [f64; 3],
    /// Real-time latitude of the camera centre, degrees (replaced by PPK later).
    pub lat_deg: f64,
    /// Real-time longitude of the camera centre, degrees.
    pub lon_deg: f64,
    /// Real-time ellipsoidal height of the camera centre, metres.
    pub ellh_m: f64,
    /// Standard deviations North, East, Up in metres, as written by the aircraft.
    pub std_neu_m: Option<[f64; 3]>,
    /// Raw status flag.
    pub flag: u16,
    /// Decoded status.
    pub quality: RtkQuality,
    /// 1-based line number in the file.
    pub line: usize,
}

impl MrkRecord {
    /// Exposure time as seconds since the GPS epoch (1980-01-06 00:00:00 GPS time).
    pub fn gps_seconds(&self) -> f64 {
        self.week as f64 * SECONDS_PER_WEEK + self.tow
    }
}

/// A line that could not be read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MrkLineError {
    pub line: usize,
    pub reason: String,
}

/// Content of one MRK file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MrkFile {
    pub records: Vec<MrkRecord>,
    pub errors: Vec<MrkLineError>,
}

fn head_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // index, TOW, [week]
    RE.get_or_init(|| {
        Regex::new(r"^\s*(\d+)[\s|]+(\d+(?:\.\d+)?)[\s|]+\[?\s*(\d+)\s*\]?").expect("valid regex")
    })
}

fn tagged_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)([+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\s*,\s*(lat|lon|ellh|n|e|v|q)\b")
            .expect("valid regex")
    })
}

fn number_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?").expect("valid regex"))
}

/// Parse one non-empty MRK line.
pub fn parse_line(text: &str, line: usize) -> Result<MrkRecord, String> {
    let head = head_re()
        .captures(text)
        .ok_or_else(|| "missing index, time of week or [week]".to_string())?;
    let index: u32 = head[1].parse().map_err(|_| "bad index".to_string())?;
    let tow: f64 = head[2].parse().map_err(|_| "bad time of week".to_string())?;
    let week: u32 = head[3].parse().map_err(|_| "bad GPS week".to_string())?;
    let rest_start = head.get(0).map(|m| m.end()).unwrap_or(0);
    let rest = &text[rest_start..];

    let (mut n, mut e, mut v) = (None, None, None);
    let (mut lat, mut lon, mut ellh, mut q) = (None, None, None, None);
    let mut ellh_end = None;
    let mut q_start = None;
    for cap in tagged_re().captures_iter(rest) {
        let whole = cap.get(0).expect("match");
        let value: f64 = cap[1].parse().map_err(|_| format!("bad number '{}'", &cap[1]))?;
        match cap[2].to_ascii_lowercase().as_str() {
            "n" => n = Some(value),
            "e" => e = Some(value),
            "v" => v = Some(value),
            "lat" => lat = Some(value),
            "lon" => lon = Some(value),
            "ellh" => {
                ellh = Some(value);
                ellh_end = Some(whole.end());
            }
            "q" => {
                q = Some(value);
                q_start = Some(whole.start());
            }
            _ => {}
        }
    }

    let missing = |name: &str| format!("missing ',{name}' field");
    let offset_ned_mm = [
        n.ok_or_else(|| missing("N"))?,
        e.ok_or_else(|| missing("E"))?,
        v.ok_or_else(|| missing("V"))?,
    ];
    let lat_deg = lat.ok_or_else(|| missing("Lat"))?;
    let lon_deg = lon.ok_or_else(|| missing("Lon"))?;
    let ellh_m = ellh.ok_or_else(|| missing("Ellh"))?;
    let q = q.ok_or_else(|| missing("Q"))?;
    if !(-90.0..=90.0).contains(&lat_deg) || !(-180.0..=180.0).contains(&lon_deg) {
        return Err("latitude or longitude out of range".into());
    }
    if q < 0.0 || q.fract() != 0.0 || q > u16::MAX as f64 {
        return Err("bad status flag".into());
    }
    let flag = q as u16;

    // The three standard deviations sit between the Ellh value and the Q flag, untagged.
    let std_neu_m = match (ellh_end, q_start) {
        (Some(a), Some(b)) if b > a => {
            let nums: Vec<f64> = number_re()
                .find_iter(&rest[a..b])
                .filter_map(|m| m.as_str().parse().ok())
                .collect();
            if nums.len() >= 3 {
                Some([nums[0], nums[1], nums[2]])
            } else {
                None
            }
        }
        _ => None,
    };

    Ok(MrkRecord {
        index,
        tow,
        week,
        offset_ned_mm,
        lat_deg,
        lon_deg,
        ellh_m,
        std_neu_m,
        flag,
        quality: RtkQuality::from_flag(flag),
        line,
    })
}

/// Parse the text of an MRK file. Bad lines are collected, not fatal.
pub fn parse_str(text: &str) -> MrkFile {
    let mut out = MrkFile::default();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let trimmed = raw.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_line(trimmed, line) {
            Ok(rec) => out.records.push(rec),
            Err(reason) => out.errors.push(MrkLineError { line, reason }),
        }
    }
    out
}

/// Read and parse an MRK file from disk.
pub fn read(path: &Path) -> Result<MrkFile, Error> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    // MRK files are ASCII; tolerate stray bytes rather than failing.
    let text = String::from_utf8_lossy(&bytes);
    Ok(parse_str(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_real_layout_with_tabs() {
        let line = "1\t219613.373815\t[2059]\t   -14,N\t     6,E\t   195,V\t49.13924545,Lat\t-123.06741924,Lon\t-12.582,Ellh\t0.010359, 0.009882, 0.021327\t50,Q";
        let r = parse_line(line, 1).unwrap();
        assert_eq!(r.index, 1);
        assert_eq!(r.week, 2059);
        assert!((r.tow - 219613.373815).abs() < 1e-9);
        assert_eq!(r.offset_ned_mm, [-14.0, 6.0, 195.0]);
        assert!((r.lat_deg - 49.13924545).abs() < 1e-12);
        assert!((r.lon_deg + 123.06741924).abs() < 1e-12);
        assert!((r.ellh_m + 12.582).abs() < 1e-12);
        assert_eq!(r.std_neu_m, Some([0.010359, 0.009882, 0.021327]));
        assert_eq!(r.quality, RtkQuality::Fixed);
    }

    #[test]
    fn reads_lowercase_tags_and_pipes() {
        // Layout shown in Skydio's description of the DJI-style MRK.
        let line = "1 | 421507.260000 | [2305] | -218,N | -131,E | 12,V | 37.53370452,lat | -122.33230888,lon | 200.599,Ellh | 3.070 | 3.070 | 6.210 | 50,Q";
        let r = parse_line(line, 1).unwrap();
        assert_eq!(r.week, 2305);
        assert_eq!(r.offset_ned_mm, [-218.0, -131.0, 12.0]);
        assert_eq!(r.std_neu_m, Some([3.070, 3.070, 6.210]));
    }

    #[test]
    fn float_is_a_range() {
        assert_eq!(RtkQuality::from_flag(34), RtkQuality::Float);
        assert_eq!(RtkQuality::from_flag(32), RtkQuality::Float);
        assert_eq!(RtkQuality::from_flag(49), RtkQuality::Float);
        assert_eq!(RtkQuality::from_flag(16), RtkQuality::Single);
        assert_eq!(RtkQuality::from_flag(0), RtkQuality::NoPosition);
        assert_eq!(RtkQuality::from_flag(7), RtkQuality::Unknown);
    }

    #[test]
    fn bad_lines_are_reported_not_fatal() {
        let text = "garbage\n\n2\t100.0\t[2300]\t1,N\t2,E\t3,V\t45.0,Lat\t-73.0,Lon\t10.0,Ellh\t0.01, 0.01, 0.02\t16,Q\n";
        let f = parse_str(text);
        assert_eq!(f.records.len(), 1);
        assert_eq!(f.errors.len(), 1);
        assert_eq!(f.errors[0].line, 1);
        assert_eq!(f.records[0].line, 3);
    }

    #[test]
    fn gps_seconds() {
        let line = "1\t10.5\t[2]\t0,N\t0,E\t0,V\t0.0,Lat\t0.0,Lon\t0.0,Ellh\t0, 0, 0\t50,Q";
        let r = parse_line(line, 1).unwrap();
        assert_eq!(r.gps_seconds(), 2.0 * SECONDS_PER_WEEK + 10.5);
    }
}
