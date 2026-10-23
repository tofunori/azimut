//! Positions written into synthetic DJI images: only the position fields change, the file
//! keeps its length, every other byte and XMP field stays, and the edit can be undone.

use std::path::{Path, PathBuf};

use azimut_export::images::{
    apply, copy_flight, patch_xmp, plan, plan_flight, rationals_deg, revert, FileEdit, XMP_FIELDS,
};
use azimut_io::layout::locate_fields;
use azimut_io::scan::{scan, ScanOptions};
use azimut_io::synthetic::{write_flight, SyntheticFlight};
use azimut_io::xmp::XmpFields;
use azimut_solve::{shift_flight, FlightSolution, ShiftSpec};

fn xmp(path: &Path) -> XmpFields {
    let raw = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")) {
        azimut_io::headers::read_jpeg_xmp(path)
    } else {
        azimut_io::headers::read_tiff_xmp(path)
    };
    XmpFields::parse(&raw.unwrap().packets)
}

/// Latitude, longitude, altitude and map datum read back from the EXIF GPS IFD.
fn gps(path: &Path) -> (f64, f64, f64, Vec<u8>) {
    let l = locate_fields(path).unwrap();
    let bytes = std::fs::read(path).unwrap();
    let value = |tag: u16| {
        let e = l.gps.iter().find(|e| e.tag == tag).unwrap();
        let o = e.value_offset as usize;
        bytes[o..o + e.value_len().unwrap() as usize].to_vec()
    };
    let sign = |r: Vec<u8>, neg: u8| if r[0] == neg { -1.0 } else { 1.0 };
    let lat = sign(value(1), b'S') * rationals_deg(&value(2), true);
    let lon = sign(value(3), b'W') * rationals_deg(&value(4), true);
    let a = value(6);
    let alt = u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64
        / u32::from_le_bytes([a[4], a[5], a[6], a[7]]) as f64;
    (lat, lon, alt, value(18))
}

fn flight(dir: &Path, captures: usize, multispectral: bool, enu_m: [f64; 3]) -> FlightSolution {
    write_flight(
        dir,
        &SyntheticFlight {
            captures,
            multispectral,
            ..Default::default()
        },
    )
    .unwrap();
    let report = scan(dir, &ScanOptions::default());
    // RTK shift: every camera moves, with the MRK's standard deviations.
    shift_flight(&report.flights[0], &ShiftSpec::Enu { enu_m }, "test frame")
}

#[test]
fn writes_only_the_position_fields_and_can_undo() {
    let tmp = tempfile::tempdir().unwrap();
    // Down by 101.5 m: the height loses a digit, so the XMP padding has to grow.
    let sol = flight(tmp.path(), 3, true, [1.234, -2.345, -101.5]);
    let before: Vec<(PathBuf, Vec<u8>)> = sol
        .exposures
        .iter()
        .flat_map(|e| &e.files)
        .map(|f| (f.path.clone(), std::fs::read(&f.path).unwrap()))
        .collect();
    assert_eq!(before.len(), 15);

    let planned = plan_flight(&sol);
    assert!(planned.skipped.is_empty(), "{:?}", planned.skipped);
    assert_eq!(planned.written.len(), 15);
    for edit in &planned.written {
        apply(edit).unwrap();
    }

    for f in sol.exposures.iter().flat_map(|e| &e.files) {
        let c = f.camera.unwrap();
        assert!(c.h_m < 100.0);
        let orig = &before.iter().find(|b| b.0 == f.path).unwrap().1;
        let now = std::fs::read(&f.path).unwrap();
        assert_eq!(orig.len(), now.len(), "file length kept");
        let abs = f.path.canonicalize().unwrap();
        let edit = planned.written.iter().find(|w| w.path == abs).unwrap();
        for (i, (a, b)) in orig.iter().zip(&now).enumerate() {
            if a != b {
                let i = i as u64;
                assert!(
                    edit.patches
                        .iter()
                        .any(|p| (p.offset..p.offset + p.new.len() as u64 / 2).contains(&i)),
                    "byte {i} changed outside the patches"
                );
            }
        }

        let (lat, lon, alt, datum) = gps(&f.path);
        assert!((lat - c.lat_deg).abs() < 1e-9 && (lon - c.lon_deg).abs() < 1e-9);
        assert!((alt - c.h_m).abs() < 1e-4, "{alt} vs {}", c.h_m);
        assert_eq!(datum, b"WGS-84\0\0", "GPSMapDatum left as it was");

        let old = XmpFields::parse(&[String::from_utf8_lossy(orig)]).qualified;
        let new = xmp(&f.path).qualified;
        assert_eq!(old.len(), new.len());
        for (k, v) in &old {
            if !XMP_FIELDS.contains(&k.as_str()) {
                assert_eq!(new.get(k), Some(v), "{k}");
            }
        }
        assert_eq!(new["drone-dji:GpsLatitude"], format!("{:+.9}", c.lat_deg));
        assert_eq!(new["drone-dji:AbsoluteAltitude"], format!("{:+.3}", c.h_m));
        let [_, n, _] = c.sigma_enu_m.unwrap();
        assert_eq!(new["drone-dji:RtkStdLat"], format!("{n:.5}"));
    }

    // The scan reads the same captures from the edited files.
    let again = scan(tmp.path(), &ScanOptions::default());
    assert_eq!(again.flights[0].captures.len(), 3);

    // Undo through a JSON round trip, then again (already undone).
    let json = serde_json::to_string(&planned.written).unwrap();
    let edits: Vec<FileEdit> = serde_json::from_str(&json).unwrap();
    for edit in &edits {
        assert!(revert(edit).unwrap());
        assert!(!revert(edit).unwrap(), "second undo is a no-op");
    }
    for (path, bytes) in &before {
        assert_eq!(&std::fs::read(path).unwrap(), bytes, "{}", path.display());
    }
}

#[test]
fn a_file_changed_after_planning_is_not_written() {
    let tmp = tempfile::tempdir().unwrap();
    let sol = flight(tmp.path(), 1, false, [1.0; 3]);
    let f = &sol.exposures[0].files[0];
    let edit = plan(&f.path, &f.camera.unwrap()).unwrap();
    let mut bytes = std::fs::read(&f.path).unwrap();
    bytes[edit.patches[0].offset as usize] ^= 0xFF;
    std::fs::write(&f.path, &bytes).unwrap();
    assert!(apply(&edit).unwrap_err().contains("not the expected"));
    assert_eq!(std::fs::read(&f.path).unwrap(), bytes, "left unchanged");
}

#[test]
fn files_without_uncertainty_are_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let mut sol = flight(tmp.path(), 1, false, [1.0; 3]);
    sol.exposures[0].files[0].camera.as_mut().unwrap().sigma_enu_m = None;
    let r = plan_flight(&sol);
    assert!(r.written.is_empty());
    assert!(r.skipped[0].1.contains("no uncertainty"), "{:?}", r.skipped);
}

#[test]
fn xmp_patching_keeps_length_and_refuses_element_form() {
    let pad = " ".repeat(40);
    let packet = format!(
        "<?xpacket begin=\"\"?><x:xmpmeta><rdf:Description drone-dji:AbsoluteAltitude=\"+9.000\" \
         drone-dji:Other=\"a\"/></x:xmpmeta>{pad}<?xpacket end=\"w\"?>"
    );
    let field = "drone-dji:AbsoluteAltitude";
    let out = patch_xmp(packet.as_bytes(), &[(field, "+1234.567".into())]).unwrap();
    assert_eq!(out.len(), packet.len());
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("drone-dji:AbsoluteAltitude=\"+1234.567\" drone-dji:Other=\"a\""));
    let shorter = patch_xmp(packet.as_bytes(), &[(field, "+1.0".into())]).unwrap();
    assert_eq!(shorter.len(), packet.len());

    let no_pad = packet.replace(&pad, "");
    let e = patch_xmp(no_pad.as_bytes(), &[(field, "+1234.567".into())]).unwrap_err();
    assert!(e.contains("padding"), "{e}");

    let element = "<?xpacket begin=\"\"?><x:xmpmeta><drone-dji:AbsoluteAltitude>9\
                   </drone-dji:AbsoluteAltitude></x:xmpmeta>   <?xpacket end=\"w\"?>";
    let e = patch_xmp(element.as_bytes(), &[(field, "1".into())]).unwrap_err();
    assert!(e.contains("rewritten by another tool"), "{e}");
}

#[test]
fn copies_carry_the_positions_and_originals_stay() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("flight");
    let mut sol = flight(&src, 2, true, [0.5, 0.5, 0.5]);
    // One capture without a position.
    for f in &mut sol.exposures[1].files {
        f.camera = None;
    }
    sol.exposures[1].camera = None;
    let before: Vec<Vec<u8>> = sol
        .exposures
        .iter()
        .flat_map(|e| &e.files)
        .map(|f| std::fs::read(&f.path).unwrap())
        .collect();

    let dest = tmp.path().join("copies");
    let r = copy_flight(&sol, &dest, false).unwrap();
    assert_eq!(r.written.len(), 5);
    assert_eq!(r.skipped.len(), 5, "unpositioned files are not copied");
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 5);
    for f in &sol.exposures[0].files {
        let (lat, ..) = gps(&dest.join(&f.file_name));
        assert!((lat - f.camera.unwrap().lat_deg).abs() < 1e-9);
    }
    let after: Vec<Vec<u8>> = sol
        .exposures
        .iter()
        .flat_map(|e| &e.files)
        .map(|f| std::fs::read(&f.path).unwrap())
        .collect();
    assert_eq!(before, after, "originals untouched");

    // A second run never overwrites; with unpositioned files they are copied unchanged.
    let again = copy_flight(&sol, &dest, true).unwrap();
    assert!(again.written.is_empty());
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 10);
    assert!(again
        .skipped
        .iter()
        .any(|(_, why)| why.contains("already exists")));
    assert!(again
        .skipped
        .iter()
        .any(|(_, why)| why.contains("aircraft's own position")));
}
