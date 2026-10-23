//! Geoid grids written in the PROJ GeoTIFF layout: georeferencing, scaling, bilinear
//! interpolation, missing nodes and the frame check.

use azimut_geo::synthetic::SyntheticGrid;
use azimut_geo::GeoidGrid;

fn grid(dir: &tempfile::TempDir, spec: SyntheticGrid) -> GeoidGrid {
    let p = dir.path().join("g.tif");
    spec.write(&p).unwrap();
    GeoidGrid::open(&p).unwrap()
}

#[test]
fn interpolates_scaled_values_bilinearly() {
    let tmp = tempfile::tempdir().unwrap();
    let g = grid(&tmp, SyntheticGrid::default());
    assert_eq!(g.source_frame(), "NAD83(CSRS)v6 (EPSG:8251)");
    assert_eq!(g.vertical_datum(), "CGVD2013(CGG2013a) height (EPSG:9245)");

    // On a node, inside a cell, on the last row and column, in the second strip.
    for (lat, lon) in [(47.0, -73.0), (46.1, -72.3), (45.0, -71.5), (45.6, -71.9)] {
        let n = g.undulation(lat, lon).unwrap();
        assert!(
            (n - SyntheticGrid::expected(lat, lon)).abs() < 1e-9,
            "{lat} {lon}: {n}"
        );
    }
    let h = g.orthometric_height(46.1, -72.3, 100.0).unwrap();
    assert!(
        (h - (100.0 - SyntheticGrid::expected(46.1, -72.3))).abs() < 1e-9,
        "H = h - N"
    );
    // Outside the grid.
    assert_eq!(g.undulation(47.1, -72.0), None);
    assert_eq!(g.undulation(46.0, -71.4), None);
    assert_eq!(g.undulation(46.0, -73.1), None);
}

#[test]
fn pixel_is_area_grids_are_shifted_by_half_a_cell() {
    let tmp = tempfile::tempdir().unwrap();
    let g = grid(
        &tmp,
        SyntheticGrid {
            pixel_is_point: false,
            ..Default::default()
        },
    );
    let n = g.undulation(46.1, -72.3).unwrap();
    assert!((n - SyntheticGrid::expected(46.1, -72.3)).abs() < 1e-9);
}

#[test]
fn missing_nodes_give_no_height() {
    let tmp = tempfile::tempdir().unwrap();
    let g = grid(
        &tmp,
        SyntheticGrid {
            nodata: Some(-32768),
            ..Default::default()
        },
    );
    // Node (col 3, row 3) is at 71.5 W, 45.5 N.
    assert_eq!(g.undulation(45.6, -71.6), None);
    assert!(g.undulation(46.6, -72.9).is_some());
}

#[test]
fn other_grid_types_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("g.tif");
    SyntheticGrid {
        kind: "HORIZONTAL_OFFSET".into(),
        ..Default::default()
    }
    .write(&p)
    .unwrap();
    let e = GeoidGrid::open(&p).unwrap_err().to_string();
    assert!(e.contains("HORIZONTAL_OFFSET"), "{e}");
}

#[test]
fn frame_must_match_the_grid() {
    let tmp = tempfile::tempdir().unwrap();
    let g = grid(&tmp, SyntheticGrid::default());
    assert_eq!(g.check_frame("NAD83(CSRS)v6, epoch 2010.0"), Ok(None));
    let note = g.check_frame("NAD83(CSRS) v8, epoch 2010.0").unwrap().unwrap();
    assert!(note.contains("not applied"), "{note}");
    for wrong in ["ITRF2020, epoch 2026.45", "IGS20, epoch 2024.605", "unspecified"] {
        let e = g.check_frame(wrong).unwrap_err();
        assert!(e.contains("ca_nrc_CGG2013ai08.tif"), "{e}");
    }

    let tmp8 = tempfile::tempdir().unwrap();
    let g8 = grid(
        &tmp8,
        SyntheticGrid {
            description: "ITRF2008 (EPSG:7911) to CGVD2013(CGG2013a) height (EPSG:9245). Test".into(),
            ..Default::default()
        },
    );
    assert_eq!(g8.check_frame("ITRF2008, epoch 2024.6"), Ok(None));
    assert!(g8.check_frame("IGS20, epoch 2024.605").unwrap().is_some());
    assert!(g8.check_frame("NAD83(CSRS) v8, epoch 2010.0").is_err());
}
