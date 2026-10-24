//! # azimut-solve
//!
//! PPK processing for Azimut.
//!
//! - [`rtklib`]: run RTKLIB-EX `convbin` (raw RTCM 3 to RINEX) and `rnx2rtkp` (PPK).
//! - [`config`]: processing options written as an RTKLIB options file.
//! - [`antex`]: Emlid receiver antenna models embedded from igs20.atx.
//! - [`pos`]: read RTKLIB ECEF solutions with their full covariance.
//! - [`camera`]: interpolate the antenna to each exposure and apply the MRK lever arm.
//! - [`geodesy`]: GRS80 conversions and ENU rotations.
//! - [`pipeline`]: one scanned flight from raw logs to camera centres.
//! - [`rtk_shift`]: real-time (RTK) MRK positions moved by a constant base correction.

use std::path::{Path, PathBuf};

pub mod antex;
pub mod camera;
pub mod config;
pub mod geodesy;
pub mod pipeline;
pub mod pos;
pub mod rtk_shift;
pub mod rtklib;

pub use config::{BasePosition, BaseSetup, PpkConfig};
pub use pipeline::{process_flight, ExposureSolution, FlightSolution, Method, ProcessOptions};
pub use rtk_shift::{shift_flight, ShiftSpec};
pub use rtklib::Rtklib;

#[derive(Debug, thiserror::Error)]
pub enum SolveError {
    #[error(
        "RTKLIB-EX programs (convbin, rnx2rtkp) not found. Give their folder with --rtklib, \
         set {}, put them in an `rtklib` folder next to azimut (as in the release archives) \
         or add them to the PATH.",
        rtklib::ENV_DIR
    )]
    RtklibNotFound,
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{program} failed (exit {status:?}): {stderr}")]
    Program {
        program: String,
        status: Option<i32>,
        stderr: String,
    },
    #[error("{0}")]
    Missing(String),
    #[error(transparent)]
    Pos(#[from] pos::PosError),
}

impl SolveError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        SolveError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
