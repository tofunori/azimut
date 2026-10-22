//! End-to-end pipeline on a synthetic flight, with stand-in RTKLIB programs (shell scripts
//! written by the test) that return a known antenna trajectory. Unix only.
#![cfg(unix)]

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use azimut_io::mrk::SECONDS_PER_WEEK;
use azimut_io::scan::{scan, ScanOptions};
use azimut_io::synthetic::{write_flight, write_rinex, SyntheticFlight, SyntheticRinex};
use azimut_solve::geodesy::{ecef_to_geodetic, geodetic_to_ecef};
use azimut_solve::pipeline::Method;
use azimut_solve::pos::SolQuality;
use azimut_solve::{process_flight, BaseSetup, PpkConfig, ProcessOptions, Rtklib, SolveError};

const LAT0: f64 = 46.35;
const LON0: f64 = -72.55;
const ANT_H: f64 = 120.18;

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

/// Stand-ins: convbin writes small obs/nav files; rnx2rtkp copies a prepared solution.
fn fake_rtklib(dir: &Path, pos: &Path, fail: bool) -> Rtklib {
    let parse = r#"out=""; nav=""
while [ $# -gt 0 ]; do case "$1" in -o) out="$2"; shift;; -n) nav="$2"; shift;; esac; shift; done"#;
    let convbin = script(
        dir,
        "convbin",
        &format!("{parse}\necho obs > \"$out\"\necho nav > \"$nav\""),
    );
    let solve_body = if fail {
        "echo 'error : no obs data' >&2\nexit 1".to_string()
    } else {
        format!("{parse}\ncp '{}' \"$out\"", pos.display())
    };
    let rnx2rtkp = script(dir, "rnx2rtkp", &solve_body);
    Rtklib { convbin, rnx2rtkp }
}

/// Antenna moving north: 1e-4 degree of latitude per photo interval.
fn antenna_lat(t: f64, flight: &SyntheticFlight) -> f64 {
    LAT0 + (t - flight.start_gps_s) / flight.interval_s * 1.0e-4
}

/// RTKLIB-style ECEF solution, every 0.2 s, optionally with a hole.
fn write_pos(path: &Path, flight: &SyntheticFlight, hole: Option<(f64, f64)>) {
    let mut s = String::from("% program   : stand-in\n%  GPST  x-ecef(m) y-ecef(m) z-ecef(m) Q ns sdx sdy sdz sdxy sdyz sdzx age ratio\n");
    let end = flight.start_gps_s + flight.captures as f64 * flight.interval_s;
    let mut t = flight.start_gps_s - 5.0;
    while t <= end + 5.0 {
        if hole.is_some_and(|(a, b)| t > a && t < b) {
            t += 0.2;
            continue;
        }
        let p = geodetic_to_ecef(antenna_lat(t, flight), LON0, ANT_H);
        let week = (t / SECONDS_PER_WEEK).floor();
        let q = if (t - flight.start_gps_s - 6.0).abs() < 0.3 {
            2
        } else {
            1
        };
        let _ = writeln!(
            s,
            "{} {:.3} {:.4} {:.4} {:.4} {q} 18 0.0060 0.0070 0.0150 0.0010 -0.0020 0.0030 1.00 12.3",
            week as u32,
            t - week * SECONDS_PER_WEEK,
            p[0],
            p[1],
            p[2]
        );
        t = ((t + 0.2) * 1000.0).round() / 1000.0;
    }
    std::fs::write(path, s).unwrap();
}

struct Setup {
    _tmp: tempfile::TempDir,
    flight: SyntheticFlight,
    report: azimut_io::ScanReport,
    base: PathBuf,
    bin: PathBuf,
    work: PathBuf,
}

fn setup() -> Setup {
    setup_with(SyntheticFlight::default())
}

fn setup_with(flight: SyntheticFlight) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    write_flight(&tmp.path().join("data/DJI_001"), &flight).unwrap();
    let base = tmp.path().join("data/base/BASE_MO.rnx");
    std::fs::create_dir_all(base.parent().unwrap()).unwrap();
    write_rinex(&base, &SyntheticRinex::default()).unwrap();
    let report = scan(&tmp.path().join("data"), &ScanOptions::default());
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let work = tmp.path().join("work");
    Setup {
        flight,
        report,
        base,
        bin,
        work,
        _tmp: tmp,
    }
}

fn options(work: &Path) -> ProcessOptions {
    ProcessOptions {
        ppk: PpkConfig::dji(),
        base: BaseSetup::default(),
        max_gap_s: 1.0,
        work_dir: work.to_path_buf(),
    }
}

#[test]
fn camera_centres_follow_the_trajectory_and_lever_arm() {
    let s = setup();
    let pos = s.bin.join("prepared.pos");
    write_pos(&pos, &s.flight, None);
    let rtk = fake_rtklib(&s.bin, &pos, false);
    let f = &s.report.flights[0];
    let sol = process_flight(f, &s.base, &[], &rtk, &options(&s.work)).unwrap();

    assert_eq!(sol.exposures.len(), 10);
    assert_eq!(sol.stats.exposures_missing, 0);
    assert!(sol.stats.max_height_check_m < 1e-4);
    // Converted rover files and the solution live in the work folder.
    let Method::Ppk { rover_obs, navs, .. } = &sol.method else {
        panic!("PPK solution expected");
    };
    assert!(rover_obs.starts_with(&s.work));
    assert!(navs.iter().any(|n| n.starts_with(&s.work)));
    let conf = std::fs::read_to_string(s.work.join("DJI_001/DJI_001.conf")).unwrap();
    assert!(conf.contains("out-solformat     =xyz"));

    for (i, e) in sol.exposures.iter().enumerate() {
        let c = e.camera.expect("camera centre");
        let t = e.gps_s.unwrap();
        // Synthetic MRK lever arm: 12 mm north, 3 mm west, 180 mm down.
        assert!((c.h_m - (ANT_H - 0.180)).abs() < 1e-4, "height {}", c.h_m);
        let ant = geodetic_to_ecef(antenna_lat(t, &s.flight), LON0, ANT_H);
        let d =
            ((c.xyz[0] - ant[0]).powi(2) + (c.xyz[1] - ant[1]).powi(2) + (c.xyz[2] - ant[2]).powi(2)).sqrt();
        let lever = (0.012f64.powi(2) + 0.003f64.powi(2) + 0.18f64.powi(2)).sqrt();
        // Linear interpolation of a slightly curved path: sub-millimetre differences.
        assert!((d - lever).abs() < 1e-3, "exposure {i}: distance to antenna {d}");
        let (lat_ant, _, _) = ecef_to_geodetic(ant);
        assert!(c.lat_deg > lat_ant, "12 mm north of the antenna");
        // Each photo spans 5 files (1 JPEG + 4 bands), except the dropped-band test cases.
        assert!(!e.files.is_empty());
    }
    assert_eq!(sol.exposures[3].files.len(), 5);
    assert_eq!(sol.exposures[2].camera.unwrap().quality, SolQuality::Fixed);
    // The exposure at +6 s falls on the float epoch of the stand-in trajectory.
    assert_eq!(
        sol.exposures[3].gps_s.map(|t| t - s.flight.start_gps_s),
        Some(6.0)
    );
    assert_eq!(sol.exposures[3].camera.unwrap().quality, SolQuality::Float);
    assert_eq!(sol.stats.exposures_float, 1);
}

#[test]
fn bands_are_placed_at_their_own_exposure_time() {
    // Measured on a real Mavic 3M: bands are exposed about 26 ms after the RGB JPEG.
    let s = setup_with(SyntheticFlight {
        band_delay_s: 0.0264,
        ..Default::default()
    });
    let pos = s.bin.join("prepared.pos");
    write_pos(&pos, &s.flight, None);
    let rtk = fake_rtklib(&s.bin, &pos, false);
    let sol = process_flight(&s.report.flights[0], &s.base, &[], &rtk, &options(&s.work)).unwrap();
    assert_eq!(sol.stats.files_without_position, 0);

    // The antenna moves 1e-4 degree north per 2 s interval.
    let north_deg = 1.0e-4 / s.flight.interval_s * 0.0264;
    for e in &sol.exposures {
        let jpeg = e.files.iter().find(|f| f.role == "jpeg").unwrap();
        assert_eq!(jpeg.time_offset_s, Some(0.0));
        assert_eq!(jpeg.camera, e.camera, "the MRK times the JPEG");
        for band in e.files.iter().filter(|f| f.role != "jpeg") {
            assert!((band.time_offset_s.unwrap() - 0.0264).abs() < 2e-6);
            let (b, j) = (band.camera.unwrap(), jpeg.camera.unwrap());
            assert!(
                ((b.lat_deg - j.lat_deg) - north_deg).abs() < 1e-9,
                "{}: band {} deg north of the JPEG",
                band.file_name,
                b.lat_deg - j.lat_deg
            );
        }
    }
}

#[test]
fn trajectory_gap_leaves_exposure_without_position() {
    let s = setup();
    let pos = s.bin.join("prepared.pos");
    let gap_at = s.flight.start_gps_s + 8.0;
    write_pos(&pos, &s.flight, Some((gap_at - 1.0, gap_at + 1.0)));
    let rtk = fake_rtklib(&s.bin, &pos, false);
    let sol = process_flight(&s.report.flights[0], &s.base, &[], &rtk, &options(&s.work)).unwrap();
    assert_eq!(sol.stats.exposures_missing, 1);
    let missing = sol.exposures.iter().find(|e| e.camera.is_none()).unwrap();
    assert_eq!(missing.index, Some(5));
    assert!(missing.problem.as_deref().unwrap().contains("gap"));
}

#[test]
fn rtklib_failure_is_reported_with_its_message() {
    let s = setup();
    let pos = s.bin.join("prepared.pos");
    write_pos(&pos, &s.flight, None);
    let rtk = fake_rtklib(&s.bin, &pos, true);
    let err = process_flight(&s.report.flights[0], &s.base, &[], &rtk, &options(&s.work)).unwrap_err();
    match err {
        SolveError::Program { stderr, status, .. } => {
            assert_eq!(status, Some(1));
            assert!(stderr.contains("no obs data"));
        }
        e => panic!("unexpected error {e}"),
    }
}

#[test]
fn locate_reports_missing_programs() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = vec![tmp.path().to_path_buf()];
    assert!(matches!(
        Rtklib::locate_in(&dirs),
        Err(SolveError::RtklibNotFound)
    ));
    let pos = tmp.path().join("p.pos");
    std::fs::write(&pos, "").unwrap();
    let rtk = fake_rtklib(tmp.path(), &pos, false);
    let found = Rtklib::locate_in(&dirs).unwrap();
    assert_eq!(found.rnx2rtkp, rtk.rnx2rtkp);
}
