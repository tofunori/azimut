//! Folder inventory and pre-processing diagnosis.
//!
//! `scan` walks a folder (an SD card copy, a mission folder, or a whole campaign), finds the
//! flight folders, reads image metadata in parallel from headers only, reads the MRK files,
//! groups images into captures, pairs captures with MRK records and lists what would break
//! a PPK run before any processing starts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::capture::{group, Capture};
use crate::matching::{match_on_time, ClockReading, MatchResult, DEFAULT_TOLERANCE_S};
use crate::mrk::{self, MrkLineError, MrkRecord, RtkQuality};
use crate::photo::{ext, is_jpeg, is_tiff, read_photo, Photo};

/// Non-image files that matter for PPK.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuxKind {
    /// `*.MRK` exposure log.
    Mrk,
    /// Aircraft raw GNSS log (RTCM 3): `*_PPKRAW.bin`, `*.RTK`.
    RawGnss,
    /// RINEX observation file decoded on board or converted.
    RinexObs,
    /// RINEX navigation file.
    RinexNav,
    /// Base-station log, e.g. D-RTK `*.DAT`, L2 `*.RTB`.
    BaseLog,
}

/// Classify a non-image file by extension and name. Case is ignored.
pub fn aux_kind(path: &Path) -> Option<AuxKind> {
    let e = ext(path)?;
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    let rinex_obs =
        e == "obs" || (e.len() == 3 && e.ends_with('o') && e[..2].chars().all(|c| c.is_ascii_digit()));
    let rinex_nav = e == "nav"
        || e == "rnx" && name.contains("_mn")
        || (e.len() == 3
            && e[..2].chars().all(|c| c.is_ascii_digit())
            && matches!(&e[2..], "n" | "g" | "l" | "p" | "c"));
    match e.as_str() {
        "mrk" => Some(AuxKind::Mrk),
        "rtk" => Some(AuxKind::RawGnss),
        "bin" if name.contains("ppkraw") => Some(AuxKind::RawGnss),
        "dat" | "rtb" => Some(AuxKind::BaseLog),
        "rnx" if name.contains("_mo") => Some(AuxKind::RinexObs),
        _ if rinex_obs => Some(AuxKind::RinexObs),
        _ if rinex_nav => Some(AuxKind::RinexNav),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// One diagnosis line. `code` is stable and meant for programs; `message` for people.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

fn finding(severity: Severity, code: &str, message: impl Into<String>) -> Finding {
    Finding {
        severity,
        code: code.to_string(),
        message: message.into(),
    }
}

/// MRK records of a flight, with the file each came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MrkEntry {
    pub file: usize,
    pub record: MrkRecord,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QualityCounts {
    pub fixed: usize,
    pub float: usize,
    pub single: usize,
    pub no_position: usize,
    pub unknown: usize,
}

/// Everything found in one flight folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flight {
    pub dir: PathBuf,
    pub photos: Vec<Photo>,
    pub captures: Vec<Capture>,
    pub aux: BTreeMap<AuxKind, Vec<PathBuf>>,
    pub mrk: Vec<MrkEntry>,
    pub mrk_errors: Vec<(usize, MrkLineError)>,
    pub quality: QualityCounts,
    pub matching: MatchResult,
    pub findings: Vec<Finding>,
}

impl Flight {
    pub fn aux_files(&self, kind: AuxKind) -> &[PathBuf] {
        self.aux.get(&kind).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Result of a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub root: PathBuf,
    pub flights: Vec<Flight>,
    pub files_seen: usize,
    pub images: usize,
    pub bytes_on_disk: u64,
    pub bytes_read: u64,
    pub elapsed_s: f64,
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub tolerance_s: f64,
    /// Skip hidden files and folders (names starting with `.`).
    pub skip_hidden: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            tolerance_s: DEFAULT_TOLERANCE_S,
            skip_hidden: true,
        }
    }
}

/// Scan a folder tree.
pub fn scan(root: &Path, opts: &ScanOptions) -> ScanReport {
    let t0 = Instant::now();
    let mut images: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut aux: BTreeMap<PathBuf, BTreeMap<AuxKind, Vec<PathBuf>>> = BTreeMap::new();
    let mut files_seen = 0usize;

    let walker = WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    for entry in walker
        .filter_entry(|e| {
            !opts.skip_hidden || e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')
        })
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        files_seen += 1;
        let path = entry.into_path();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if is_jpeg(&path) || is_tiff(&path) {
            images.entry(dir).or_default().push(path);
        } else if let Some(k) = aux_kind(&path) {
            aux.entry(dir).or_default().entry(k).or_default().push(path);
        }
    }

    // A flight folder holds images or an MRK file.
    let mut dirs: Vec<PathBuf> = images.keys().cloned().collect();
    for (d, kinds) in &aux {
        if kinds.contains_key(&AuxKind::Mrk) && !images.contains_key(d) {
            dirs.push(d.clone());
        }
    }
    dirs.sort();

    let flights: Vec<Flight> = dirs
        .into_iter()
        .map(|dir| {
            let imgs = images.remove(&dir).unwrap_or_default();
            let aux = aux.remove(&dir).unwrap_or_default();
            build_flight(dir, imgs, aux, opts)
        })
        .collect();

    let images_n = flights.iter().map(|f| f.photos.len()).sum();
    let bytes_on_disk = flights.iter().flat_map(|f| &f.photos).map(|p| p.size).sum();
    let bytes_read = flights.iter().flat_map(|f| &f.photos).map(|p| p.bytes_read).sum();
    ScanReport {
        root: root.to_path_buf(),
        flights,
        files_seen,
        images: images_n,
        bytes_on_disk,
        bytes_read,
        elapsed_s: t0.elapsed().as_secs_f64(),
    }
}

fn build_flight(
    dir: PathBuf,
    image_paths: Vec<PathBuf>,
    aux: BTreeMap<AuxKind, Vec<PathBuf>>,
    opts: &ScanOptions,
) -> Flight {
    // Headers only, in parallel.
    let photos: Vec<Photo> = image_paths.par_iter().map(|p| read_photo(p)).collect();
    let captures = group(&photos);

    let mut mrk = Vec::new();
    let mut mrk_errors = Vec::new();
    let mut findings = Vec::new();
    for (fi, path) in aux
        .get(&AuxKind::Mrk)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .enumerate()
    {
        match mrk::read(path) {
            Ok(f) => {
                mrk.extend(f.records.into_iter().map(|record| MrkEntry { file: fi, record }));
                mrk_errors.extend(f.errors.into_iter().map(|e| (fi, e)));
            }
            Err(e) => findings.push(finding(Severity::Error, "mrk_unreadable", e.to_string())),
        }
    }
    mrk.sort_by(|a, b| a.record.gps_seconds().total_cmp(&b.record.gps_seconds()));

    let mut quality = QualityCounts::default();
    for e in &mrk {
        match e.record.quality {
            RtkQuality::Fixed => quality.fixed += 1,
            RtkQuality::Float => quality.float += 1,
            RtkQuality::Single => quality.single += 1,
            RtkQuality::NoPosition => quality.no_position += 1,
            RtkQuality::Unknown => quality.unknown += 1,
        }
    }

    let records: Vec<MrkRecord> = mrk.iter().map(|e| e.record.clone()).collect();
    let matching = match_on_time(&records, &captures, opts.tolerance_s);

    let mut flight = Flight {
        dir,
        photos,
        captures,
        aux,
        mrk,
        mrk_errors,
        quality,
        matching,
        findings,
    };
    diagnose(&mut flight);
    flight
}

fn diagnose(f: &mut Flight) {
    let mut out = std::mem::take(&mut f.findings);
    let n_mrk = f.mrk.len();
    let n_cap = f.captures.len();
    let n_mrk_files = f.aux_files(AuxKind::Mrk).len();

    if !f.photos.is_empty() && n_mrk_files == 0 {
        out.push(finding(Severity::Error, "no_mrk", "Images but no MRK file: exposure times and lever arms are unknown, PPK geotagging is impossible."));
    }
    if n_mrk_files > 0
        && f.aux_files(AuxKind::RawGnss).is_empty()
        && f.aux_files(AuxKind::RinexObs).is_empty()
    {
        out.push(finding(Severity::Error, "no_rover_gnss", "No raw GNSS log (*_PPKRAW.bin, *.RTK) or RINEX observation file: only the real-time MRK positions are available."));
    } else if n_mrk_files > 0 && f.aux_files(AuxKind::RawGnss).is_empty() {
        out.push(finding(Severity::Warning, "no_raw_log", "No raw RTCM log; only an on-board RINEX file. On-board RINEX has been seen with wrong dates or missing L2: prefer the raw log."));
    }
    if n_mrk_files > 1 {
        out.push(finding(
            Severity::Info,
            "several_mrk",
            format!("{n_mrk_files} MRK files in this folder; their records are merged and sorted by time."),
        ));
    }
    if !f.mrk_errors.is_empty() {
        let lines: Vec<String> = f
            .mrk_errors
            .iter()
            .take(5)
            .map(|(_, e)| format!("line {}: {}", e.line, e.reason))
            .collect();
        out.push(finding(
            Severity::Warning,
            "mrk_bad_lines",
            format!(
                "{} unreadable MRK line(s) ({}).",
                f.mrk_errors.len(),
                lines.join("; ")
            ),
        ));
    }

    let unreadable = f.photos.iter().filter(|p| p.error.is_some()).count();
    if unreadable > 0 {
        let first = f
            .photos
            .iter()
            .find(|p| p.error.is_some())
            .map(|p| format!("{}: {}", p.file_name, p.error.as_deref().unwrap_or("")))
            .unwrap_or_default();
        out.push(finding(
            Severity::Warning,
            "image_metadata",
            format!("{unreadable} image(s) without readable XMP metadata (e.g. {first})."),
        ));
    }

    // Multispectral captures.
    let ms: Vec<&Capture> = f.captures.iter().filter(|c| c.is_multispectral()).collect();
    if !ms.is_empty() {
        let incomplete: Vec<String> = ms
            .iter()
            .filter(|c| !c.is_complete_m3m())
            .map(|c| {
                let missing: Vec<String> = c.missing_bands().iter().map(|b| b.label().to_string()).collect();
                let seq = c
                    .sequence
                    .map(|s| format!("#{s:04}"))
                    .unwrap_or_else(|| "?".into());
                if c.jpegs != 1 {
                    format!("{seq} ({} JPEG)", c.jpegs)
                } else {
                    format!("{seq} (missing {})", missing.join(", "))
                }
            })
            .collect();
        if incomplete.is_empty() {
            out.push(finding(
                Severity::Info,
                "m3m_groups",
                format!(
                    "{} multispectral captures, all complete (1 JPEG + 4 bands).",
                    ms.len()
                ),
            ));
        } else {
            let shown: Vec<String> = incomplete.iter().take(8).cloned().collect();
            out.push(finding(
                Severity::Warning,
                "m3m_incomplete",
                format!(
                    "{} of {} multispectral captures are incomplete: {}{}.",
                    incomplete.len(),
                    ms.len(),
                    shown.join(", "),
                    if incomplete.len() > 8 { ", ..." } else { "" }
                ),
            ));
        }
        let spread = ms.iter().map(|c| c.clock_spread_s).fold(0.0, f64::max);
        if spread > 0.01 {
            out.push(finding(
                Severity::Info,
                "m3m_clock_spread",
                format!(
                    "Exposure times within one capture differ by up to {spread:.3} s; the JPEG time is used."
                ),
            ));
        }
        let by_seq = ms
            .iter()
            .filter(|c| c.key == crate::capture::GroupKey::Sequence)
            .count();
        if by_seq > 0 {
            out.push(finding(Severity::Warning, "m3m_no_uuid", format!("{by_seq} multispectral capture(s) grouped by file number because CaptureUUID is missing.")));
        }
    }

    // Time pairing.
    let m = &f.matching;
    if n_mrk > 0 && n_cap > 0 {
        match m.clock {
            None => out.push(finding(Severity::Error, "no_time_match", format!("No capture pairs with an MRK record within {:.3} s, reading image times as GPS time or as UTC. Check the camera clock and that the MRK belongs to these images.", m.tolerance_s))),
            Some(ClockReading::GpsTime) => out.push(finding(Severity::Info, "clock_gps", "Image exposure times (XMP UTCAtExposure) read as GPS time, despite the UTC label.")),
            Some(ClockReading::Utc) => out.push(finding(Severity::Info, "clock_utc", "Image exposure times (XMP UTCAtExposure) read as UTC (18 s behind GPS).")),
        }
        if m.clock.is_some() {
            out.push(finding(
                Severity::Info,
                "paired",
                format!(
                    "{} of {} MRK records paired with a capture (largest time gap {:.6} s).",
                    m.pairs.len(),
                    n_mrk,
                    m.max_abs_dt_s
                ),
            ));
            if !m.unmatched_mrk.is_empty() {
                out.push(finding(
                    Severity::Warning,
                    "mrk_without_capture",
                    format!(
                        "{} MRK record(s) have no capture (photos deleted or in another folder?).",
                        m.unmatched_mrk.len()
                    ),
                ));
            }
            if !m.unmatched_captures.is_empty() {
                out.push(finding(
                    Severity::Warning,
                    "capture_without_mrk",
                    format!(
                        "{} capture(s) have no MRK record (MRK from another folder or truncated?).",
                        m.unmatched_captures.len()
                    ),
                ));
            }
        }
        if !m.captures_without_time.is_empty() {
            out.push(finding(
                Severity::Warning,
                "capture_without_time",
                format!(
                    "{} capture(s) without a readable exposure time; they cannot be paired.",
                    m.captures_without_time.len()
                ),
            ));
        }
    }

    // Real-time solution quality in the MRK.
    if n_mrk > 0 {
        let q = &f.quality;
        let pct = 100.0 * q.fixed as f64 / n_mrk as f64;
        let sev = if pct < 95.0 {
            Severity::Warning
        } else {
            Severity::Info
        };
        out.push(finding(sev, "mrk_quality", format!("Real-time status in the MRK: {pct:.1} % fixed ({} fixed, {} float, {} single, {} none). PPK will replace these positions.", q.fixed, q.float, q.single, q.no_position)));
    }

    // Folder splitting and numbering.
    if let Some(max_seq) = f.photos.iter().filter_map(|p| p.sequence).max() {
        if max_seq >= 999 {
            out.push(finding(Severity::Warning, "folder_split", "This folder reaches photo number 999. DJI then opens a new folder and closes the GNSS log a few seconds early: the last exposures may only be covered by the next folder's log."));
        }
        let min_seq = f.photos.iter().filter_map(|p| p.sequence).min().unwrap_or(1);
        if min_seq > 1 {
            out.push(finding(Severity::Info, "starts_after_1", format!("Numbering starts at {min_seq:04}, not 0001 (interrupted or continued mission). Pairing uses time, not order.")));
        }
    }

    out.sort_by_key(|x| std::cmp::Reverse(x.severity));
    f.findings = out;
}
