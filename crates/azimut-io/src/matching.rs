//! Pairing MRK records with captures on exposure time.
//!
//! The MRK carries GPS week and time of week. The images carry XMP `UTCAtExposure`. DJI
//! labels that field UTC, but measurements on P1 and L2 data found GPS time (18 s ahead of
//! UTC). Rather than trust either label, both readings are tried and the one that pairs the
//! most records wins. Pairing never falls back on file order. See SOURCES.md (time).

use serde::{Deserialize, Serialize};

use crate::capture::Capture;
use crate::mrk::MrkRecord;
use crate::time::GPS_UTC_OFFSET_S;

/// Default pairing tolerance, seconds. Far below any photo interval, far above the
/// microsecond agreement seen in real data.
pub const DEFAULT_TOLERANCE_S: f64 = 0.05;

/// How the image clock relates to GPS time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockReading {
    /// The image clock reads GPS time.
    GpsTime,
    /// The image clock reads UTC; GPS = clock + 18 s.
    Utc,
}

impl ClockReading {
    pub fn offset_s(self) -> f64 {
        match self {
            ClockReading::GpsTime => 0.0,
            ClockReading::Utc => GPS_UTC_OFFSET_S,
        }
    }
}

/// One MRK record paired with one capture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub mrk: usize,
    pub capture: usize,
    /// MRK GPS time minus the capture's GPS time, seconds.
    pub dt_s: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatchResult {
    pub clock: Option<ClockReading>,
    pub tolerance_s: f64,
    pub pairs: Vec<Pair>,
    pub unmatched_mrk: Vec<usize>,
    pub unmatched_captures: Vec<usize>,
    /// Captures without a readable exposure time.
    pub captures_without_time: Vec<usize>,
    /// Largest |dt| among pairs, seconds.
    pub max_abs_dt_s: f64,
}

/// Pair records and captures. `mrk` and `captures` may be in any order.
pub fn match_on_time(mrk: &[MrkRecord], captures: &[Capture], tolerance_s: f64) -> MatchResult {
    let captures_without_time: Vec<usize> = captures
        .iter()
        .enumerate()
        .filter(|(_, c)| c.exposure_clock_s.is_none())
        .map(|(i, _)| i)
        .collect();

    let mut best: Option<(ClockReading, Vec<Pair>)> = None;
    for reading in [ClockReading::GpsTime, ClockReading::Utc] {
        let pairs = pair_with_offset(mrk, captures, reading.offset_s(), tolerance_s);
        let better = match &best {
            None => true,
            Some((_, b)) => pairs.len() > b.len(),
        };
        if better {
            best = Some((reading, pairs));
        }
    }
    let (clock, pairs) = match best {
        Some((r, p)) if !p.is_empty() => (Some(r), p),
        _ => (None, Vec::new()),
    };

    let mut mrk_used = vec![false; mrk.len()];
    let mut cap_used = vec![false; captures.len()];
    for p in &pairs {
        mrk_used[p.mrk] = true;
        cap_used[p.capture] = true;
    }
    let max_abs_dt_s = pairs.iter().map(|p| p.dt_s.abs()).fold(0.0, f64::max);
    MatchResult {
        clock,
        tolerance_s,
        unmatched_mrk: (0..mrk.len()).filter(|&i| !mrk_used[i]).collect(),
        unmatched_captures: (0..captures.len())
            .filter(|&i| !cap_used[i] && captures[i].exposure_clock_s.is_some())
            .collect(),
        captures_without_time,
        max_abs_dt_s,
        pairs,
    }
}

fn pair_with_offset(mrk: &[MrkRecord], captures: &[Capture], offset_s: f64, tol: f64) -> Vec<Pair> {
    // Capture GPS times, sorted.
    let mut times: Vec<(f64, usize)> = captures
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.exposure_clock_s.map(|t| (t + offset_s, i)))
        .collect();
    times.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Candidate pairs within tolerance, closest first, each side used once.
    let mut candidates: Vec<Pair> = Vec::new();
    for (mi, r) in mrk.iter().enumerate() {
        let t = r.gps_seconds();
        let start = times.partition_point(|(ct, _)| *ct < t - tol);
        for &(ct, ci) in times[start..].iter().take_while(|(ct, _)| *ct <= t + tol) {
            candidates.push(Pair {
                mrk: mi,
                capture: ci,
                dt_s: t - ct,
            });
        }
    }
    candidates.sort_by(|a, b| a.dt_s.abs().total_cmp(&b.dt_s.abs()));
    let mut mrk_used = vec![false; mrk.len()];
    let mut cap_used = vec![false; captures.len()];
    let mut pairs = Vec::new();
    for c in candidates {
        if !mrk_used[c.mrk] && !cap_used[c.capture] {
            mrk_used[c.mrk] = true;
            cap_used[c.capture] = true;
            pairs.push(c);
        }
    }
    pairs.sort_by_key(|p| p.mrk);
    pairs
}
