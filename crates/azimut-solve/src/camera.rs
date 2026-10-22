//! From antenna trajectory to camera centre.
//!
//! The antenna position is interpolated to the exposure time, then the MRK lever arm (from
//! antenna phase centre to camera CMOS centre, North-East-Down, millimetres) is rotated to
//! ECEF at the antenna's position and added. Nothing is extrapolated. See SOURCES.md.
//!
//! RTKLIB can write covariances that are not positive definite (it prints 0 for a negative
//! variance). Such an uncertainty is dropped, never reported as 0 m.

use serde::{Deserialize, Serialize};

use crate::geodesy::{cov_ecef_to_enu, ecef_to_geodetic, ned_to_ecef};
use crate::pos::{PosEpoch, SolQuality};

/// Antenna position at one instant.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AntennaFix {
    pub xyz: [f64; 3],
    /// ECEF covariance, m². `None` when a bracketing epoch's covariance is not positive
    /// definite.
    pub cov: Option<[[f64; 3]; 3]>,
    /// Worse status of the bracketing epochs.
    pub quality: SolQuality,
    pub n_sats: u32,
    /// Time between the bracketing epochs, seconds (0 when an epoch matches exactly).
    pub gap_s: f64,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum InterpError {
    #[error("exposure outside the PPK trajectory")]
    Outside,
    #[error("gap of {0:.2} s in the PPK trajectory around the exposure")]
    Gap(f64),
}

/// Interpolate the antenna position at `t` (seconds since the GPS epoch).
///
/// Position is linear between the bracketing epochs. Covariance is the time-weighted mean
/// of the two (not reduced, since successive filter epochs are strongly correlated).
pub fn antenna_at(traj: &[PosEpoch], t: f64, max_gap_s: f64) -> Result<AntennaFix, InterpError> {
    const SAME: f64 = 1e-6;
    let i = traj.partition_point(|e| e.gps_s < t - SAME);
    if let Some(e) = traj.get(i).filter(|e| (e.gps_s - t).abs() <= SAME) {
        return Ok(AntennaFix {
            xyz: e.xyz,
            cov: is_positive_definite(&e.cov).then_some(e.cov),
            quality: e.quality,
            n_sats: e.n_sats,
            gap_s: 0.0,
        });
    }
    if i == 0 || i >= traj.len() {
        return Err(InterpError::Outside);
    }
    let (a, b) = (&traj[i - 1], &traj[i]);
    let gap = b.gps_s - a.gps_s;
    if gap > max_gap_s {
        return Err(InterpError::Gap(gap));
    }
    let w = (t - a.gps_s) / gap;
    let xyz = [0, 1, 2].map(|k| a.xyz[k] + w * (b.xyz[k] - a.xyz[k]));
    // A mean with an invalid covariance can look valid, so both epochs must be valid.
    let cov = (is_positive_definite(&a.cov) && is_positive_definite(&b.cov)).then(|| {
        let mut cov = [[0.0; 3]; 3];
        for (r, row) in cov.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = (1.0 - w) * a.cov[r][c] + w * b.cov[r][c];
            }
        }
        cov
    });
    let quality = if a.quality.rank() >= b.quality.rank() {
        a.quality
    } else {
        b.quality
    };
    Ok(AntennaFix {
        xyz,
        cov,
        quality,
        n_sats: a.n_sats.min(b.n_sats),
        gap_s: gap,
    })
}

/// Whether a 3x3 covariance is finite and positive definite (leading principal minors > 0).
pub fn is_positive_definite(c: &[[f64; 3]; 3]) -> bool {
    let m1 = c[0][0];
    let m2 = c[0][0] * c[1][1] - c[0][1] * c[1][0];
    let m3 = c[0][0] * (c[1][1] * c[2][2] - c[1][2] * c[2][1])
        - c[0][1] * (c[1][0] * c[2][2] - c[1][2] * c[2][0])
        + c[0][2] * (c[1][0] * c[2][1] - c[1][1] * c[2][0]);
    c.iter().flatten().all(|v| v.is_finite()) && m1 > 0.0 && m2 > 0.0 && m3 > 0.0
}

/// Camera centre for one exposure.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraCentre {
    pub xyz: [f64; 3],
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Ellipsoidal height, metres.
    pub h_m: f64,
    /// 1-sigma East, North, Up, metres (from the PPK covariance only). `None` when RTKLIB's
    /// covariance is not positive definite.
    pub sigma_enu_m: Option<[f64; 3]>,
    pub quality: SolQuality,
    /// Satellites used, when the source gives it (the MRK does not).
    pub n_sats: Option<u32>,
    /// Antenna height minus the Down offset, minus camera height. Should be ~0; 0 when no
    /// lever arm was applied.
    pub height_check_m: f64,
}

/// Apply the MRK lever arm to an antenna position.
pub fn camera_centre(ant: &AntennaFix, offset_ned_mm: [f64; 3]) -> CameraCentre {
    let (lat_a, lon_a, h_a) = ecef_to_geodetic(ant.xyz);
    let ned_m = offset_ned_mm.map(|v| v / 1000.0);
    let d = ned_to_ecef(lat_a, lon_a, ned_m);
    let xyz = [0, 1, 2].map(|k| ant.xyz[k] + d[k]);
    let (lat, lon, h) = ecef_to_geodetic(xyz);
    CameraCentre {
        xyz,
        lat_deg: lat,
        lon_deg: lon,
        h_m: h,
        sigma_enu_m: ant.cov.map(|cov| {
            let enu = cov_ecef_to_enu(lat, lon, cov);
            [0, 1, 2].map(|k| enu[k][k].sqrt())
        }),
        quality: ant.quality,
        n_sats: Some(ant.n_sats),
        height_check_m: (h_a - ned_m[2]) - h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geodesy::geodetic_to_ecef;

    fn epoch(t: f64, xyz: [f64; 3], q: SolQuality, s: f64) -> PosEpoch {
        PosEpoch {
            gps_s: t,
            xyz,
            quality: q,
            n_sats: 20,
            cov: [[s * s, 0.0, 0.0], [0.0, s * s, 0.0], [0.0, 0.0, s * s]],
            age_s: 1.0,
            ratio: 10.0,
        }
    }

    #[test]
    fn interpolates_between_epochs() {
        let a = geodetic_to_ecef(46.35, -72.55, 120.0);
        let b = [a[0] + 2.0, a[1], a[2]];
        let traj = vec![
            epoch(100.0, a, SolQuality::Fixed, 0.01),
            epoch(100.2, b, SolQuality::Float, 0.03),
        ];
        let f = antenna_at(&traj, 100.05, 1.0).unwrap();
        assert!((f.xyz[0] - (a[0] + 0.5)).abs() < 1e-9);
        assert_eq!(f.quality, SolQuality::Float, "the worse status is kept");
        assert!((f.cov.unwrap()[0][0] - (0.75 * 1e-4 + 0.25 * 9e-4)).abs() < 1e-15);
    }

    #[test]
    fn exact_epoch_and_refusals() {
        let a = geodetic_to_ecef(46.35, -72.55, 120.0);
        let traj = vec![
            epoch(100.0, a, SolQuality::Fixed, 0.01),
            epoch(103.0, a, SolQuality::Fixed, 0.01),
        ];
        assert_eq!(antenna_at(&traj, 100.0, 1.0).unwrap().gap_s, 0.0);
        assert_eq!(antenna_at(&traj, 99.0, 1.0), Err(InterpError::Outside));
        assert_eq!(antenna_at(&traj, 104.0, 1.0), Err(InterpError::Outside));
        assert_eq!(antenna_at(&traj, 101.0, 1.0), Err(InterpError::Gap(3.0)));
    }

    #[test]
    fn lever_arm_moves_camera_below_and_beside_antenna() {
        let ant = AntennaFix {
            xyz: geodetic_to_ecef(46.35, -72.55, 120.0),
            cov: Some([[1e-4, 0.0, 0.0], [0.0, 1e-4, 0.0], [0.0, 0.0, 4e-4]]),
            quality: SolQuality::Fixed,
            n_sats: 20,
            gap_s: 0.2,
        };
        // 0.106 m north, 0.556 m west, 0.300 m below (example from the dji-geotagger docs).
        let c = camera_centre(&ant, [106.0, -556.0, 300.0]);
        assert!((c.h_m - (120.0 - 0.300)).abs() < 1e-4);
        assert!(c.height_check_m.abs() < 1e-4);
        assert!(c.lat_deg > 46.35 && c.lon_deg < -72.55);
        let horiz = ((c.xyz[0] - ant.xyz[0]).powi(2)
            + (c.xyz[1] - ant.xyz[1]).powi(2)
            + (c.xyz[2] - ant.xyz[2]).powi(2))
        .sqrt();
        let expected = (0.106f64.powi(2) + 0.556f64.powi(2) + 0.3f64.powi(2)).sqrt();
        assert!((horiz - expected).abs() < 1e-9);
        assert!(c.sigma_enu_m.unwrap().iter().all(|s| *s > 0.0));
    }

    #[test]
    fn invalid_covariance_gives_no_sigma() {
        let a = geodetic_to_ecef(46.35, -72.55, 120.0);
        // As RTKLIB kinematic solutions were seen to write: negative variance printed as 0,
        // with a non-zero cross term.
        let mut bad = epoch(100.2, a, SolQuality::Fixed, 0.01);
        bad.cov = [[0.0, 0.63, 0.0], [0.63, 2.9e-4, -0.45], [0.0, -0.45, 0.0]];
        assert!(!is_positive_definite(&bad.cov));
        let traj = vec![epoch(100.0, a, SolQuality::Fixed, 0.01), bad];
        assert!(antenna_at(&traj, 100.0, 1.0).unwrap().cov.is_some());
        let exact = antenna_at(&traj, 100.2, 1.0).unwrap();
        assert_eq!(exact.cov, None);
        let between = antenna_at(&traj, 100.1, 1.0).unwrap();
        assert_eq!(between.cov, None, "one invalid bracketing epoch is enough");
        let c = camera_centre(&between, [0.0, 0.0, 300.0]);
        assert_eq!(c.sigma_enu_m, None);
        assert!(
            (c.h_m - (120.0 - 0.300)).abs() < 1e-4,
            "the position itself is kept"
        );
    }
}
