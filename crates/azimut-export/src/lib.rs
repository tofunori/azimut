//! # azimut-export
//!
//! Outputs for camera positions. Images are modified only by [`images`], on request.
//!
//! - [`positions_csv`]: one row per image file (all Mavic 3M bands included), with
//!   1-sigma East/North/Up and the reference frame. Imports into Metashape and Pix4D.
//!   [`positions_csv_with_heights`] adds orthometric heights from a geoid grid (CGVD2013).
//!
//! Each file gets its own position and time: Mavic 3M bands are exposed after the RGB JPEG
//! that the MRK times, so their row differs from the JPEG's.
//! - [`odm_geo_txt`]: OpenDroneMap `geo.txt` (EPSG:4326, ellipsoidal heights).
//! - [`pix4dmatic_csv`]: PIX4Dmatic image geolocation file (positions and accuracies, no
//!   angles, so PIX4Dmatic keeps the orientation read from the images).
//! - [`problems_csv`]: exposures without a camera centre or without an uncertainty, and why.
//! - [`images`]: positions written into the images themselves (EXIF GPS and DJI XMP), in
//!   place and only on request, with the original bytes kept for undoing.
//! - [`report`]: quality report of a flight, as JSON and as a self-contained HTML page.
//!
//! An uncertainty that RTKLIB could not give (covariance not positive definite) is left
//! empty, never written as 0 m: photogrammetry software would read 0 as a perfect position.

use std::fmt::Write as _;

pub mod images;
pub mod report;

use azimut_geo::GeoidGrid;
use azimut_io::mrk::SECONDS_PER_WEEK;
use azimut_solve::FlightSolution;

/// Which files get a row.
#[derive(Debug, Clone, Default)]
pub enum RowFilter {
    /// Every image file.
    #[default]
    All,
    /// Only files whose role is listed (`jpeg`, `Green`, `Red`, `RedEdge`, `NIR`, `ms_jpeg`,
    /// `tiff`).
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
    positions_csv_with_heights(sol, filter, None)
}

/// [`positions_csv`] plus `orthometric_height` (H = h - N) and `vertical_datum` columns from
/// a geoid grid. The grid must apply to the solution's frame (see
/// [`GeoidGrid::check_frame`]); outside the grid the height is left empty.
pub fn positions_csv_with_heights(
    sol: &FlightSolution,
    filter: &RowFilter,
    geoid: Option<&GeoidGrid>,
) -> String {
    let mut s = String::from(
        "file,photo,role,gps_week,gps_tow,latitude,longitude,ellipsoidal_height,sigma_east,sigma_north,sigma_up,quality,satellites,frame",
    );
    s.push_str(if geoid.is_some() {
        ",orthometric_height,vertical_datum\n"
    } else {
        "\n"
    });
    let frame = csv_field(&sol.frame);
    let datum = geoid.map(|g| csv_field(g.vertical_datum()));
    for e in &sol.exposures {
        let Some(t0) = e.gps_s else {
            continue;
        };
        for f in e.files.iter().filter(|f| filter.keep(&f.role)) {
            let Some(c) = f.camera else {
                continue;
            };
            let t = t0 + f.time_offset_s.unwrap_or(0.0);
            let week = (t / SECONDS_PER_WEEK).floor();
            let tow = t - week * SECONDS_PER_WEEK;
            let sigma = c
                .sigma_enu_m
                .map(|[e, n, u]| format!("{e:.4},{n:.4},{u:.4}"))
                .unwrap_or_else(|| ",,".into());
            let _ = write!(
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
            if let (Some(g), Some(datum)) = (geoid, &datum) {
                let h = g
                    .orthometric_height(c.lat_deg, c.lon_deg, c.h_m)
                    .map(|h| format!("{h:.4}"))
                    .unwrap_or_default();
                let _ = write!(s, ",{h},{datum}");
            }
            s.push('\n');
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
        for f in e.files.iter().filter(|f| filter.keep(&f.role)) {
            let Some(c) = f.camera else { continue };
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

/// PIX4Dmatic image geolocation file, in its documented "geographic coordinates with
/// accuracy values" layout: no header (one `#` comment line), then
/// `image,latitude,longitude,z,accuracy horizontal,accuracy vertical`, latitude first as in
/// geographic EPSG definitions. Accuracies are 1-sigma: horizontal radial
/// sqrt(sE² + sN²), vertical sU. `z` is the ellipsoidal height, or the orthometric height
/// with a geoid grid. Files without an uncertainty (or outside the grid) get no row, so that
/// PIX4Dmatic keeps the values read from the image. Returns the text and the number of
/// files left out.
pub fn pix4dmatic_csv(
    sol: &FlightSolution,
    filter: &RowFilter,
    geoid: Option<&GeoidGrid>,
) -> (String, usize) {
    let height = match geoid {
        Some(g) => format!("orthometric height {} (m)", g.vertical_datum()),
        None => "ellipsoidal height (m)".into(),
    };
    let mut s = format!(
        "#image,latitude,longitude,{height},accuracy horizontal (m),accuracy vertical (m); frame: {}\n",
        sol.frame.replace('\n', " ")
    );
    let mut left_out = 0;
    for f in sol
        .exposures
        .iter()
        .flat_map(|e| &e.files)
        .filter(|f| filter.keep(&f.role))
    {
        let Some(c) = f.camera else { continue };
        let z = match geoid {
            Some(g) => g.orthometric_height(c.lat_deg, c.lon_deg, c.h_m),
            None => Some(c.h_m),
        };
        let (Some([se, sn, su]), Some(z)) = (c.sigma_enu_m, z) else {
            left_out += 1;
            continue;
        };
        let _ = writeln!(
            s,
            "{},{:.10},{:.10},{:.4},{:.4},{:.4}",
            f.file_name,
            c.lat_deg,
            c.lon_deg,
            z,
            (se * se + sn * sn).sqrt(),
            su
        );
    }
    (s, left_out)
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
        let file = |n: &str, r: &str, camera: Option<CameraCentre>| FileRef {
            file_name: n.into(),
            path: PathBuf::from(n),
            role: r.into(),
            gimbal_ypr_deg: (r == "jpeg").then_some([0.5, -90.0, 0.0]),
            time_offset_s: Some(0.0),
            camera,
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
                    files: vec![
                        file("A_D.JPG", "jpeg", Some(cam)),
                        file("A_MS_G.TIF", "Green", Some(cam)),
                    ],
                    camera: Some(cam),
                    problem: None,
                },
                ExposureSolution {
                    index: Some(2),
                    gps_s: Some(2420.0 * SECONDS_PER_WEEK + 12.5),
                    files: vec![file("B_D.JPG", "jpeg", None)],
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
    fn band_rows_have_their_own_time_and_position() {
        let mut sol = sample();
        let band = &mut sol.exposures[0].files[1];
        band.time_offset_s = Some(0.0264);
        band.camera.as_mut().unwrap().lat_deg = 46.3500009;
        let csv = positions_csv(&sol, &RowFilter::All);
        assert!(csv.lines().nth(1).unwrap().contains(",10.500000,46.3500000000,"));
        assert!(csv.lines().nth(2).unwrap().contains(",10.526400,46.3500009000,"));
        let g = odm_geo_txt(&sol, &RowFilter::All);
        assert!(g
            .lines()
            .nth(2)
            .unwrap()
            .starts_with("A_MS_G.TIF -72.5500000000 46.3500009000 "));
        // A file that could not be positioned at its own time gets no row.
        sol.exposures[0].files[1].camera = None;
        assert_eq!(positions_csv(&sol, &RowFilter::All).lines().count(), 2);
    }

    #[test]
    fn orthometric_heights_from_a_geoid_grid() {
        use azimut_geo::synthetic::SyntheticGrid;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("geoid.tif");
        SyntheticGrid::default().write(&path).unwrap();
        let g = GeoidGrid::open(&path).unwrap();
        let mut sol = sample();
        // The band lies outside the grid: its height is left empty.
        sol.exposures[0].files[1].camera.as_mut().unwrap().lat_deg = 47.5;
        let csv = positions_csv_with_heights(&sol, &RowFilter::All, Some(&g));
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].ends_with(",frame,orthometric_height,vertical_datum"));
        let h = 119.82 - SyntheticGrid::expected(46.35, -72.55);
        assert!(
            lines[1].ends_with(&format!(",{h:.4},CGVD2013(CGG2013a) height (EPSG:9245)")),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].ends_with(",,CGVD2013(CGG2013a) height (EPSG:9245)"),
            "{}",
            lines[2]
        );
        // Without a grid the columns are not written.
        assert!(positions_csv(&sol, &RowFilter::All)
            .lines()
            .next()
            .unwrap()
            .ends_with(",frame"));
    }

    #[test]
    fn pix4dmatic_rows_follow_the_documented_layout() {
        let mut sol = sample();
        let (csv, left_out) = pix4dmatic_csv(&sol, &RowFilter::All, None);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with('#'), "single comment line, no header");
        assert!(lines[0].contains("NAD83(CSRS) v8, epoch 2010.0"));
        assert_eq!(
            lines[1], "A_D.JPG,46.3500000000,-72.5500000000,119.8200,0.0163,0.0250",
            "image, latitude, longitude, z, 1-sigma horizontal radial, vertical"
        );
        assert_eq!(lines.len(), 3, "the exposure without a position has no row");
        assert_eq!(left_out, 0);

        // No uncertainty: no row, so PIX4Dmatic keeps the image's own values.
        sol.exposures[0].files[1].camera.as_mut().unwrap().sigma_enu_m = None;
        let (csv, left_out) = pix4dmatic_csv(&sol, &RowFilter::All, None);
        assert_eq!(csv.lines().count(), 2);
        assert_eq!(left_out, 1);
    }

    #[test]
    fn report_lists_points_to_check() {
        use crate::report::FlightReport;
        let sol = sample();
        let r = FlightReport::build(&sol, None, None, None);
        assert_eq!(r.summary.exposures, sol.stats.exposures);
        assert_eq!(r.summary.files, 3);
        assert_eq!(r.summary.files_positioned, 2);
        assert_eq!(r.exposures.len(), 2);
        assert_eq!(r.exposures[0].quality.as_deref(), Some("fixed"));
        assert_eq!(r.summary.sigma_median_enu_m, Some([0.012, 0.011, 0.025]));
        let html = r.html();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("exposure outside the PPK trajectory"));
        assert!(
            html.contains("<g class=\"mark fixed\">"),
            "status by shape and colour"
        );
        assert!(html.contains("class=\"chart\""), "uncertainty chart");
        assert!(!html.contains("<script"), "self-contained, no scripts");
        let fr = r.html_in(crate::report::Lang::Fr);
        assert!(fr.contains("<html lang=\"fr\">") && fr.contains("Rapport de qualité"));
        assert!(fr.contains("46,35000000"), "French decimal comma");
        let json = serde_json::to_string(&r).unwrap();
        let back: FlightReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.exposures.len(), 2);
    }

    #[test]
    fn report_stays_readable_with_many_captures() {
        use crate::report::FlightReport;
        let mut sol = sample();
        let template = sol.exposures[0].clone();
        sol.exposures = (0..300)
            .map(|i| {
                let mut e = template.clone();
                e.index = Some(i + 1);
                e.gps_s = e.gps_s.map(|g| g + i as f64 * 2.0);
                e
            })
            .collect();
        let html = FlightReport::build(&sol, None, None, None).html();
        assert!(html.contains("class=\"map dense\""), "smaller marks");
        assert_eq!(
            html.matches("<circle class=\"s").count(),
            3,
            "one end marker per sigma series, not one per capture"
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
        for f in &mut e.files {
            f.camera.as_mut().unwrap().sigma_enu_m = None;
        }
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
