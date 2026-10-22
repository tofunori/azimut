//! RTK shift on a scanned synthetic flight: every paired exposure moves by the base
//! correction, float positions keep their status, captures without a record are listed.

use azimut_io::scan::{scan, ScanOptions};
use azimut_io::synthetic::{write_flight, SyntheticFlight};
use azimut_solve::geodesy::geodetic_to_ecef;
use azimut_solve::pos::SolQuality;
use azimut_solve::{shift_flight, Method, ShiftSpec};

#[test]
fn shifts_every_paired_exposure_and_keeps_status() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = SyntheticFlight {
        captures: 12,
        float_at: vec![3],
        drop_mrk_at: vec![8],
        ..Default::default()
    };
    write_flight(tmp.path(), &spec).unwrap();
    let report = scan(tmp.path(), &ScanOptions::default());
    let flight = &report.flights[0];

    let shift = ShiftSpec::Enu {
        enu_m: [0.30, -0.40, 1.20],
    };
    let sol = shift_flight(flight, &shift, "NAD83(CSRS) v8, epoch 2010.0");

    assert_eq!(sol.stats.exposures, 12);
    assert_eq!(sol.stats.exposures_fixed, 10);
    assert_eq!(sol.stats.exposures_float, 1);
    assert_eq!(sol.stats.exposures_missing, 1, "capture without an MRK record");
    assert_eq!(sol.stats.exposures_without_sigma, 0);
    assert_eq!(sol.frame, "NAD83(CSRS) v8, epoch 2010.0");
    let Method::RtkShift(s) = &sol.method else {
        panic!("RTK shift expected");
    };
    assert_eq!(s.enu_m, [0.30, -0.40, 1.20]);

    let expected = (0.3f64.powi(2) + 0.4f64.powi(2) + 1.2f64.powi(2)).sqrt();
    for e in sol.exposures.iter().filter(|e| e.camera.is_some()) {
        let c = e.camera.unwrap();
        let rec = &flight
            .mrk
            .iter()
            .find(|m| Some(m.record.index) == e.index)
            .unwrap()
            .record;
        let before = geodetic_to_ecef(rec.lat_deg, rec.lon_deg, rec.ellh_m);
        let moved = (0..3).map(|k| (c.xyz[k] - before[k]).powi(2)).sum::<f64>().sqrt();
        assert!(
            (moved - expected).abs() < 1e-6,
            "MRK positions are camera centres: no lever arm"
        );
        assert!(((c.h_m - rec.ellh_m) - 1.20).abs() < 1e-4);
        let float = e.index == Some(4);
        assert_eq!(c.quality == SolQuality::Float, float);
        assert_eq!(e.files.len(), 5, "JPEG and four bands");
    }
    let orphan = sol.exposures.iter().find(|e| e.camera.is_none()).unwrap();
    assert_eq!(orphan.problem.as_deref(), Some("no MRK record for this capture"));
}

#[test]
fn bands_are_interpolated_between_mrk_records() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = SyntheticFlight {
        captures: 6,
        band_delay_s: 0.0264,
        ..Default::default()
    };
    write_flight(tmp.path(), &spec).unwrap();
    let report = scan(tmp.path(), &ScanOptions::default());
    let sol = shift_flight(&report.flights[0], &ShiftSpec::Enu { enu_m: [0.0; 3] }, "x");

    // Synthetic MRK positions move 1e-4 degree north per 2 s interval.
    let north_deg = 1.0e-4 / spec.interval_s * 0.0264;
    for e in &sol.exposures[..5] {
        let jpeg = e.files.iter().find(|f| f.role == "jpeg").unwrap().camera.unwrap();
        for band in e.files.iter().filter(|f| f.role != "jpeg") {
            let b = band.camera.expect("interpolated band position");
            assert!(((b.lat_deg - jpeg.lat_deg) - north_deg).abs() < 1e-9);
        }
    }
    // The last exposure has no later record to interpolate towards.
    let last = sol.exposures.last().unwrap();
    assert!(last
        .files
        .iter()
        .filter(|f| f.role != "jpeg")
        .all(|f| f.camera.is_none()));
    assert!(last
        .problem
        .as_deref()
        .unwrap()
        .contains("no neighbouring MRK record"));
    assert_eq!(sol.stats.files_without_position, 4);
}
