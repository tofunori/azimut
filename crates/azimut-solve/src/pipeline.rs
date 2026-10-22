//! One flight from SD-card folder to camera centres.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use azimut_io::photo::ImageKind;
use azimut_io::scan::{AuxKind, Flight};
use azimut_io::time::datetime_from_gps_seconds;

use crate::camera::{antenna_at, camera_centre, CameraCentre};
use crate::config::{BaseSetup, PpkConfig};
use crate::pos::{self, SolQuality};
use crate::rtk_shift::RtkShift;
use crate::rtklib::Rtklib;
use crate::SolveError;

/// Options for processing flights.
#[derive(Debug, Clone)]
pub struct ProcessOptions {
    pub ppk: PpkConfig,
    pub base: BaseSetup,
    /// Largest gap allowed between the two trajectory epochs around an exposure, seconds.
    pub max_gap_s: f64,
    /// Folder for converted RINEX, options and solution files.
    pub work_dir: PathBuf,
}

/// One image file of an exposure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRef {
    pub file_name: String,
    pub path: PathBuf,
    /// `jpeg`, `tiff`, or the band name (`Green`, `Red`, `RedEdge`, `NIR`).
    pub role: String,
    /// Gimbal yaw, pitch, roll from the image XMP, degrees.
    pub gimbal_ypr_deg: Option<[f64; 3]>,
    /// This file's exposure time minus the capture's (the MRK time), seconds, from the XMP
    /// clock. Mavic 3M bands are exposed about 26 ms after the RGB JPEG. `None` when the file
    /// has no exposure time.
    pub time_offset_s: Option<f64>,
    /// Camera centre at this file's own exposure time.
    pub camera: Option<CameraCentre>,
}

/// Result for one exposure (one capture).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExposureSolution {
    /// Photo number from the MRK, or from the file name for unpaired captures.
    pub index: Option<u32>,
    /// GPS time of the exposure, seconds since the GPS epoch (from the MRK).
    pub gps_s: Option<f64>,
    pub files: Vec<FileRef>,
    pub camera: Option<CameraCentre>,
    /// Why there is no camera centre, or why it has no uncertainty.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SolutionStats {
    pub epochs: usize,
    pub epochs_fixed: usize,
    pub exposures: usize,
    pub exposures_fixed: usize,
    pub exposures_float: usize,
    pub exposures_other: usize,
    pub exposures_missing: usize,
    /// Exposures with a camera centre but no usable RTKLIB covariance.
    pub exposures_without_sigma: usize,
    /// Files of positioned exposures that could not be positioned at their own time.
    pub files_without_position: usize,
    /// Largest |height check| over exposures, metres (lever-arm sanity check).
    pub max_height_check_m: f64,
}

/// How the camera positions of a flight were obtained.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Method {
    /// Post-processed kinematic solution against a base station (RTKLIB-EX).
    Ppk {
        rover_obs: PathBuf,
        base_obs: PathBuf,
        navs: Vec<PathBuf>,
        pos_file: PathBuf,
    },
    /// Real-time (RTK) MRK positions moved by a constant shift.
    RtkShift(RtkShift),
}

/// Result for one flight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlightSolution {
    pub dir: PathBuf,
    pub method: Method,
    /// Reference frame of the coordinates (that of the base coordinates).
    pub frame: String,
    pub exposures: Vec<ExposureSolution>,
    pub stats: SolutionStats,
}

/// The image files of one capture.
pub(crate) fn exposure_files(flight: &Flight, capture: usize) -> Vec<FileRef> {
    let cap = &flight.captures[capture];
    cap.members
        .iter()
        .map(|&i| {
            let p = &flight.photos[i];
            FileRef {
                file_name: p.file_name.clone(),
                path: p.path.clone(),
                role: role(&p.kind),
                gimbal_ypr_deg: p.gimbal_ypr_deg,
                // Both readings come from the same camera clock, so GPS/UTC labels cancel.
                time_offset_s: p.exposure_clock_s.zip(cap.exposure_clock_s).map(|(f, c)| f - c),
                camera: None,
            }
        })
        .collect()
}

/// Give every file of a positioned exposure its own camera centre. `locate(dt)` returns the
/// camera centre `dt` seconds after the MRK time, or why there is none. Files without an
/// exposure time are placed at the MRK time, which is said in the exposure's problem.
pub(crate) fn place_files(e: &mut ExposureSolution, locate: impl Fn(f64) -> Result<CameraCentre, String>) {
    let Some(at_mrk) = e.camera else {
        return;
    };
    let mut notes = Vec::new();
    for f in &mut e.files {
        let dt = match f.time_offset_s {
            Some(dt) => dt,
            None => {
                notes.push(format!(
                    "{}: no exposure time, placed at the MRK time",
                    f.file_name
                ));
                0.0
            }
        };
        f.camera = if dt.abs() < 1e-6 {
            Some(at_mrk)
        } else {
            match locate(dt) {
                Ok(c) => {
                    if c.sigma_enu_m.is_none() && at_mrk.sigma_enu_m.is_some() {
                        notes.push(format!(
                            "{}: no valid uncertainty at its exposure time",
                            f.file_name
                        ));
                    }
                    Some(c)
                }
                Err(why) => {
                    notes.push(format!("{}: {why}", f.file_name));
                    None
                }
            }
        };
    }
    if !notes.is_empty() {
        let joined = notes.join("; ");
        e.problem = Some(match e.problem.take() {
            Some(p) => format!("{p}; {joined}"),
            None => joined,
        });
    }
}

/// Add the captures without an MRK record, sort by time and count.
pub(crate) fn finish(
    flight: &Flight,
    mut exposures: Vec<ExposureSolution>,
    mut stats: SolutionStats,
) -> (Vec<ExposureSolution>, SolutionStats) {
    for &c in &flight.matching.unmatched_captures {
        exposures.push(ExposureSolution {
            index: flight.captures[c].sequence,
            gps_s: None,
            files: exposure_files(flight, c),
            camera: None,
            problem: Some("no MRK record for this capture".into()),
        });
    }
    exposures.sort_by(|a, b| {
        a.gps_s
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b.gps_s.unwrap_or(f64::INFINITY))
    });

    for e in &exposures {
        stats.exposures += 1;
        match e.camera.map(|c| c.quality) {
            Some(SolQuality::Fixed) => stats.exposures_fixed += 1,
            Some(SolQuality::Float) => stats.exposures_float += 1,
            Some(_) => stats.exposures_other += 1,
            None => stats.exposures_missing += 1,
        }
        if let Some(c) = e.camera {
            stats.files_without_position += e.files.iter().filter(|f| f.camera.is_none()).count();
            if c.sigma_enu_m.is_none() {
                stats.exposures_without_sigma += 1;
            }
            stats.max_height_check_m = stats.max_height_check_m.max(c.height_check_m.abs());
        }
    }
    (exposures, stats)
}

fn role(kind: &ImageKind) -> String {
    match kind {
        ImageKind::Jpeg => "jpeg".into(),
        ImageKind::Tiff => "tiff".into(),
        ImageKind::Band(b) => b.label().to_string(),
    }
}

/// Process one scanned flight against a base observation file.
pub fn process_flight(
    flight: &Flight,
    base_obs: &Path,
    extra_navs: &[PathBuf],
    rtk: &Rtklib,
    opts: &ProcessOptions,
) -> Result<FlightSolution, SolveError> {
    let first = flight
        .mrk
        .first()
        .ok_or_else(|| SolveError::Missing("no MRK records in this flight".into()))?;
    let stem = flight
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "flight".into());
    let work = opts.work_dir.join(&stem);

    // Rover observations: convert the raw log when there is one, else use RINEX on the card.
    let mut navs: Vec<PathBuf> = Vec::new();
    let raw = flight.aux_files(AuxKind::RawGnss);
    let rover_obs = match raw {
        [one] => {
            let approx = datetime_from_gps_seconds(first.record.gps_seconds());
            let (obs, nav) = rtk.convert_rtcm3(one, &work, approx)?;
            navs.push(nav);
            obs
        }
        [] => flight
            .aux_files(AuxKind::RinexObs)
            .first()
            .cloned()
            .ok_or_else(|| {
                SolveError::Missing("no raw GNSS log or RINEX observation file for the drone".into())
            })?,
        many => {
            return Err(SolveError::Missing(format!(
                "{} raw GNSS logs in one folder; merging them is not supported yet",
                many.len()
            )))
        }
    };
    for n in flight.aux_files(AuxKind::RinexNav).iter().chain(extra_navs) {
        if std::fs::metadata(n).map(|m| m.len() > 0).unwrap_or(false) && !navs.contains(n) {
            navs.push(n.clone());
        }
    }
    if navs.is_empty() {
        return Err(SolveError::Missing("no navigation file (orbits)".into()));
    }

    let conf = opts.ppk.to_conf(&opts.base);
    let pos_file = rtk.solve(&rover_obs, base_obs, &navs, &conf, &work, &stem)?;
    let traj = pos::read(&pos_file)?;

    let stats = SolutionStats {
        epochs: traj.len(),
        epochs_fixed: traj.iter().filter(|e| e.quality == SolQuality::Fixed).count(),
        ..Default::default()
    };

    let mut exposures = Vec::new();
    for pair in &flight.matching.pairs {
        let rec = &flight.mrk[pair.mrk].record;
        let t = rec.gps_seconds();
        let at = |t: f64| -> Result<CameraCentre, String> {
            antenna_at(&traj, t, opts.max_gap_s)
                .map(|fix| camera_centre(&fix, rec.offset_ned_mm))
                .map_err(|e| e.to_string())
        };
        let (camera, problem) = match antenna_at(&traj, t, opts.max_gap_s) {
            Ok(fix) => {
                let c = camera_centre(&fix, rec.offset_ned_mm);
                let problem = c
                    .sigma_enu_m
                    .is_none()
                    .then(|| "RTKLIB covariance not positive definite; uncertainty left empty".to_string());
                (Some(c), problem)
            }
            Err(e) => (None, Some(e.to_string())),
        };
        let mut e = ExposureSolution {
            index: Some(rec.index),
            gps_s: Some(t),
            files: exposure_files(flight, pair.capture),
            camera,
            problem,
        };
        // The MRK lever arm is the RGB camera's; the band lenses' own offset is not documented.
        place_files(&mut e, |dt| at(t + dt));
        exposures.push(e);
    }
    let (exposures, stats) = finish(flight, exposures, stats);

    Ok(FlightSolution {
        dir: flight.dir.clone(),
        method: Method::Ppk {
            rover_obs,
            base_obs: base_obs.to_path_buf(),
            navs,
            pos_file,
        },
        frame: opts.base.frame.clone(),
        exposures,
        stats,
    })
}
