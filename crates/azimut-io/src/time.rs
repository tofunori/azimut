//! Time conversions between DJI clock strings and GPS time.
//!
//! All times are carried as seconds since the GPS epoch (1980-01-06 00:00:00), as `f64`.
//! A DJI clock string is converted "as written": whether that reading is GPS time or UTC is
//! not trusted from its label and is decided later from the data (see `matching`).

use chrono::{NaiveDate, NaiveDateTime};

use crate::mrk::SECONDS_PER_WEEK;

/// GPS minus UTC, in seconds. Constant since 2017-01-01; no leap second has been added since.
pub const GPS_UTC_OFFSET_S: f64 = 18.0;

fn gps_epoch() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1980, 1, 6)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .expect("valid date")
}

/// Seconds since the GPS epoch for a calendar reading (no time zone applied).
pub fn seconds_since_gps_epoch(t: NaiveDateTime) -> f64 {
    let d = t - gps_epoch();
    let micros = d.num_microseconds().unwrap_or(i64::MAX);
    micros as f64 / 1e6
}

/// Calendar reading for a number of seconds since the GPS epoch.
pub fn datetime_from_gps_seconds(s: f64) -> NaiveDateTime {
    let micros = (s * 1e6).round() as i64;
    gps_epoch() + chrono::Duration::microseconds(micros)
}

/// Split seconds since the GPS epoch into (week, time of week).
pub fn week_tow(s: f64) -> (u32, f64) {
    let week = (s / SECONDS_PER_WEEK).floor();
    (week as u32, s - week * SECONDS_PER_WEEK)
}

/// Parse a DJI clock string such as `2023:03:09 02:47:57.725671` (XMP `UTCAtExposure`).
/// Also accepts `-` as date separator, a `T` between date and time, and a trailing `Z`.
/// Returns the reading as seconds since the GPS epoch, without any time-scale correction.
pub fn parse_dji_clock(s: &str) -> Option<f64> {
    let t = s.trim().trim_end_matches('Z');
    const FORMATS: [&str; 4] = [
        "%Y:%m:%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y:%m:%dT%H:%M:%S%.f",
    ];
    FORMATS
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(t, f).ok())
        .map(seconds_since_gps_epoch)
}

/// Format seconds since the GPS epoch the way DJI writes `UTCAtExposure`.
pub fn format_dji_clock(s: f64) -> String {
    datetime_from_gps_seconds(s)
        .format("%Y:%m:%d %H:%M:%S%.6f")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_zero() {
        assert_eq!(parse_dji_clock("1980:01:06 00:00:00"), Some(0.0));
    }

    #[test]
    fn microseconds_survive() {
        let s = parse_dji_clock("2023:03:09 02:47:57.725671").unwrap();
        assert_eq!(format_dji_clock(s), "2023:03:09 02:47:57.725671");
    }

    #[test]
    fn other_separators() {
        let a = parse_dji_clock("2023:03:09 02:47:57.5").unwrap();
        assert_eq!(parse_dji_clock("2023-03-09T02:47:57.5Z"), Some(a));
        assert_eq!(parse_dji_clock("2023-03-09 02:47:57.5"), Some(a));
        assert!(parse_dji_clock("not a date").is_none());
    }

    #[test]
    fn week_and_tow_round_trip() {
        let s = 2305.0 * SECONDS_PER_WEEK + 421_507.26;
        let (w, tow) = week_tow(s);
        assert_eq!(w, 2305);
        assert!((tow - 421_507.26).abs() < 1e-6);
    }
}
