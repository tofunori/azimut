//! Absolute accuracy of Azimut's PPK step against an independent reference.
//!
//! A U.S. NGS CORS station is processed as a moving rover, epoch by epoch (1 s, 15 minutes),
//! against another CORS station a few kilometres away, with Azimut's DJI options file run by
//! RTKLIB-EX. Each epoch is compared with the rover's published ITRF2020 position, which NGS
//! computed from years of data with its own software: the reference does not depend on
//! RTKLIB. Both stations of a pair carry the same antenna model, so phase-centre offsets
//! cancel and the solution is the rover's antenna reference point (ARP).
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
/// Epochs in each observation file: 15 minutes at 1 s.
const EPOCHS: usize = 900;

/// A station: name, ITRF2020 ARP at epoch 2020.0 and velocity (m, m/yr), from the NGS
/// coordinate file `<name>_20.coord.txt`.
struct Station {
    name: &'static str,
    xyz: [f64; 3],
    vel: [f64; 3],
}

const MILS: Station = Station {
    name: "mils",
    xyz: [432018.495, -4378088.743, 4602817.609],
    vel: [-0.0170, -0.0018, 0.0010],
};
/// Velocity predicted with HTDP, not computed from the station's data (installed 2021).
const MISR: Station = Station {
    name: "misr",
    xyz: [436821.286, -4377675.056, 4602701.797],
    vel: [-0.0163, -0.0014, -0.0002],
};
const MIAL: Station = Station {
    name: "mial",
    xyz: [516445.049, -4483091.758, 4492397.968],
    vel: [-0.0167, -0.0002, -0.0002],
};
const NOR3: Station = Station {
    name: "nor3",
    xyz: [505437.126, -4483924.270, 4492858.235],
    vel: [-0.0168, -0.0006, 0.0002],
};

impl Station {
    fn at_epoch(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| self.xyz[i] + self.vel[i] * (EPOCH - 2020.0))
    }
}

/// Acceptance thresholds, from the typical specification of a dual-frequency RTK receiver:
/// 8 mm + 1 ppm horizontal and 15 mm + 1 ppm vertical, 1-sigma.
const MIN_FIX_RATE: f64 = 0.90;

fn spec_h(baseline_m: f64) -> f64 {
    0.008 + 1e-6 * baseline_m
}

fn spec_u(baseline_m: f64) -> f64 {
    0.015 + 1e-6 * baseline_m
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

fn percentile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

fn std(v: &[f64]) -> f64 {
    let m = mean(v);
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

/// Errors of one baseline, metres, East / North / Up per fixed epoch.
struct Errors {
    label: String,
    baseline_m: f64,
    fix_rate: f64,
    enu: [Vec<f64>; 3],
    /// RTKLIB's own 1-sigma, East / North / Up, for epochs with a usable covariance.
    sigma: [Vec<f64>; 3],
}

/// Process `rover` against `base` and compare with the rover's published position.
fn baseline(rtklib: &Rtklib, base: &Station, rover: &Station) -> Errors {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/cors-2025-180");
    let tmp = tempfile::tempdir().unwrap();
    let file = |f: String| gunzip(&data.join(f), tmp.path());
    let base_obs = file(format!("{}180p.25o.gz", base.name));
    let rover_obs = file(format!("{}180p.25o.gz", rover.name));
    let navs = [file("brdc1800.25n.gz".into()), file("brdc1800.25g.gz".into())];

    // Base held at its published ARP; no antenna type and no height on either station, so
    // the solution is the rover's ARP (same antenna model at both ends).
    let setup = BaseSetup {
        position: BasePosition::Ecef {
            xyz_m: base.at_epoch(),
        },
        antenna_height_m: Some(0.0),
        antenna_type: None,
        frame: format!("ITRF2020, epoch {EPOCH:.3}"),
    };
    let conf = PpkConfig::dji().to_conf(&setup);
    let pos_file = rtklib
        .solve(&rover_obs, &base_obs, &navs, &conf, tmp.path(), rover.name)
        .unwrap();
    let traj = pos::read(&pos_file).unwrap();

    let truth = rover.at_epoch();
    let baseline_m = (0..3)
        .map(|i| (truth[i] - base.at_epoch()[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let (lat, lon, _) = ecef_to_geodetic(truth);
    let rot = enu_basis(lat, lon);
    let mut enu: [Vec<f64>; 3] = Default::default();
    let mut sigma: [Vec<f64>; 3] = Default::default();
    let mut fixed = 0;
    for e in traj.iter().filter(|e| e.quality == SolQuality::Fixed) {
        fixed += 1;
        let d = [0, 1, 2].map(|i| e.xyz[i] - truth[i]);
        let c = cov_ecef_to_enu(lat, lon, e.cov);
        for k in 0..3 {
            enu[k].push(rot[k][0] * d[0] + rot[k][1] * d[1] + rot[k][2] * d[2]);
        }
        // RTKLIB writes 0 for a negative variance: such epochs have no usable sigma.
        if (0..3).all(|k| c[k][k] > 0.0) {
            for k in 0..3 {
                sigma[k].push(c[k][k].sqrt());
            }
        }
    }
    Errors {
        label: format!(
            "{} against {} ({:.1} km)",
            rover.name.to_uppercase(),
            base.name.to_uppercase(),
            baseline_m / 1000.0
        ),
        baseline_m,
        fix_rate: fixed as f64 / EPOCHS as f64,
        enu,
        sigma,
    }
}

impl Errors {
    /// Print the comparison; return the thresholds it fails.
    fn report(&self) -> Vec<String> {
        let cm = |m: f64| format!("{:+.2} cm", m * 100.0);
        let [e, n, u] = &self.enu;
        let h: Vec<f64> = e.iter().zip(n).map(|(x, y)| x.hypot(*y)).collect();
        let au: Vec<f64> = u.iter().map(|x| x.abs()).collect();
        let (bias_e, bias_n, bias_u) = (mean(e), mean(n), mean(u));
        let rms_h = (rms(e).powi(2) + rms(n).powi(2)).sqrt();
        let rms_u = rms(u);
        let (p95_h, p95_u) = (percentile(&h, 0.95), percentile(&au, 0.95));
        let (sh, su) = (spec_h(self.baseline_m), spec_u(self.baseline_m));
        println!("{}, 2025-06-29 15:00-15:15 GPST, 1 s", self.label);
        println!(
            "  fixed: {} of {EPOCHS} ({:.1} %)",
            e.len(),
            self.fix_rate * 100.0
        );
        println!(
            "  mean error E / N / U: {} / {} / {}",
            cm(bias_e),
            cm(bias_n),
            cm(bias_u)
        );
        println!(
            "  scatter (std) E / N / U: {} / {} / {}",
            cm(std(e)),
            cm(std(n)),
            cm(std(u))
        );
        println!(
            "  RMS H / U: {} / {}; 95th percentile H / U: {} / {}",
            cm(rms_h),
            cm(rms_u),
            cm(p95_h),
            cm(p95_u)
        );
        println!(
            "  RTKLIB 1-sigma, median E / N / U ({} epochs): {} / {} / {}",
            self.sigma[0].len(),
            cm(percentile(&self.sigma[0], 0.5)),
            cm(percentile(&self.sigma[1], 0.5)),
            cm(percentile(&self.sigma[2], 0.5))
        );
        println!(
            "  thresholds: bias H / U {} / {}, RMS H / U {} / {}, 95th percentile H / U {} / {}",
            cm(sh),
            cm(su),
            cm(1.5 * sh),
            cm(1.5 * su),
            cm(2.5 * sh),
            cm(2.5 * su)
        );
        let mut fails = Vec::new();
        let mut check = |ok: bool, what: String| {
            if !ok {
                fails.push(format!("{}: {what}", self.label));
            }
        };
        check(
            self.fix_rate >= MIN_FIX_RATE,
            format!("fix rate {:.3}", self.fix_rate),
        );
        if e.is_empty() {
            return fails;
        }
        check(bias_e.abs() <= sh, format!("east bias {}", cm(bias_e)));
        check(bias_n.abs() <= sh, format!("north bias {}", cm(bias_n)));
        check(bias_u.abs() <= su, format!("up bias {}", cm(bias_u)));
        check(rms_h <= 1.5 * sh, format!("horizontal RMS {}", cm(rms_h)));
        check(rms_u <= 1.5 * su, format!("vertical RMS {}", cm(rms_u)));
        check(p95_h <= 2.5 * sh, format!("horizontal 95th pct {}", cm(p95_h)));
        check(p95_u <= 2.5 * su, format!("vertical 95th pct {}", cm(p95_u)));
        fails
    }
}

#[test]
fn cors_baselines_match_published_coordinates() {
    let Some(rtklib) = rtklib() else {
        return;
    };
    let mut fails = Vec::new();
    for (base, rover) in [(&MIAL, &NOR3), (&MILS, &MISR)] {
        fails.extend(baseline(&rtklib, base, rover).report());
    }
    assert!(fails.is_empty(), "{fails:#?}");
}
