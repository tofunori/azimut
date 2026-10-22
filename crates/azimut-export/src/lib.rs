//! # azimut-export
//!
//! Text outputs for camera positions. Images are never modified.
//!
//! - [`positions_csv`]: one row per image file (all Mavic 3M bands included), with
//!   1-sigma East/North/Up and the reference frame. Imports into Metashape and Pix4D.
//! - [`odm_geo_txt`]: OpenDroneMap `geo.txt` (EPSG:4326, ellipsoidal heights).
//! - [`problems_csv`]: exposures without a camera centre or without an uncertainty, and why.
//!
//! An uncertainty that RTKLIB could not give (covariance not positive definite) is left
//! empty, never written as 0 m: photogrammetry software would read 0 as a perfect position.

use std::fmt::Write as _;

use azimut_io::mrk::SECONDS_PER_WEEK;
use azimut_solve::FlightSolution;

/// Which files get a row.
#[derive(Debug, Clone, Default)]
pub enum RowFilter {
    /// Every image file.
    #[default]
    All,
    /// Only files whose role is listed (`jpeg`, `Green`, `Red`, `RedEdge`, `NIR`, `tiff`).
    /// `--roles Green` gives one row per capture for a Metashape multi-camera master band.
    Roles(Vec<String>),
}

impl RowFilter {
    fn keep(&self, role: &str) -> bool {
        match self {
            RowFilter::All => true,
            RowFilter::Roles(r) => r.iter().any(|x| x.eq_ignore_ascii_case(role)),
        }
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// CSV with one row per image file that has a camera centre.
pub fn positions_csv(sol: &FlightSolution, filter: &RowFilter) -> String {
    let mut s = String::from(
        "file,photo,role,gps_week,gps_tow,latitude,longitude,ellipsoidal_height,sigma_east,sigma_north,sigma_up,quality,satellites,frame\n",
    );
    let frame = csv_field(&sol.frame);
    for e in &sol.exposures {
        let (Some(c), Some(t)) = (e.camera, e.gps_s) else {
            continue;
        };
        let week = (t / SECONDS_PER_WEEK).floor();
        let tow = t - week * SECONDS_PER_WEEK;
        let sigma = c
            .sigma_enu_m
            .map(|[e, n, u]| format!("{e:.4},{n:.4},{u:.4}"))
            .unwrap_or_else(|| ",,".into());
        for f in e.files.iter().filter(|f| filter.keep(&f.role)) {
            let _ = writeln!(
                s,
                "{},{},{},{},{:.6},{:.10},{:.10},{:.4},{},{},{},{}",
                csv_field(&f.file_name),
                e.index.map(|i| i.to_string()).unwrap_or_default(),
                f.role,
                week as u32,
                tow,
                c.lat_deg,
                c.lon_deg,
                c.h_m,
                sigma,
                c.quality.label(),
                c.n_sats.map(|n| n.to_string()).unwrap_or_default(),
                frame
            );
        }
    }
    s
}

/// OpenDroneMap `geo.txt`: projection line, then per file
/// `image lon lat height [yaw pitch roll horizontal_accuracy vertical_accuracy]`.
///
/// ODM uses the camera angles for radiometric calibration, so angles and accuracies are
/// written only when the image carries DJI gimbal angles; zeros are never invented.
/// Horizontal accuracy is the 1-sigma radial (sqrt(sE² + sN²)), vertical is 1-sigma Up.
/// Without an uncertainty, the accuracy columns are left out and only the angles written.
pub fn odm_geo_txt(sol: &FlightSolution, filter: &RowFilter) -> String {
    let mut s = String::from("EPSG:4326\n");
    for e in &sol.exposures {
        let Some(c) = e.camera else { continue };
        for f in e.files.iter().filter(|f| filter.keep(&f.role)) {
            let _ = write!(
                s,
                "{} {:.10} {:.10} {:.4}",
                f.file_name, c.lon_deg, c.lat_deg, c.h_m
            );
            if let Some([y, p, r]) = f.gimbal_ypr_deg {
                let _ = write!(s, " {y:.2} {p:.2} {r:.2}");
                if let Some([se, sn, su]) = c.sigma_enu_m {
                    let _ = write!(s, " {:.4} {su:.4}", (se * se + sn * sn).sqrt());
                }
            }
            s.push('\n');
        }
    }
    s
}

/// Exposures without a camera centre, or with one but no uncertainty.
pub fn problems_csv(sol: &FlightSolution) -> String {
    let mut s = String::from("photo,files,problem\n");
    for e in sol.exposures.iter().filter(|e| e.problem.is_some()) {
        let files: Vec<&str> = e.files.iter().map(|f| f.file_name.as_str()).collect();
        let _ = writeln!(
            s,
            "{},{},{}",
            e.index.map(|i| i.to_string()).unwrap_or_default(),
            csv_field(&files.join(" ")),
            csv_field(e.problem.as_deref().unwrap_or(""))
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use azimut_solve::camera::CameraCentre;
    use azimut_solve::pipeline::{ExposureSolution, FileRef, Method, SolutionStats};
    use azimut_solve::pos::SolQuality;
    use std::path::PathBuf;

    fn sample() -> FlightSolution {
        let cam = CameraCentre {
            xyz: [0.0; 3],
            lat_deg: 46.35,
            lon_deg: -72.55,
            h_m: 119.82,
            sigma_enu_m: Some([0.012, 0.011, 0.025]),
            quality: SolQuality::Fixed,
            n_sats: Some(21),
            height_check_m: 0.0,
        };
        let file = |n: &str, r: &str| FileRef {
            file_name: n.into(),
            path: PathBuf::from(n),
            role: r.into(),
            gimbal_ypr_deg: (r == "jpeg").then_some([0.5, -90.0, 0.0]),
        };
        FlightSolution {
            dir: PathBuf::from("f"),
            method: Method::Ppk {
                rover_obs: PathBuf::new(),
                base_obs: PathBuf::new(),
                navs: vec![],
                pos_file: PathBuf::new(),
            },
            frame: "NAD83(CSRS) v8, epoch 2010.0".into(),
            exposures: vec![
                ExposureSolution {
                    index: Some(1),
                    gps_s: Some(2420.0 * SECONDS_PER_WEEK + 10.5),
                    files: vec![file("A_D.JPG", "jpeg"), file("A_MS_G.TIF", "Green")],
                    camera: Some(cam),
                    problem: None,
                },
                ExposureSolution {
                    index: Some(2),
                    gps_s: Some(2420.0 * SECONDS_PER_WEEK + 12.5),
                    files: vec![file("B_D.JPG", "jpeg")],
                    camera: None,
                    problem: Some("exposure outside the PPK trajectory".into()),
                },
            ],
            stats: SolutionStats::default(),
        }
    }

    #[test]
    fn csv_rows_and_frame_quoting() {
        let s = positions_csv(&sample(), &RowFilter::All);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1]
            .starts_with("A_D.JPG,1,jpeg,2420,10.500000,46.3500000000,-72.5500000000,119.8200,0.0120"));
        assert!(lines[1].ends_with(",fixed,21,\"NAD83(CSRS) v8, epoch 2010.0\""));
        assert!(lines[2].starts_with("A_MS_G.TIF,1,Green,"));
    }

    #[test]
    fn role_filter_and_geo_txt() {
        let s = positions_csv(&sample(), &RowFilter::Roles(vec!["green".into()]));
        assert_eq!(s.lines().count(), 2);
        let g = odm_geo_txt(&sample(), &RowFilter::All);
        assert_eq!(g.lines().next(), Some("EPSG:4326"));
        // Angles and accuracies only where the image carries gimbal angles.
        assert_eq!(
            g.lines().nth(1),
            Some("A_D.JPG -72.5500000000 46.3500000000 119.8200 0.50 -90.00 0.00 0.0163 0.0250")
        );
        assert_eq!(
            g.lines().nth(2),
            Some("A_MS_G.TIF -72.5500000000 46.3500000000 119.8200")
        );
    }

    #[test]
    fn problems_are_listed() {
        let p = problems_csv(&sample());
        assert!(p.contains("2,B_D.JPG,exposure outside the PPK trajectory"));
    }

    #[test]
    fn missing_uncertainty_is_left_empty_not_zero() {
        let mut sol = sample();
        let e = &mut sol.exposures[0];
        e.camera.as_mut().unwrap().sigma_enu_m = None;
        e.problem = Some("RTKLIB covariance not positive definite; uncertainty left empty".into());
        let csv = positions_csv(&sol, &RowFilter::All);
        assert!(csv.lines().nth(1).unwrap().contains(",119.8200,,,,fixed,21,"));
        let g = odm_geo_txt(&sol, &RowFilter::All);
        assert_eq!(
            g.lines().nth(1),
            Some("A_D.JPG -72.5500000000 46.3500000000 119.8200 0.50 -90.00 0.00"),
            "angles kept, accuracies left out"
        );
        assert!(problems_csv(&sol).contains("1,A_D.JPG A_MS_G.TIF,RTKLIB covariance not positive definite"));
    }
}
