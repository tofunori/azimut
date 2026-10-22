//! Ellipsoid maths on GRS80 (the ellipsoid of ITRF and NAD83; WGS84 differs by 0.1 mm).

/// Semi-major axis, metres.
pub const A: f64 = 6_378_137.0;
/// Flattening (GRS80).
pub const F: f64 = 1.0 / 298.257_222_101;

fn e2() -> f64 {
    F * (2.0 - F)
}

/// Geodetic (degrees, metres above the ellipsoid) to ECEF metres.
pub fn geodetic_to_ecef(lat_deg: f64, lon_deg: f64, h: f64) -> [f64; 3] {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let n = A / (1.0 - e2() * lat.sin().powi(2)).sqrt();
    [
        (n + h) * lat.cos() * lon.cos(),
        (n + h) * lat.cos() * lon.sin(),
        (n * (1.0 - e2()) + h) * lat.sin(),
    ]
}

/// ECEF metres to geodetic (degrees, metres). Iterative; converges to < 1e-9 m in a few steps.
pub fn ecef_to_geodetic(p: [f64; 3]) -> (f64, f64, f64) {
    let [x, y, z] = p;
    let lon = y.atan2(x);
    let r = (x * x + y * y).sqrt();
    let mut lat = z.atan2(r * (1.0 - e2()));
    let mut h = 0.0;
    for _ in 0..10 {
        let n = A / (1.0 - e2() * lat.sin().powi(2)).sqrt();
        h = r / lat.cos() - n;
        let next = z.atan2(r * (1.0 - e2() * n / (n + h)));
        if (next - lat).abs() < 1e-14 {
            lat = next;
            break;
        }
        lat = next;
    }
    (lat.to_degrees(), lon.to_degrees(), h)
}

/// Rows are the East, North and Up unit vectors in ECEF at a location.
pub fn enu_basis(lat_deg: f64, lon_deg: f64) -> [[f64; 3]; 3] {
    let (sl, cl) = lat_deg.to_radians().sin_cos();
    let (so, co) = lon_deg.to_radians().sin_cos();
    [[-so, co, 0.0], [-sl * co, -sl * so, cl], [cl * co, cl * so, sl]]
}

/// ECEF vector for a North-East-Down offset at a location.
pub fn ned_to_ecef(lat_deg: f64, lon_deg: f64, ned: [f64; 3]) -> [f64; 3] {
    let [e, n, u] = enu_basis(lat_deg, lon_deg);
    let (dn, de, dd) = (ned[0], ned[1], ned[2]);
    [0, 1, 2].map(|i| dn * n[i] + de * e[i] - dd * u[i])
}

/// Rotate an ECEF covariance matrix to local East-North-Up.
pub fn cov_ecef_to_enu(lat_deg: f64, lon_deg: f64, c: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let r = enu_basis(lat_deg, lon_deg);
    let mut rc = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            rc[i][j] = (0..3).map(|k| r[i][k] * c[k][j]).sum();
        }
    }
    let mut out = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (0..3).map(|k| rc[i][k] * r[j][k]).sum();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for &(lat, lon, h) in &[
            (46.35, -72.55, 120.0),
            (-13.5, -71.9, 5200.0),
            (73.1, -80.0, -10.0),
        ] {
            let p = geodetic_to_ecef(lat, lon, h);
            let (la, lo, hh) = ecef_to_geodetic(p);
            assert!((la - lat).abs() < 1e-10 && (lo - lon).abs() < 1e-10 && (hh - h).abs() < 1e-6);
        }
    }

    #[test]
    fn down_offset_lowers_height_only() {
        let (lat, lon, h) = (46.35, -72.55, 120.0);
        let p = geodetic_to_ecef(lat, lon, h);
        let d = ned_to_ecef(lat, lon, [0.0, 0.0, 0.180]);
        let (la, lo, hh) = ecef_to_geodetic([p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
        assert!((hh - (h - 0.180)).abs() < 1e-7, "{}", hh - (h - 0.180));
        assert!((la - lat).abs() < 1e-12 && (lo - lon).abs() < 1e-12);
    }

    #[test]
    fn north_offset_moves_north() {
        let (lat, lon, h) = (46.35, -72.55, 120.0);
        let p = geodetic_to_ecef(lat, lon, h);
        let d = ned_to_ecef(lat, lon, [1.0, 0.0, 0.0]);
        let (la, lo, _) = ecef_to_geodetic([p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
        assert!(la > lat && (lo - lon).abs() < 1e-12);
    }

    #[test]
    fn covariance_rotation_keeps_trace() {
        let c = [[4e-4, 1e-5, -2e-5], [1e-5, 3e-4, 5e-6], [-2e-5, 5e-6, 9e-4]];
        let e = cov_ecef_to_enu(46.35, -72.55, c);
        let tr = |m: [[f64; 3]; 3]| m[0][0] + m[1][1] + m[2][2];
        assert!((tr(c) - tr(e)).abs() < 1e-15);
    }
}
