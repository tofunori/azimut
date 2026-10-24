//! Absolute accuracy of Azimut's PPK step against an independent reference.
//!
//! NGS CORS station MISR is processed as a moving rover, epoch by epoch, against CORS MILS
//! 4.8 km away, with Azimut's DJI options file run by RTKLIB-EX. Each epoch is compared with
//! MISR's published ITRF2020 position, which NGS computed from years of data with its own
//! software: the reference does not depend on RTKLIB. Both stations carry the same antenna
//! model, so phase-centre offsets cancel and the solution is MISR's antenna reference point.
//! Data and sources: `tests/data/cors-2025-180/README.md`; results: docs/validation.md.
//!
//! Needs the RTKLIB-EX programs (`--rtklib` lookup: `AZIMUT_RTKLIB_DIR`, then the `PATH`).
//! Without them the test is skipped, unless `AZIMUT_REQUIRE_RTKLIB` is set (as in CI).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use azimut_solve::config::BasePosition;
use azimut_solve::geodesy::{cov_ecef_to_enu, ecef_to_geodetic, enu_basis};
use azimut_solve::pos::{self, SolQuality};
use azimut_solve::{BaseSetup, PpkConfig, Rtklib};

/// Mid-window epoch, 2025-06-29 15:07 GPST, as a decimal year.
const EPOCH: f64 = 2025.0 + (179.0 + 15.125 / 24.0) / 365.0;
/// Epochs in the fixture: 15 minutes at 1 s.
const EPOCHS: usize = 900;

/// ITRF2020 antenna reference point at epoch 2020.0 and velocity (m, m/yr), from the NGS
/// coordinate files `mils_20.coord.txt` and `misr_20.coord.txt`.
const MILS: ([f64; 3], [f64; 3]) = (
    [432018.495, -4378088.743, 4602817.609],
    [-0.0170, -0.0018, 0.0010],
);
const MISR: ([f64; 3], [f64; 3]) = (
    [436821.286, -4377675.056, 4602701.797],
    [-0.0163, -0.0014, -0.0002],
);

/// Acceptance thresholds. A dual-frequency RTK receiver is specified at 8 mm + 1 ppm
/// horizontal and 15 mm + 1 ppm vertical (1-sigma, typical manufacturer figures); at 4.8 km
/// that is 1.3 cm and 2.0 cm.
const MIN_FIX_RATE: f64 = 0.90;
/// Mean error of the fixed epochs, per axis: a bias here would be a setup error (frame,
/// base position, antenna) that every flight would carry.
const MAX_BIAS_H_M: f64 = 0.010;
const MAX_BIAS_U_M: f64 = 0.020;
/// Root mean square of the fixed epochs' errors.
const MAX_RMS_H_M: f64 = 0.015;
const MAX_RMS_U_M: f64 = 0.030;
/// 95th percentile of the fixed epochs' errors.
const MAX_P95_H_M: f64 = 0.025;
const MAX_P95_U_M: f64 = 0.050;

fn at_epoch((xyz, v): ([f64; 3], [f64; 3])) -> [f64; 3] {
    [0, 1, 2].map(|i| xyz[i] + v[i] * (EPOCH - 2020.0))
}

fn gunzip(src: &Path, dir: &Path) -> PathBuf {
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(src).unwrap())
        .read_to_end(&mut text)
        .unwrap();
    let out = dir.join(src.file_stem().unwrap());
    std::fs::write(&out, text).unwrap();
    out
}

fn rtklib() -> Option<Rtklib> {
    match Rtklib::locate(None) {
        Ok(r) => Some(r),
        Err(e) if std::env::var_os("AZIMUT_REQUIRE_RTKLIB").is_some() => panic!("{e}"),
        Err(_) => {
            eprintln!("skipped: RTKLIB-EX not found (set AZIMUT_RTKLIB_DIR to run this test)");
            None
        }
    }
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

#[test]
fn cors_baseline_matches_published_coordinates() {
    let Some(rtklib) = rtklib() else {
        return;
    };
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/cors-2025-180");
    let tmp = tempfile::tempdir().unwrap();
    let [base_obs, rover_obs, gps_nav, glo_nav] = [
        "mils180p.25o.gz",
        "misr180p.25o.gz",
        "brdc1800.25n.gz",
        "brdc1800.25g.gz",
    ]
    .map(|f| gunzip(&data.join(f), tmp.path()));

    // Base held at its published ARP; no antenna type and no height on either station, so
    // the solution is MISR's ARP (same antenna model at both ends).
    let base = BaseSetup {
        position: BasePosition::Ecef {
            xyz_m: at_epoch(MILS),
        },
        antenna_height_m: Some(0.0),
        antenna_type: None,
        frame: format!("ITRF2020, epoch {EPOCH:.3}"),
    };
    let conf = PpkConfig::dji().to_conf(&base);
    let pos_file = rtklib
        .solve(
            &rover_obs,
            &base_obs,
            &[gps_nav, glo_nav],
            &conf,
            tmp.path(),
            "misr",
        )
        .unwrap();
    let traj = pos::read(&pos_file).unwrap();

    let truth = at_epoch(MISR);
    let (lat, lon, _) = ecef_to_geodetic(truth);
    let enu = enu_basis(lat, lon);
    let fixed: Vec<_> = traj.iter().filter(|e| e.quality == SolQuality::Fixed).collect();
    let fix_rate = fixed.len() as f64 / EPOCHS as f64;
    assert!(!fixed.is_empty(), "no fixed epoch out of {}", traj.len());

    let (mut de, mut dn, mut du, mut dh, mut ze, mut zn, mut zu) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    for e in &fixed {
        let d = [0, 1, 2].map(|i| e.xyz[i] - truth[i]);
        let [x, y, z] = enu.map(|r| r[0] * d[0] + r[1] * d[1] + r[2] * d[2]);
        let c = cov_ecef_to_enu(lat, lon, e.cov);
        de.push(x);
        dn.push(y);
        du.push(z);
        dh.push(x.hypot(y));
        // Error over RTKLIB's own 1-sigma: about 1 if its uncertainties are realistic.
        // RTKLIB writes 0 for a negative variance: such epochs have no usable sigma.
        if c[0][0] > 0.0 && c[1][1] > 0.0 && c[2][2] > 0.0 {
            ze.push((x / c[0][0].sqrt()).abs());
            zn.push((y / c[1][1].sqrt()).abs());
            zu.push((z / c[2][2].sqrt()).abs());
        }
    }
    let (bias_e, bias_n, bias_u) = (mean(&de), mean(&dn), mean(&du));
    let (rms_h, rms_u) = ((rms(&de).powi(2) + rms(&dn).powi(2)).sqrt(), rms(&du));
    let p95_h = percentile(&mut dh, 0.95);
    let p95_u = percentile(&mut du.iter().map(|x| x.abs()).collect::<Vec<_>>(), 0.95);
    let max_h = dh.last().copied().unwrap_or(0.0);
    let max_u = du.iter().fold(0.0_f64, |m, x| m.max(x.abs()));
    let cm = |m: f64| format!("{:+.2} cm", m * 100.0);
    println!("MISR against MILS (4.8 km), 2025-06-29 15:00-15:15 GPST, 1 s");
    println!(
        "epochs: {} solved, {} fixed of {EPOCHS} ({:.1} %)",
        traj.len(),
        fixed.len(),
        fix_rate * 100.0
    );
    println!(
        "mean error E / N / U: {} / {} / {}",
        cm(bias_e),
        cm(bias_n),
        cm(bias_u)
    );
    println!(
        "RMS H / U: {} / {}; 95th percentile H / U: {} / {}; max H / U: {} / {}",
        cm(rms_h),
        cm(rms_u),
        cm(p95_h),
        cm(p95_u),
        cm(max_h),
        cm(max_u)
    );
    println!(
        "median |error| / RTKLIB 1-sigma E / N / U ({} epochs with a usable covariance): {:.1} / {:.1} / {:.1}",
        ze.len(),
        percentile(&mut ze, 0.5),
        percentile(&mut zn, 0.5),
        percentile(&mut zu, 0.5)
    );

    assert!(fix_rate >= MIN_FIX_RATE, "fix rate {fix_rate:.3}");
    assert!(bias_e.abs() <= MAX_BIAS_H_M, "east bias {}", cm(bias_e));
    assert!(bias_n.abs() <= MAX_BIAS_H_M, "north bias {}", cm(bias_n));
    assert!(bias_u.abs() <= MAX_BIAS_U_M, "up bias {}", cm(bias_u));
    assert!(rms_h <= MAX_RMS_H_M, "horizontal RMS {}", cm(rms_h));
    assert!(rms_u <= MAX_RMS_U_M, "vertical RMS {}", cm(rms_u));
    assert!(p95_h <= MAX_P95_H_M, "horizontal 95th percentile {}", cm(p95_h));
    assert!(p95_u <= MAX_P95_U_M, "vertical 95th percentile {}", cm(p95_u));
}
