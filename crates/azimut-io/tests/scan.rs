//! End-to-end checks on synthetic flights that follow the documented DJI layout.

use azimut_io::capture::GroupKey;
use azimut_io::headers::{read_jpeg_xmp, read_tiff_xmp};
use azimut_io::matching::ClockReading;
use azimut_io::photo::{read_photo, Band, ImageKind};
use azimut_io::scan::{scan, AuxKind, ScanOptions, Severity};
use azimut_io::synthetic::{jpeg_with_xmp, tiff_with_xmp, write_flight, SyntheticFlight};

fn codes(f: &azimut_io::Flight) -> Vec<&str> {
    f.findings.iter().map(|x| x.code.as_str()).collect()
}

#[test]
fn complete_m3m_flight_pairs_every_capture() {
    let tmp = tempfile::tempdir().unwrap();
    write_flight(tmp.path(), &SyntheticFlight::default()).unwrap();
    let report = scan(tmp.path(), &ScanOptions::default());
    assert_eq!(report.flights.len(), 1);
    let f = &report.flights[0];
    assert_eq!(f.photos.len(), 50);
    assert_eq!(f.captures.len(), 10);
    assert!(f.captures.iter().all(|c| c.is_complete_m3m()));
    assert!(f.captures.iter().all(|c| c.key == GroupKey::CaptureUuid));
    assert_eq!(f.matching.clock, Some(ClockReading::GpsTime));
    assert_eq!(f.matching.pairs.len(), 10);
    assert!(f.matching.max_abs_dt_s < 1e-5);
    assert!(
        f.findings.iter().all(|x| x.severity != Severity::Error),
        "{:?}",
        f.findings
    );
    assert_eq!(f.aux_files(AuxKind::RawGnss).len(), 1);
    assert!(codes(f).contains(&"m3m_groups"));
}

#[test]
fn utc_clock_is_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = SyntheticFlight {
        clock_is_utc: true,
        ..Default::default()
    };
    write_flight(tmp.path(), &spec).unwrap();
    let f = &scan(tmp.path(), &ScanOptions::default()).flights[0];
    assert_eq!(f.matching.clock, Some(ClockReading::Utc));
    assert_eq!(f.matching.pairs.len(), 10);
    assert!(codes(f).contains(&"clock_utc"));
}

#[test]
fn incomplete_capture_and_missing_record_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = SyntheticFlight {
        drop_nir_at: vec![3],
        drop_mrk_at: vec![7],
        float_at: vec![1, 2],
        ..Default::default()
    };
    write_flight(tmp.path(), &spec).unwrap();
    let f = &scan(tmp.path(), &ScanOptions::default()).flights[0];
    let incomplete: Vec<_> = f.captures.iter().filter(|c| !c.is_complete_m3m()).collect();
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].missing_bands(), vec![Band::Nir]);
    assert_eq!(f.matching.pairs.len(), 9);
    assert_eq!(f.matching.unmatched_captures.len(), 1);
    assert_eq!(f.quality.float, 2);
    let c = codes(f);
    assert!(c.contains(&"m3m_incomplete"));
    assert!(c.contains(&"capture_without_mrk"));
    assert!(c.contains(&"mrk_quality"));
}

#[test]
fn numbering_after_one_does_not_break_pairing() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = SyntheticFlight {
        first_index: 412,
        ..Default::default()
    };
    write_flight(tmp.path(), &spec).unwrap();
    let f = &scan(tmp.path(), &ScanOptions::default()).flights[0];
    assert_eq!(f.matching.pairs.len(), 10);
    for p in &f.matching.pairs {
        let rec = &f.mrk[p.mrk].record;
        assert_eq!(Some(rec.index), f.captures[p.capture].sequence);
    }
    assert!(codes(f).contains(&"starts_after_1"));
}

#[test]
fn missing_uuid_falls_back_on_file_number() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = SyntheticFlight {
        with_uuid: false,
        ..Default::default()
    };
    write_flight(tmp.path(), &spec).unwrap();
    let f = &scan(tmp.path(), &ScanOptions::default()).flights[0];
    assert_eq!(f.captures.len(), 10);
    assert!(f
        .captures
        .iter()
        .all(|c| c.key == GroupKey::Sequence && c.is_complete_m3m()));
    assert!(codes(f).contains(&"m3m_no_uuid"));
}

#[test]
fn no_mrk_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let written = write_flight(tmp.path(), &SyntheticFlight::default()).unwrap();
    for p in written
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "MRK"))
    {
        std::fs::remove_file(p).unwrap();
    }
    let f = &scan(tmp.path(), &ScanOptions::default()).flights[0];
    assert_eq!(f.findings[0].severity, Severity::Error);
    assert_eq!(f.findings[0].code, "no_mrk");
}

#[test]
fn several_flight_folders_are_kept_apart() {
    let tmp = tempfile::tempdir().unwrap();
    write_flight(&tmp.path().join("DJI_001"), &SyntheticFlight::default()).unwrap();
    let later = SyntheticFlight {
        start_gps_s: SyntheticFlight::default().start_gps_s + 3600.0,
        captures: 4,
        multispectral: false,
        ..Default::default()
    };
    write_flight(&tmp.path().join("DJI_002"), &later).unwrap();
    let r = scan(tmp.path(), &ScanOptions::default());
    assert_eq!(r.flights.len(), 2);
    assert_eq!(r.flights[1].captures.len(), 4);
    assert!(r.flights[1].captures.iter().all(|c| !c.is_multispectral()));
    assert_eq!(r.flights[1].matching.pairs.len(), 4);
}

#[test]
fn header_reads_skip_pixels() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = SyntheticFlight {
        captures: 1,
        band_size: (2592, 1944),
        ..Default::default()
    };
    write_flight(tmp.path(), &spec).unwrap();
    let r = scan(tmp.path(), &ScanOptions::default());
    let tif = r.flights[0]
        .photos
        .iter()
        .find(|p| matches!(p.kind, ImageKind::Band(_)))
        .unwrap();
    assert!(tif.size > 10_000_000);
    assert!(tif.bytes_read < 64 * 1024, "read {} bytes", tif.bytes_read);
    let jpg = r.flights[0]
        .photos
        .iter()
        .find(|p| p.kind == ImageKind::Jpeg)
        .unwrap();
    // The 20 kB EXIF-like segment is skipped, not read.
    assert!(jpg.bytes_read < 20_000, "read {} bytes", jpg.bytes_read);
}

#[test]
fn readers_reject_wrong_files() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("x.jpg");
    std::fs::write(&p, b"not an image").unwrap();
    assert!(read_jpeg_xmp(&p).is_err());
    let t = tmp.path().join("x.tif");
    std::fs::write(&t, b"II*\0\xff\xff\xff\xff").unwrap();
    assert!(read_tiff_xmp(&t).map(|r| r.packets.is_empty()).unwrap_or(true));
    let photo = read_photo(&p);
    assert!(photo.error.is_some());
}

#[test]
fn big_endian_and_inline_values_are_read() {
    // Round trip through the little-endian writer, then a hand-made big-endian file.
    let xmp = r#"<x:xmpmeta xmlns:x="x"><rdf:RDF xmlns:rdf="r"><rdf:Description xmlns:drone-dji="d" drone-dji:BandName="Red"/></rdf:RDF></x:xmpmeta>"#;
    let tmp = tempfile::tempdir().unwrap();
    let le = tmp.path().join("le.tif");
    std::fs::write(&le, tiff_with_xmp(xmp, 1, 1)).unwrap();
    assert_eq!(read_tiff_xmp(&le).unwrap().packets, vec![xmp.to_string()]);

    let mut be = b"MM\0\x2a\0\0\0\x08".to_vec();
    let data_off = 8 + 2 + 12 + 4;
    be.extend_from_slice(&1u16.to_be_bytes());
    be.extend_from_slice(&700u16.to_be_bytes());
    be.extend_from_slice(&7u16.to_be_bytes());
    be.extend_from_slice(&(xmp.len() as u32).to_be_bytes());
    be.extend_from_slice(&(data_off as u32).to_be_bytes());
    be.extend_from_slice(&0u32.to_be_bytes());
    be.extend_from_slice(xmp.as_bytes());
    let bep = tmp.path().join("be.tif");
    std::fs::write(&bep, be).unwrap();
    assert_eq!(read_tiff_xmp(&bep).unwrap().packets, vec![xmp.to_string()]);

    let j = tmp.path().join("a.jpg");
    std::fs::write(&j, jpeg_with_xmp(xmp)).unwrap();
    assert_eq!(read_jpeg_xmp(&j).unwrap().packets, vec![xmp.to_string()]);
}

#[test]
fn report_serialises_to_json() {
    let tmp = tempfile::tempdir().unwrap();
    write_flight(
        tmp.path(),
        &SyntheticFlight {
            captures: 2,
            ..Default::default()
        },
    )
    .unwrap();
    let r = scan(tmp.path(), &ScanOptions::default());
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"clock\":\"gps_time\""));
}
