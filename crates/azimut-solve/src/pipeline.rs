//! One flight from SD-card folder to camera centres.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use azimut_io::photo::ImageKind;
use azimut_io::scan::{AuxKind, Flight};
use azimut_io::time::datetime_from_gps_seconds;

use crate::camera::{antenna_at, camera_centre, CameraCentre};
use crate::config::{BaseSetup, PpkConfig};
use crate::pos::{self, SolQuality};
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
    /// Largest |height check| over exposures, metres (lever-arm sanity check).
    pub max_height_check_m: f64,
}

/// Result for one flight.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlightSolution {
    pub dir: PathBuf,
    pub rover_obs: PathBuf,
    pub base_obs: PathBuf,
    pub navs: Vec<PathBuf>,
    pub pos_file: PathBuf,
    /// Reference frame of the coordinates (that of the base coordinates).
    pub frame: String,
    pub exposures: Vec<ExposureSolution>,
    pub stats: SolutionStats,
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

    let mut stats = SolutionStats {
        epochs: traj.len(),
        epochs_fixed: traj.iter().filter(|e| e.quality == SolQuality::Fixed).count(),
        ..Default::default()
    };

    let files_of = |capture: usize| -> Vec<FileRef> {
        flight.captures[capture]
            .members
            .iter()
            .map(|&i| {
                let p = &flight.photos[i];
                FileRef {
                    file_name: p.file_name.clone(),
                    path: p.path.clone(),
                    role: role(&p.kind),
                    gimbal_ypr_deg: p.gimbal_ypr_deg,
                }
            })
            .collect()
    };

    let mut exposures = Vec::new();
    for pair in &flight.matching.pairs {
        let rec = &flight.mrk[pair.mrk].record;
        let t = rec.gps_seconds();
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
        exposures.push(ExposureSolution {
            index: Some(rec.index),
            gps_s: Some(t),
            files: files_of(pair.capture),
            camera,
            problem,
        });
    }
    for &c in &flight.matching.unmatched_captures {
        exposures.push(ExposureSolution {
            index: flight.captures[c].sequence,
            gps_s: None,
            files: files_of(c),
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
            if c.sigma_enu_m.is_none() {
                stats.exposures_without_sigma += 1;
            }
            stats.max_height_check_m = stats.max_height_check_m.max(c.height_check_m.abs());
        }
    }

    Ok(FlightSolution {
        dir: flight.dir.clone(),
        rover_obs,
        base_obs: base_obs.to_path_buf(),
        navs,
        pos_file,
        frame: opts.base.frame.clone(),
        exposures,
        stats,
    })
}
