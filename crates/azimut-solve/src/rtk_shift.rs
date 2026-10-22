//! Real-time (RTK) camera positions moved by a constant shift.
//!
//! An RTK aircraft positions itself relative to the coordinates its base station broadcasts.
//! When those coordinates are off, for example a base that averaged its own single-point
//! position, every RTK position is off by the same vector. Moving all of them by the
//! difference between the correct base position (from a PPP solution, say) and the
//! broadcast one removes that offset. The MRK positions are already camera (CMOS) centres,
//! so no lever arm is added. Only fixed and float positions are relative to the base:
//! single-point positions are left without a position. Images are never modified.
//! See SOURCES.md (RTK shift).

use serde::{Deserialize, Serialize};

use azimut_io::mrk::{MrkRecord, RtkQuality};
use azimut_io::scan::Flight;

use crate::camera::CameraCentre;
use crate::geodesy::{ecef_to_geodetic, enu_basis, geodetic_to_ecef};
use crate::pipeline::{exposure_files, finish, ExposureSolution, FlightSolution, Method, SolutionStats};
use crate::pos::SolQuality;

/// The shift to apply.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShiftSpec {
    /// From the base coordinates the aircraft used to the correct coordinates of the same
    /// point: latitude, longitude (degrees), ellipsoidal height (m).
    Points { from_llh: [f64; 3], to_llh: [f64; 3] },
    /// East, North, Up in metres, applied in each camera's local frame.
    Enu { enu_m: [f64; 3] },
}

impl ShiftSpec {
    /// ECEF shift of a position, metres.
    pub fn ecef_delta_at(&self, xyz: [f64; 3]) -> [f64; 3] {
        match *self {
            ShiftSpec::Points { from_llh, to_llh } => {
                let a = geodetic_to_ecef(from_llh[0], from_llh[1], from_llh[2]);
                let b = geodetic_to_ecef(to_llh[0], to_llh[1], to_llh[2]);
                [0, 1, 2].map(|k| b[k] - a[k])
            }
            ShiftSpec::Enu { enu_m } => {
                let (lat, lon, _) = ecef_to_geodetic(xyz);
                let [e, n, u] = enu_basis(lat, lon);
                [0, 1, 2].map(|k| enu_m[0] * e[k] + enu_m[1] * n[k] + enu_m[2] * u[k])
            }
        }
    }

    /// East, North, Up components, metres: at the corrected base for `Points`, as given for
    /// `Enu`.
    pub fn enu_m(&self) -> [f64; 3] {
        match *self {
            ShiftSpec::Points { to_llh, .. } => {
                let d = self.ecef_delta_at([0.0; 3]);
                let r = enu_basis(to_llh[0], to_llh[1]);
                [0, 1, 2].map(|i| (0..3).map(|k| r[i][k] * d[k]).sum())
            }
            ShiftSpec::Enu { enu_m } => enu_m,
        }
    }
}

/// The shift recorded with a solution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RtkShift {
    pub spec: ShiftSpec,
    /// East, North, Up components, metres (see [`ShiftSpec::enu_m`]).
    pub enu_m: [f64; 3],
}

/// Shifted camera centre of one MRK record, or why there is none.
pub fn shifted_camera(rec: &MrkRecord, spec: &ShiftSpec) -> Result<CameraCentre, String> {
    let quality = match rec.quality {
        RtkQuality::Fixed => SolQuality::Fixed,
        RtkQuality::Float => SolQuality::Float,
        RtkQuality::Single => {
            return Err(
                "single-point position in the MRK (no RTK): not relative to the base, \
                        so it cannot be shifted"
                    .into(),
            )
        }
        RtkQuality::NoPosition => return Err("no real-time position in the MRK".into()),
        RtkQuality::Unknown => return Err(format!("unknown MRK status flag {}", rec.flag)),
    };
    let xyz0 = geodetic_to_ecef(rec.lat_deg, rec.lon_deg, rec.ellh_m);
    let d = spec.ecef_delta_at(xyz0);
    let xyz = [0, 1, 2].map(|k| xyz0[k] + d[k]);
    let (lat, lon, h) = ecef_to_geodetic(xyz);
    Ok(CameraCentre {
        xyz,
        lat_deg: lat,
        lon_deg: lon,
        h_m: h,
        // The aircraft's own standard deviations (North, East, Up), relative to the base.
        sigma_enu_m: rec.std_neu_m.map(|[n, e, u]| [e, n, u]),
        quality,
        n_sats: None,
        height_check_m: 0.0,
    })
}

/// Shift every paired exposure of a scanned flight.
pub fn shift_flight(flight: &Flight, spec: &ShiftSpec, frame: &str) -> FlightSolution {
    let mut exposures = Vec::new();
    for pair in &flight.matching.pairs {
        let rec = &flight.mrk[pair.mrk].record;
        let (camera, problem) = match shifted_camera(rec, spec) {
            Ok(c) => {
                let problem = c
                    .sigma_enu_m
                    .is_none()
                    .then(|| "no standard deviations in the MRK; uncertainty left empty".to_string());
                (Some(c), problem)
            }
            Err(e) => (None, Some(e)),
        };
        exposures.push(ExposureSolution {
            index: Some(rec.index),
            gps_s: Some(rec.gps_seconds()),
            files: exposure_files(flight, pair.capture),
            camera,
            problem,
        });
    }
    let (exposures, stats) = finish(flight, exposures, SolutionStats::default());
    FlightSolution {
        dir: flight.dir.clone(),
        method: Method::RtkShift(RtkShift {
            spec: *spec,
            enu_m: spec.enu_m(),
        }),
        frame: frame.to_string(),
        exposures,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(quality: RtkQuality, std: Option<[f64; 3]>) -> MrkRecord {
        MrkRecord {
            index: 7,
            tow: 100.0,
            week: 2420,
            offset_ned_mm: [12.0, -3.0, 180.0],
            lat_deg: 46.35,
            lon_deg: -72.55,
            ellh_m: 120.0,
            std_neu_m: std,
            flag: 50,
            quality,
            line: 1,
        }
    }

    fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
        (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
    }

    #[test]
    fn points_shift_moves_every_camera_by_the_base_correction() {
        // Base broadcast 1.2 m east, 0.8 m south and 0.5 m low of its PPP position.
        let to = [46.3491234, -72.5512345, 31.204];
        let r = enu_basis(to[0], to[1]);
        let t = geodetic_to_ecef(to[0], to[1], to[2]);
        let off = [1.2, -0.8, -0.5];
        let f = [0, 1, 2].map(|k| t[k] + (0..3).map(|i| off[i] * r[i][k]).sum::<f64>());
        let (fl, fo, fh) = ecef_to_geodetic(f);
        let spec = ShiftSpec::Points {
            from_llh: [fl, fo, fh],
            to_llh: to,
        };
        let enu = spec.enu_m();
        for k in 0..3 {
            assert!(
                (enu[k] + off[k]).abs() < 1e-6,
                "shift is the opposite of the error"
            );
        }

        let rec = record(RtkQuality::Fixed, Some([0.01, 0.02, 0.03]));
        let c = shifted_camera(&rec, &spec).unwrap();
        let before = geodetic_to_ecef(rec.lat_deg, rec.lon_deg, rec.ellh_m);
        let moved = dist(before, c.xyz);
        assert!((moved - (1.2f64.powi(2) + 0.8f64.powi(2) + 0.5f64.powi(2)).sqrt()).abs() < 1e-6);
        assert!(
            ((c.h_m - rec.ellh_m) - 0.5).abs() < 1e-3,
            "the camera goes up by 0.5 m"
        );
        assert_eq!(
            c.sigma_enu_m,
            Some([0.02, 0.01, 0.03]),
            "MRK gives North, East, Up"
        );
        assert_eq!(c.n_sats, None);
    }

    #[test]
    fn enu_shift_is_applied_locally() {
        let spec = ShiftSpec::Enu {
            enu_m: [0.0, 1.0, 0.0],
        };
        let rec = record(RtkQuality::Float, None);
        let c = shifted_camera(&rec, &spec).unwrap();
        let north_m = (c.lat_deg - rec.lat_deg).to_radians() * 6_367_000.0;
        assert!((north_m - 1.0).abs() < 0.01);
        assert!((c.lon_deg - rec.lon_deg).abs() < 1e-10);
        assert_eq!(c.quality, SolQuality::Float);
        assert_eq!(c.sigma_enu_m, None);
    }

    #[test]
    fn positions_not_relative_to_the_base_are_not_shifted() {
        let spec = ShiftSpec::Enu { enu_m: [1.0; 3] };
        for q in [RtkQuality::Single, RtkQuality::NoPosition, RtkQuality::Unknown] {
            assert!(shifted_camera(&record(q, None), &spec).is_err());
        }
        let msg = shifted_camera(&record(RtkQuality::Single, None), &spec).unwrap_err();
        assert!(msg.contains("cannot be shifted"));
    }
}
