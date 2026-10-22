//! RINEX summaries and base-station checks on synthetic files.

use azimut_io::rinex::{summarize, Compression, EpochSource, RinexKind};
use azimut_io::scan::{scan, ScanOptions, Severity};
use azimut_io::synthetic::{write_flight, write_rinex, SyntheticFlight, SyntheticRinex};

fn codes(f: &azimut_io::Flight) -> Vec<&str> {
    f.findings.iter().map(|x| x.code.as_str()).collect()
}

#[test]
fn rinex3_header_and_epochs() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("BASE00CAN_R_20261660000_01H_01S_MO.rnx");
    let spec = SyntheticRinex::default();
    write_rinex(&p, &spec).unwrap();
    let r = summarize(&p);
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.version, Some(3.04));
    assert_eq!(r.kind, Some(RinexKind::Observation));
    assert_eq!(r.epochs_from, EpochSource::Data);
    assert_eq!(r.first_epoch_s, Some(spec.start_gps_s));
    assert_eq!(r.last_epoch_s, Some(spec.end_gps_s));
    assert_eq!(r.interval_s, Some(1.0));
    assert_eq!(r.bands_label(), "E 1/5, G 1/2, R 1/2");
    assert_eq!(r.antenna_type.as_deref(), Some("EMLIDREACHRS4   NONE"));
    assert_eq!(r.antenna_delta_hen_m.map(|d| d[0]), Some(1.8));
    assert!(r.approx_xyz_m.is_some());
    assert!(!r.single_frequency());
    // Header, a few epochs and the 64 kB tail at most.
    assert!(
        r.bytes_read <= r.size.min(150 * 1024),
        "read {} of {}",
        r.bytes_read,
        r.size
    );
}

#[test]
fn rinex2_two_digit_years_and_inferred_interval() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("base1660.26o");
    let spec = SyntheticRinex {
        major_version: 2,
        interval_s: 5.0,
        interval_in_header: false,
        codes: vec![('G', vec!["C1C", "L1C"]), ('R', vec!["C1C", "L1C"])],
        ..Default::default()
    };
    write_rinex(&p, &spec).unwrap();
    let r = summarize(&p);
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.version, Some(2.11));
    assert_eq!(r.first_epoch_s, Some(spec.start_gps_s));
    assert_eq!(r.last_epoch_s, Some(spec.end_gps_s));
    assert_eq!(r.interval_s, Some(5.0));
    assert!(r.single_frequency());
    assert_eq!(r.bands_label(), "G 1, R 1");
}

#[test]
fn archives_and_non_rinex_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let gz = tmp.path().join("base.26d.gz");
    std::fs::write(&gz, [0x1f, 0x8b, 0, 0]).unwrap();
    let r = summarize(&gz);
    assert_eq!(r.compression, Compression::Archive);
    assert!(r.error.is_some());

    let junk = tmp.path().join("x.obs");
    std::fs::write(&junk, "hello\nworld\n").unwrap();
    assert!(summarize(&junk).error.is_some());
}

#[test]
fn hatanaka_header_gives_times_from_header() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain.rnx");
    write_rinex(&plain, &SyntheticRinex::default()).unwrap();
    let text = std::fs::read_to_string(&plain).unwrap();
    let header_end = text.find("END OF HEADER").unwrap() + "END OF HEADER".len();
    let crx = tmp.path().join("BASE_MO.crx");
    let mut c = String::new();
    c += &format!(
        "{:<60}{:<20}\n",
        "3.0                 COMPACT RINEX FORMAT", "CRINEX VERS   / TYPE"
    );
    c += &format!("{:<60}{:<20}\n", "RNX2CRX ver.4.1.0", "CRINEX PROG / DATE");
    c += &text[..header_end];
    c += "\n>2026 06 15 13 50  0.0000000  0  3      G01R01E01\n";
    std::fs::write(&crx, c).unwrap();
    let r = summarize(&crx);
    assert_eq!(r.compression, Compression::Hatanaka);
    assert_eq!(r.epochs_from, EpochSource::Header);
    assert_eq!(r.first_epoch_s, Some(SyntheticRinex::default().start_gps_s));
}

#[test]
fn base_covering_the_flight_is_checked() {
    let tmp = tempfile::tempdir().unwrap();
    write_flight(&tmp.path().join("DCIM/100MEDIA"), &SyntheticFlight::default()).unwrap();
    std::fs::create_dir_all(tmp.path().join("base")).unwrap();
    write_rinex(&tmp.path().join("base/BASE_MO.rnx"), &SyntheticRinex::default()).unwrap();
    let r = scan(tmp.path(), &ScanOptions::default());
    let f = &r.flights[0];
    let b = f.base.as_ref().expect("base chosen");
    assert_eq!(b.exposures_outside, 0);
    assert!(b.margin_before_s.unwrap() >= 599.0);
    assert!(b.distance_km.unwrap() < 1.0);
    let c = codes(f);
    assert!(c.contains(&"base"));
    assert!(!c.contains(&"base_partial") && !c.contains(&"base_short_margin"));
    assert!(
        f.findings.iter().all(|x| x.severity != Severity::Error),
        "{:?}",
        f.findings
    );
}

#[test]
fn short_coarse_far_base_is_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let flight = SyntheticFlight::default();
    write_flight(&tmp.path().join("flight"), &flight).unwrap();
    let base_dir = tempfile::tempdir().unwrap();
    let spec = SyntheticRinex {
        // Starts after the third photo, records every 30 s, 60 km away, no antenna type.
        start_gps_s: flight.start_gps_s + 5.0,
        interval_s: 30.0,
        antenna_type: None,
        antenna_height_m: 0.0,
        approx_llh: Some((46.89, -72.55, 60.0)),
        ..Default::default()
    };
    write_rinex(
        &base_dir.path().join("far.26o"),
        &SyntheticRinex {
            major_version: 2,
            ..spec
        },
    )
    .unwrap();
    let opts = ScanOptions {
        base: vec![base_dir.path().to_path_buf()],
        ..Default::default()
    };
    let r = scan(tmp.path(), &opts);
    let f = &r.flights[0];
    let c = codes(f);
    for code in [
        "base_partial",
        "base_interval",
        "base_far",
        "base_antenna_type",
        "base_antenna_height",
    ] {
        assert!(c.contains(&code), "missing {code} in {c:?}");
    }
    assert_eq!(f.base.as_ref().unwrap().exposures_outside, 3);
}

#[test]
fn no_base_and_rover_1980_epoch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("flight");
    write_flight(&dir, &SyntheticFlight::default()).unwrap();
    // On-board RINEX whose first epoch is from 1980, as seen on some DJI files.
    let spec = SyntheticRinex {
        start_gps_s: 86_400.0,
        end_gps_s: 86_402.0,
        ..Default::default()
    };
    write_rinex(&dir.join("DJI_PPKOBS.obs"), &spec).unwrap();
    let f = &scan(tmp.path(), &ScanOptions::default()).flights[0];
    let c = codes(f);
    assert!(c.contains(&"rinex_1980"), "{c:?}");
    assert!(c.contains(&"no_base"), "{c:?}");
    assert_eq!(f.rover_rinex.len(), 1);
}
