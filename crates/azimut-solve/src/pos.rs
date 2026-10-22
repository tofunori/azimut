//! Reader for RTKLIB solution files written in ECEF (`out-solformat=xyz`).
//!
//! Line layout (RTKLIB `outecef`): time, x, y, z (m), Q, ns, sdx, sdy, sdz (m),
//! sdxy, sdyz, sdzx (signed square roots of the covariances, m), age (s), ratio.
//! Time is either `week tow` (`out-timeform=tow`) or `yyyy/mm/dd hh:mm:ss.sss`.

use std::path::Path;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use azimut_io::mrk::SECONDS_PER_WEEK;
use azimut_io::time::seconds_since_gps_epoch;

/// RTKLIB solution status (`SOLQ_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolQuality {
    Fixed = 1,
    Float = 2,
    Sbas = 3,
    Dgps = 4,
    Single = 5,
    Ppp = 6,
    DeadReckoning = 7,
}

impl SolQuality {
    pub fn from_code(q: i64) -> Option<Self> {
        Some(match q {
            1 => SolQuality::Fixed,
            2 => SolQuality::Float,
            3 => SolQuality::Sbas,
            4 => SolQuality::Dgps,
            5 => SolQuality::Single,
            6 => SolQuality::Ppp,
            7 => SolQuality::DeadReckoning,
            _ => return None,
        })
    }

    /// Larger is worse. Used to keep the worse of two interpolated epochs.
    pub fn rank(self) -> u8 {
        match self {
            SolQuality::Fixed => 0,
            SolQuality::Float => 1,
            SolQuality::Ppp => 2,
            SolQuality::Dgps | SolQuality::Sbas => 3,
            SolQuality::Single => 4,
            SolQuality::DeadReckoning => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SolQuality::Fixed => "fixed",
            SolQuality::Float => "float",
            SolQuality::Sbas => "sbas",
            SolQuality::Dgps => "dgps",
            SolQuality::Single => "single",
            SolQuality::Ppp => "ppp",
            SolQuality::DeadReckoning => "dead_reckoning",
        }
    }
}

/// One epoch of the antenna trajectory.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PosEpoch {
    /// GPS time, seconds since the GPS epoch.
    pub gps_s: f64,
    pub xyz: [f64; 3],
    pub quality: SolQuality,
    pub n_sats: u32,
    /// ECEF covariance, m².
    pub cov: [[f64; 3]; 3],
    pub age_s: f64,
    pub ratio: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum PosError {
    #[error("{0}: {1}")]
    Io(String, std::io::Error),
    #[error("line {line}: {reason}")]
    Line { line: usize, reason: String },
    #[error("no solution epochs in {0}")]
    Empty(String),
}

fn signed_square(s: f64) -> f64 {
    s * s.abs()
}

/// Parse the text of a solution file. Comment lines start with `%`.
pub fn parse_str(text: &str) -> Result<Vec<PosEpoch>, PosError> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('%') {
            continue;
        }
        let err = |reason: &str| PosError::Line {
            line: i + 1,
            reason: reason.to_string(),
        };
        let t: Vec<&str> = line.split_whitespace().collect();
        let (gps_s, rest) = if t.first().is_some_and(|s| s.contains('/')) {
            if t.len() < 2 {
                return Err(err("missing time"));
            }
            let dt = NaiveDateTime::parse_from_str(&format!("{} {}", t[0], t[1]), "%Y/%m/%d %H:%M:%S%.f")
                .map_err(|_| err("bad date/time"))?;
            (seconds_since_gps_epoch(dt), &t[2..])
        } else {
            let week: f64 = t
                .first()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| err("bad GPS week"))?;
            let tow: f64 = t
                .get(1)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| err("bad time of week"))?;
            (week * SECONDS_PER_WEEK + tow, &t[2..])
        };
        if rest.len() < 13 {
            return Err(err(
                "expected x y z Q ns sdx sdy sdz sdxy sdyz sdzx age ratio (out-solformat=xyz)",
            ));
        }
        let v: Vec<f64> = rest[..13]
            .iter()
            .map(|s| s.parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| err("bad number"))?;
        let norm = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if !(6.0e6..7.0e6).contains(&norm) {
            return Err(err(
                "not ECEF coordinates: looks like lat/lon/height output; use out-solformat=xyz",
            ));
        }
        let quality = SolQuality::from_code(v[3] as i64).ok_or_else(|| err("unknown solution status"))?;
        let (sx, sy, sz) = (v[5], v[6], v[7]);
        let (cxy, cyz, czx) = (signed_square(v[8]), signed_square(v[9]), signed_square(v[10]));
        out.push(PosEpoch {
            gps_s,
            xyz: [v[0], v[1], v[2]],
            quality,
            n_sats: v[4] as u32,
            cov: [[sx * sx, cxy, czx], [cxy, sy * sy, cyz], [czx, cyz, sz * sz]],
            age_s: v[11],
            ratio: v[12],
        });
    }
    out.sort_by(|a, b| a.gps_s.total_cmp(&b.gps_s));
    Ok(out)
}

pub fn read(path: &Path) -> Result<Vec<PosEpoch>, PosError> {
    let text = std::fs::read_to_string(path).map_err(|e| PosError::Io(path.display().to_string(), e))?;
    let v = parse_str(&text)?;
    if v.is_empty() {
        return Err(PosError::Empty(path.display().to_string()));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOW: &str = "\
% program   : RNX2RTKP ver.demo5
%  GPST                      x-ecef(m)      y-ecef(m)      z-ecef(m)   Q  ns   sdx(m)   sdy(m)   sdz(m)  sdxy(m)  sdyz(m)  sdzx(m) age(s)  ratio
2420 136800.000   1347546.1234 -4210382.5678  4594623.9012   1  18   0.0050   0.0060   0.0070   0.0020  -0.0030   0.0010   1.00  999.9
2420 136800.200   1347546.2234 -4210382.6678  4594624.0012   2  17   0.0500   0.0600   0.0700   0.0000   0.0000   0.0000   1.20    2.1
";

    #[test]
    fn reads_tow_lines_and_covariance_signs() {
        let v = parse_str(SAMPLE_TOW).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].gps_s, 2420.0 * SECONDS_PER_WEEK + 136_800.0);
        assert_eq!(v[0].quality, SolQuality::Fixed);
        assert_eq!(v[1].quality, SolQuality::Float);
        assert_eq!(v[0].n_sats, 18);
        assert!((v[0].cov[0][0] - 2.5e-5).abs() < 1e-12);
        assert!((v[0].cov[0][1] - 4e-6).abs() < 1e-12);
        assert!(
            (v[0].cov[1][2] + 9e-6).abs() < 1e-12,
            "negative covariance keeps its sign"
        );
        assert_eq!(v[0].cov[1][2], v[0].cov[2][1]);
        assert_eq!(v[0].ratio, 999.9);
    }

    #[test]
    fn reads_calendar_time() {
        let line = "2026/06/15 14:00:00.000   1347546.1234 -4210382.5678  4594623.9012   1  18   0.0050   0.0060   0.0070   0.0020  -0.0030   0.0010   1.00  999.9";
        let v = parse_str(line).unwrap();
        let expected = azimut_io::time::parse_dji_clock("2026:06:15 14:00:00").unwrap();
        assert_eq!(v[0].gps_s, expected);
    }

    #[test]
    fn llh_output_is_rejected_with_a_hint() {
        let line = "2420 136800.000   46.35000000  -72.55000000   120.0000   1  18   0.0050   0.0060   0.0070   0.0020  -0.0030   0.0010   1.00  999.9";
        let e = parse_str(line).unwrap_err().to_string();
        assert!(e.contains("out-solformat=xyz"), "{e}");
    }
}
