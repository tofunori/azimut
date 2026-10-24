//! Satellite background in the report map: the tiles chosen for the view cover all of it,
//! and the page carries the credit.

use azimut_export::basemap::{tile_range, Basemap, Tile};
use azimut_export::report::{FlightReport, Lang};

fn demo() -> FlightReport {
    let json = include_str!("../../../examples/demo-flight/DJI_202606151400_001_Demo_report.json");
    serde_json::from_str(json).unwrap()
}

/// x, y, width and height of every `<image>` in the page.
fn images(html: &str) -> Vec<[f64; 4]> {
    html.split("<image ")
        .skip(1)
        .map(|rest| {
            let tag = rest.split("/>").next().unwrap();
            let attr = |k: &str| -> f64 {
                let from = tag
                    .find(&format!(" {k}=\""))
                    .map(|i| i + k.len() + 3)
                    .unwrap_or_else(|| {
                        // x is the first attribute, without a leading space.
                        assert_eq!(k, "x");
                        3
                    });
                tag[from..].split('"').next().unwrap().parse().unwrap()
            };
            [attr("x"), attr("y"), attr("width"), attr("height")]
        })
        .collect()
}

fn with_tiles(rep: &mut FlightReport, max_zoom: u8) -> usize {
    let r = tile_range(&rep.map_bounds().unwrap(), max_zoom, 48);
    rep.basemap = Some(Basemap {
        zoom: r.zoom,
        tiles: r
            .tiles()
            .map(|(x, y)| Tile {
                x,
                y,
                mime: "image/jpeg".into(),
                data: vec![0xff, 0xd8, 0xff],
            })
            .collect(),
        attribution: "EOxCloudless 2016 <test>".into(),
    });
    r.count()
}

#[test]
fn tiles_cover_the_whole_view() {
    for zoom in [14, 16, 19] {
        let mut rep = demo();
        let n = with_tiles(&mut rep, zoom);
        let html = rep.html_in(Lang::Fr);
        let boxes = images(&html);
        assert_eq!(boxes.len(), n);
        let left = boxes.iter().map(|b| b[0]).fold(f64::MAX, f64::min);
        let top = boxes.iter().map(|b| b[1]).fold(f64::MAX, f64::min);
        let right = boxes.iter().map(|b| b[0] + b[2]).fold(f64::MIN, f64::max);
        let bottom = boxes.iter().map(|b| b[1] + b[3]).fold(f64::MIN, f64::max);
        assert!(
            left <= 0.0 && top <= 0.0 && right >= 560.0 && bottom >= 420.0,
            "zoom {zoom}: {boxes:?}"
        );
        // Each tile meets the next one, overlapping by half a unit: no gap.
        for b in &boxes {
            let (end, below) = (b[0] + b[2], b[1] + b[3]);
            assert!(
                end > 560.0 || boxes.iter().any(|o| (o[0] - (end - 0.5)).abs() < 0.02),
                "gap at x {end}"
            );
            assert!(
                below > 420.0 || boxes.iter().any(|o| (o[1] - (below - 0.5)).abs() < 0.02),
                "gap at y {below}"
            );
        }
        assert!(
            html.contains("Fond : EOxCloudless 2016 &lt;test&gt;"),
            "credit missing"
        );
        assert!(html.contains("data:image/jpeg;base64,/9j/"));
    }
}

#[test]
fn no_basemap_no_image() {
    let rep = demo();
    let html = rep.html();
    assert!(!html.contains("<image "));
    assert!(!html.contains("map-clip"));
    assert!(!html.contains("Background:"));
    // The JSON stays as it was.
    let mut sat = demo();
    with_tiles(&mut sat, 16);
    assert_eq!(
        serde_json::to_string(&sat).unwrap(),
        serde_json::to_string(&rep).unwrap()
    );
}

#[test]
fn view_bounds_hold_every_camera() {
    let rep = demo();
    let b = rep.map_bounds().unwrap();
    for e in &rep.exposures {
        if let (Some(lat), Some(lon)) = (e.lat_deg, e.lon_deg) {
            assert!(lat > b.south && lat < b.north && lon > b.west && lon < b.east);
        }
    }
    assert!(b.m_per_px > 0.0);
}
