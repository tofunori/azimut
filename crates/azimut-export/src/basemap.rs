//! Satellite background of the report map: which Web Mercator tiles (XYZ scheme, 256 px)
//! cover the plan view, and the tiles once fetched.
//!
//! This crate does not fetch anything: the caller downloads the tiles of [`TileRange`] and
//! hands them back as a [`Basemap`], which the page embeds (base64), so the report stays a
//! single file that opens offline. Each tile is placed by its own corners, so the
//! Web Mercator stretch never accumulates across the view.

use std::f64::consts::PI;

/// Fetched tiles and the credit to show under the map.
#[derive(Debug, Clone, PartialEq)]
pub struct Basemap {
    pub zoom: u8,
    pub tiles: Vec<Tile>,
    /// Shown under the map, as required by the imagery licence.
    pub attribution: String,
}

/// One 256 px tile.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    /// Media type, e.g. `image/jpeg`.
    pub mime: String,
    pub data: Vec<u8>,
}

/// Area shown by the plan view, degrees, and its ground size of one SVG unit, metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapBounds {
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
    pub m_per_px: f64,
}

/// Tiles to fetch: zoom and inclusive column and row ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRange {
    pub zoom: u8,
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
}

impl TileRange {
    pub fn count(&self) -> usize {
        (self.x1 - self.x0 + 1) as usize * (self.y1 - self.y0 + 1) as usize
    }

    /// Every (x, y) of the range, row by row.
    pub fn tiles(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (self.y0..=self.y1).flat_map(move |y| (self.x0..=self.x1).map(move |x| (x, y)))
    }
}

/// Ground size of a tile pixel at the equator and zoom 0, metres.
const M_PER_PX_Z0: f64 = 2.0 * PI * 6_378_137.0 / 256.0;
/// Web Mercator stops here.
const MAX_LAT: f64 = 85.051_128_78;

/// Tile column and row (fractional) of a point.
pub fn tile_xy(lat: f64, lon: f64, zoom: u8) -> (f64, f64) {
    let n = 2f64.powi(zoom as i32);
    let phi = lat.clamp(-MAX_LAT, MAX_LAT).to_radians();
    let x = (lon + 180.0) / 360.0 * n;
    let y = (1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / PI) / 2.0 * n;
    (x, y)
}

/// Latitude and longitude of the north-west corner of tile (x, y).
pub fn tile_corner(x: u32, y: u32, zoom: u8) -> (f64, f64) {
    let n = 2f64.powi(zoom as i32);
    let lon = x as f64 / n * 360.0 - 180.0;
    let lat = (PI * (1.0 - 2.0 * y as f64 / n)).sinh().atan().to_degrees();
    (lat, lon)
}

/// Tiles covering `b`, at the zoom whose pixels are about half an SVG unit (sharp on
/// high-density screens), at most `max_zoom`, lowered until no more than `max_tiles`.
pub fn tile_range(b: &MapBounds, max_zoom: u8, max_tiles: usize) -> TileRange {
    let lat = ((b.south + b.north) / 2.0).clamp(-MAX_LAT, MAX_LAT);
    let want = (M_PER_PX_Z0 * lat.to_radians().cos() / (b.m_per_px / 2.0))
        .log2()
        .ceil();
    let mut zoom = want.clamp(0.0, max_zoom as f64) as u8;
    loop {
        let r = range_at(b, zoom);
        if r.count() <= max_tiles.max(1) || zoom == 0 {
            return r;
        }
        zoom -= 1;
    }
}

fn range_at(b: &MapBounds, zoom: u8) -> TileRange {
    let last = (1u32 << zoom) - 1;
    let (xw, yn) = tile_xy(b.north, b.west, zoom);
    let (xe, ys) = tile_xy(b.south, b.east, zoom);
    let cell = |v: f64| (v.floor().max(0.0) as u32).min(last);
    TileRange {
        zoom,
        x0: cell(xw),
        x1: cell(xe),
        y0: cell(yn),
        y1: cell(ys),
    }
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn tile_corners_round_trip() {
        // Montréal at zoom 15 (values from the OSM slippy-map formulas).
        let (x, y) = tile_xy(45.5017, -73.5673, 15);
        assert_eq!((x as u32, y as u32), (9687, 11722));
        let (lat, lon) = tile_corner(x as u32, y as u32, 15);
        let (bx, by) = tile_xy(lat, lon, 15);
        assert!((bx - bx.round()).abs() < 1e-9 && (by - by.round()).abs() < 1e-9);
        assert!((tile_corner(0, 0, 0).0 - MAX_LAT).abs() < 1e-6);
    }

    #[test]
    fn zoom_follows_the_view_and_the_tile_cap() {
        // 0.54 m per unit at 46 N: tile pixels of 0.27 m or less need zoom 19.
        let b = MapBounds {
            south: 46.0,
            west: -72.0,
            north: 46.0027,
            east: -71.9961,
            m_per_px: 0.54,
        };
        assert_eq!(tile_range(&b, 21, 64).zoom, 19);
        assert_eq!(tile_range(&b, 16, 64).zoom, 16);
        let few = tile_range(&b, 21, 4);
        assert!(few.count() <= 4 && few.zoom < 19, "{few:?}");
        let r = tile_range(&b, 16, 64);
        assert_eq!(r.tiles().count(), r.count());
    }
}
