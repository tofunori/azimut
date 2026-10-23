//! Small synthetic flights that follow the documented DJI layout.
//!
//! Used by the tests and by `azimut demo`. The files carry real metadata structures (JPEG
//! APP1 XMP segments, TIFF IFDs with tag 700, tab-separated MRK lines) built from public DJI
//! documentation. JPEG files carry no decodable image; TIFF files hold a valid 16-bit
//! single-band image, 1 x 1 pixel by default or full Mavic 3M size on request.
//! These are not real DJI files: confirm every behaviour on real SD-card copies.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::photo::Band;
use crate::time::{
    datetime_from_gps_seconds, format_dji_clock, seconds_since_gps_epoch, week_tow, GPS_UTC_OFFSET_S,
};

/// Pixel size of a Mavic 3M multispectral band image.
pub const M3M_BAND_SIZE: (u32, u32) = (2592, 1944);

/// Description of a synthetic flight.
#[derive(Debug, Clone)]
pub struct SyntheticFlight {
    pub captures: usize,
    /// Number of the first photo.
    pub first_index: u32,
    /// GPS time of the first exposure, seconds since the GPS epoch.
    pub start_gps_s: f64,
    pub interval_s: f64,
    /// Write four band TIFFs per capture (Mavic 3M).
    pub multispectral: bool,
    /// Write `UTCAtExposure` as true UTC (GPS - 18 s) instead of GPS time.
    pub clock_is_utc: bool,
    /// Capture indices (0-based) whose NIR band is not written.
    pub drop_nir_at: Vec<usize>,
    /// Capture indices (0-based) left out of the MRK file.
    pub drop_mrk_at: Vec<usize>,
    /// Capture indices (0-based) flagged float (34) instead of fixed (50).
    pub float_at: Vec<usize>,
    /// TIFF pixel size (width, height).
    pub band_size: (u32, u32),
    /// Write `CaptureUUID` in the XMP.
    pub with_uuid: bool,
    /// Band exposure time minus JPEG exposure time, seconds (about 0.026 on a real Mavic 3M).
    pub band_delay_s: f64,
}

impl Default for SyntheticFlight {
    fn default() -> Self {
        let start = NaiveDate::from_ymd_opt(2026, 6, 15)
            .and_then(|d| d.and_hms_opt(14, 0, 0))
            .map(seconds_since_gps_epoch)
            .expect("valid date");
        Self {
            captures: 10,
            first_index: 1,
            start_gps_s: start,
            interval_s: 2.0,
            multispectral: true,
            clock_is_utc: false,
            drop_nir_at: vec![],
            drop_mrk_at: vec![],
            float_at: vec![],
            band_size: (1, 1),
            with_uuid: true,
            band_delay_s: 0.0,
        }
    }
}

/// XMP packet with `drone-dji:` attributes and 2 kB of whitespace padding before the
/// trailer, as DJI writes it (real packets carry 2 to 6 kB).
fn xmp_packet(attrs: &[(&str, String)]) -> String {
    let mut a = String::new();
    for (k, v) in attrs {
        a.push_str(&format!("\n    drone-dji:{k}=\"{v}\""));
    }
    let padding = format!("{}\n", " ".repeat(99)).repeat(20);
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
<rdf:Description rdf:about=\"\" xmlns:drone-dji=\"http://www.dji.com/drone-dji/1.0/\"{a}/>\n </rdf:RDF>\n</x:xmpmeta>\n\
{padding}<?xpacket end=\"w\"?>"
    )
}

/// Camera position written into a synthetic image: latitude, longitude (degrees),
/// ellipsoidal height (m).
pub type Llh = (f64, f64, f64);

/// Little-endian EXIF GPS IFD placed at `at` (offset from the TIFF header), followed by its
/// values, in DJI's layout: version 2.3, references, degrees/minutes/seconds with seconds
/// to 1e-4, altitude to 1 mm, map datum `WGS-84`.
fn gps_ifd(at: u32, (lat, lon, h): Llh) -> Vec<u8> {
    let n_entries = 8u32;
    let values_at = at + 2 + 12 * n_entries + 4;
    let dms = |deg: f64| -> Vec<u8> {
        let a = deg.abs();
        let d = a.floor();
        let m = ((a - d) * 60.0).floor();
        let sec = ((a - d) * 60.0 - m) * 60.0;
        [
            (d as u32, 1u32),
            (m as u32, 1),
            ((sec * 1e4).round() as u32, 10_000),
        ]
        .iter()
        .flat_map(|&(n, q)| [n.to_le_bytes(), q.to_le_bytes()])
        .flatten()
        .collect()
    };
    let mut values = Vec::new();
    let mut entry = |v: &mut Vec<u8>, tag: u16, typ: u16, count: u32, data: &[u8]| {
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&typ.to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
        if data.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..data.len()].copy_from_slice(data);
            v.extend_from_slice(&inline);
        } else {
            v.extend_from_slice(&(values_at + values.len() as u32).to_le_bytes());
            values.extend_from_slice(data);
        }
    };
    let mut v = (n_entries as u16).to_le_bytes().to_vec();
    entry(&mut v, 0, 1, 4, &[2, 3, 0, 0]);
    entry(&mut v, 1, 2, 2, &[if lat < 0.0 { b'S' } else { b'N' }, 0]);
    entry(&mut v, 2, 5, 3, &dms(lat));
    entry(&mut v, 3, 2, 2, &[if lon < 0.0 { b'W' } else { b'E' }, 0]);
    entry(&mut v, 4, 5, 3, &dms(lon));
    entry(&mut v, 5, 1, 1, &[u8::from(h < 0.0)]);
    let alt = [
        ((h.abs() * 1000.0).round() as u32).to_le_bytes(),
        1000u32.to_le_bytes(),
    ]
    .concat();
    entry(&mut v, 6, 5, 1, &alt);
    entry(&mut v, 18, 2, 8, b"WGS-84\0\0");
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&values);
    v
}

/// EXIF APP1 payload (`Exif\0\0` and a TIFF structure whose IFD0 points to a GPS IFD).
fn exif_payload(gps: Llh) -> Vec<u8> {
    let mut t = b"II".to_vec();
    t.extend_from_slice(&42u16.to_le_bytes());
    t.extend_from_slice(&8u32.to_le_bytes());
    // IFD0: one entry, the GPS IFD pointer (tag 34853, LONG).
    t.extend_from_slice(&1u16.to_le_bytes());
    t.extend_from_slice(&34853u16.to_le_bytes());
    t.extend_from_slice(&4u16.to_le_bytes());
    t.extend_from_slice(&1u32.to_le_bytes());
    let gps_at = 8 + 2 + 12 + 4;
    t.extend_from_slice(&(gps_at as u32).to_le_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(&gps_ifd(gps_at as u32, gps));
    [b"Exif\0\0".as_slice(), &t].concat()
}

/// Metadata-only JPEG: SOI, an EXIF-like APP1 to skip, an XMP APP1, then SOS and EOI.
pub fn jpeg_with_xmp(xmp: &str) -> Vec<u8> {
    jpeg_with_xmp_gps(xmp, None)
}

/// [`jpeg_with_xmp`] whose 20 kB EXIF APP1 (standing in for EXIF and its thumbnail; readers
/// must skip it) starts with a GPS IFD holding `gps`.
pub fn jpeg_with_xmp_gps(xmp: &str, gps: Option<Llh>) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    let mut exif = match gps {
        Some(p) => exif_payload(p),
        None => b"Exif\0\0".to_vec(),
    };
    exif.resize(20_000, 0);
    push_segment(&mut v, 0xE1, &exif);
    let mut xp = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    xp.extend_from_slice(xmp.as_bytes());
    push_segment(&mut v, 0xE1, &xp);
    push_segment(&mut v, 0xDA, &[0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
    v.extend_from_slice(&[0x00, 0x00, 0xFF, 0xD9]);
    v
}

fn push_segment(v: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    v.extend_from_slice(&[0xFF, marker]);
    v.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    v.extend_from_slice(payload);
}

/// Valid little-endian 16-bit grayscale TIFF, pixels zeroed, XMP in tag 700.
pub fn tiff_with_xmp(xmp: &str, width: u32, height: u32) -> Vec<u8> {
    tiff_with_xmp_gps(xmp, width, height, None)
}

/// [`tiff_with_xmp`] with a GPS IFD (tag 34853) holding `gps`.
pub fn tiff_with_xmp_gps(xmp: &str, width: u32, height: u32, gps: Option<Llh>) -> Vec<u8> {
    let xmp = xmp.as_bytes();
    let pixels = (width as u64 * height as u64 * 2) as u32;
    // Layout: header (8) | XMP | GPS IFD | pixel strip | IFD at the end, as many writers do.
    let xmp_off = 8u32;
    let mut gps_off = xmp_off + xmp.len() as u32;
    gps_off += gps_off % 2;
    let gps_bytes = gps.map(|p| gps_ifd(gps_off, p)).unwrap_or_default();
    let mut pix_off = gps_off + gps_bytes.len() as u32;
    pix_off += pix_off % 2;
    let ifd_off = pix_off + pixels + (pixels % 2);
    let mut entries: Vec<(u16, u16, u32, u32)> = vec![
        (256, 4, 1, width),
        (257, 4, 1, height),
        (258, 3, 1, 16),
        (259, 3, 1, 1),
        (262, 3, 1, 1),
        (273, 4, 1, pix_off),
        (277, 3, 1, 1),
        (278, 4, 1, height),
        (279, 4, 1, pixels),
        (700, 1, xmp.len() as u32, xmp_off),
    ];
    if gps.is_some() {
        entries.push((34853, 4, 1, gps_off));
    }
    let mut v = Vec::with_capacity(ifd_off as usize + 2 + entries.len() * 12 + 4);
    v.extend_from_slice(b"II");
    v.extend_from_slice(&42u16.to_le_bytes());
    v.extend_from_slice(&ifd_off.to_le_bytes());
    v.extend_from_slice(xmp);
    v.resize(gps_off as usize, 0);
    v.extend_from_slice(&gps_bytes);
    v.resize(pix_off as usize, 0);
    v.resize((pix_off + pixels) as usize, 0);
    v.resize(ifd_off as usize, 0);
    v.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, typ, count, value) in entries {
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&typ.to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
        // SHORT values sit in the first two bytes of the value field.
        if typ == 3 && count == 1 {
            v.extend_from_slice(&(value as u16).to_le_bytes());
            v.extend_from_slice(&[0, 0]);
        } else {
            v.extend_from_slice(&value.to_le_bytes());
        }
    }
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

/// One synthetic MRK line in the real-file layout (tab-separated, suffix tags).
pub fn mrk_line(
    index: u32,
    gps_s: f64,
    offset_ned_mm: [i32; 3],
    lat: f64,
    lon: f64,
    h: f64,
    flag: u16,
) -> String {
    let (week, tow) = week_tow(gps_s);
    format!(
        "{index}\t{tow:.6}\t[{week}]\t{:>6},N\t{:>6},E\t{:>6},V\t{lat:.8},Lat\t{lon:.8},Lon\t{h:.3},Ellh\t0.010000, 0.010000, 0.020000\t{flag},Q",
        offset_ned_mm[0], offset_ned_mm[1], offset_ned_mm[2]
    )
}

fn pseudo_uuid(seed: u64) -> String {
    // Deterministic, UUID-shaped; not a real v4 UUID.
    let a = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let b = a.rotate_left(17) ^ 0xD1B5_4A32_D192_ED03;
    format!(
        "{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        (a & 0xfff) as u16,
        (b >> 52) as u16 & 0xfff,
        b & 0xffff_ffff_ffff
    )
}

/// Write a synthetic flight into `dir`. Returns the paths written.
pub fn write_flight(dir: &Path, spec: &SyntheticFlight) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let mut written = Vec::new();
    let mut mrk = String::new();
    let (lat0, lon0, h0) = (46.3500, -72.5500, 120.0);
    for i in 0..spec.captures {
        let gps = spec.start_gps_s + i as f64 * spec.interval_s;
        let index = spec.first_index + i as u32;
        let stamp = datetime_from_gps_seconds(gps).format("%Y%m%d%H%M%S").to_string();
        let clock = if spec.clock_is_utc {
            gps - GPS_UTC_OFFSET_S
        } else {
            gps
        };
        let uuid = pseudo_uuid(index as u64 ^ (spec.start_gps_s as u64));
        // The aircraft's position, written in EXIF and XMP like the MRK's.
        let pos: Llh = (lat0 + i as f64 * 1.0e-4, lon0, h0);
        let mut base: Vec<(&str, String)> = vec![
            ("UTCAtExposure", format_dji_clock(clock)),
            ("GpsLatitude", format!("{:+.9}", pos.0)),
            ("GpsLongitude", format!("{:+.9}", pos.1)),
            ("AbsoluteAltitude", format!("{:+.3}", pos.2)),
            ("GimbalYawDegree", "+0.50".into()),
            ("GimbalPitchDegree", "-90.00".into()),
            ("GimbalRollDegree", "+0.00".into()),
            ("RtkFlag", "50".into()),
            ("RtkStdLon", "0.01000".into()),
            ("RtkStdLat", "0.01000".into()),
            ("RtkStdHgt", "0.02000".into()),
        ];
        if spec.with_uuid {
            base.push(("CaptureUUID", uuid));
        }

        let jpg = dir.join(format!("DJI_{stamp}_{index:04}_D.JPG"));
        fs::write(&jpg, jpeg_with_xmp_gps(&xmp_packet(&base), Some(pos)))?;
        written.push(jpg);

        if spec.multispectral {
            for (k, band) in Band::MAVIC_3M.iter().enumerate() {
                if *band == Band::Nir && spec.drop_nir_at.contains(&i) {
                    continue;
                }
                let token = match band {
                    Band::Green => "G",
                    Band::Red => "R",
                    Band::RedEdge => "RE",
                    _ => "NIR",
                };
                let mut attrs = base.clone();
                attrs[0].1 = format_dji_clock(clock + spec.band_delay_s);
                attrs.push(("BandName", band.label().to_string()));
                attrs.push(("SensorIndex", (k + 1).to_string()));
                let tif = dir.join(format!("DJI_{stamp}_{index:04}_MS_{token}.TIF"));
                let (w, h) = spec.band_size;
                fs::write(&tif, tiff_with_xmp_gps(&xmp_packet(&attrs), w, h, Some(pos)))?;
                written.push(tif);
            }
        }

        if !spec.drop_mrk_at.contains(&i) {
            let flag = if spec.float_at.contains(&i) { 34 } else { 50 };
            let lat = lat0 + i as f64 * 1.0e-4;
            mrk.push_str(&mrk_line(index, gps, [12, -3, 180], lat, lon0, h0, flag));
            mrk.push('\n');
        }
    }
    let first_stamp = datetime_from_gps_seconds(spec.start_gps_s)
        .format("%Y%m%d%H%M%S")
        .to_string();
    let mrk_path = dir.join(format!("DJI_{first_stamp}_Timestamp.MRK"));
    fs::write(&mrk_path, mrk)?;
    written.push(mrk_path);
    // Placeholder raw GNSS log and navigation file: empty, only their presence is checked.
    let raw = dir.join(format!("DJI_{first_stamp}_PPKRAW.bin"));
    fs::write(&raw, b"")?;
    written.push(raw);
    let nav = dir.join(format!("DJI_{first_stamp}_PPKNAV.nav"));
    fs::write(&nav, b"")?;
    written.push(nav);
    Ok(written)
}

/// Description of a synthetic RINEX observation file.
#[derive(Debug, Clone)]
pub struct SyntheticRinex {
    /// 2 or 3.
    pub major_version: u8,
    pub start_gps_s: f64,
    pub end_gps_s: f64,
    pub interval_s: f64,
    /// Write the `INTERVAL` header record.
    pub interval_in_header: bool,
    /// Observation codes per system for RINEX 3 (e.g. `('G', &["C1C", "L1C", "C2W", "L2W"])`);
    /// for RINEX 2 the codes of the first entry are used for every system.
    pub codes: Vec<(char, Vec<&'static str>)>,
    pub antenna_type: Option<&'static str>,
    pub antenna_height_m: f64,
    /// Approximate position as latitude, longitude (degrees) and ellipsoidal height (m).
    pub approx_llh: Option<(f64, f64, f64)>,
}

impl Default for SyntheticRinex {
    fn default() -> Self {
        let flight = SyntheticFlight::default();
        Self {
            major_version: 3,
            start_gps_s: flight.start_gps_s - 600.0,
            end_gps_s: flight.start_gps_s + 1800.0,
            interval_s: 1.0,
            interval_in_header: true,
            codes: vec![
                ('G', vec!["C1C", "L1C", "C2W", "L2W"]),
                ('R', vec!["C1C", "L1C", "C2P", "L2P"]),
                ('E', vec!["C1C", "L1C", "C5Q", "L5Q"]),
            ],
            antenna_type: Some("EMLIDREACHRS4   NONE"),
            antenna_height_m: 1.8,
            approx_llh: Some((46.3490, -72.5510, 60.0)),
        }
    }
}

fn hdr(content: &str, label: &str) -> String {
    format!("{content:<60}{label:<20}\n")
}

/// Write a small RINEX observation file (only epoch lines and dummy observation lines).
pub fn write_rinex(path: &Path, spec: &SyntheticRinex) -> io::Result<()> {
    use crate::rinex::geodetic_to_ecef;
    let v3 = spec.major_version >= 3;
    let mut s = String::new();
    let (ver, sys) = if v3 {
        ("     3.04", "M")
    } else {
        ("     2.11", "M")
    };
    s += &hdr(
        &format!("{ver}           OBSERVATION DATA    {sys}"),
        "RINEX VERSION / TYPE",
    );
    s += &hdr("azimut synthetic", "PGM / RUN BY / DATE");
    s += &hdr("BASE", "MARKER NAME");
    s += &hdr(
        &format!("{:<20}{:<20}{:<20}", "0001", "SYNTH RECEIVER", "1.0"),
        "REC # / TYPE / VERS",
    );
    s += &hdr(
        &format!("{:<20}{:<20}", "0001", spec.antenna_type.unwrap_or("")),
        "ANT # / TYPE",
    );
    if let Some((lat, lon, h)) = spec.approx_llh {
        let p = geodetic_to_ecef(lat, lon, h);
        s += &hdr(
            &format!("{:14.4}{:14.4}{:14.4}", p[0], p[1], p[2]),
            "APPROX POSITION XYZ",
        );
    }
    s += &hdr(
        &format!("{:14.4}{:14.4}{:14.4}", spec.antenna_height_m, 0.0, 0.0),
        "ANTENNA: DELTA H/E/N",
    );
    if v3 {
        for (sys, codes) in &spec.codes {
            let mut line = format!("{sys}  {:3}", codes.len());
            for c in codes {
                line += &format!(" {c:<3}");
            }
            s += &hdr(&line, "SYS / # / OBS TYPES");
        }
    } else {
        let codes: Vec<String> = spec.codes[0].1.iter().map(|c| c[..2].to_string()).collect();
        let mut line = format!("{:6}", codes.len());
        for c in &codes {
            line += &format!("    {c:<2}");
        }
        s += &hdr(&line, "# / TYPES OF OBSERV");
    }
    if spec.interval_in_header {
        s += &hdr(&format!("{:10.3}", spec.interval_s), "INTERVAL");
    }
    let t0 = datetime_from_gps_seconds(spec.start_gps_s);
    s += &hdr(
        &format!(
            "{}     GPS",
            t0.format("  %Y    %m    %d    %H    %M   %S.0000000")
        ),
        "TIME OF FIRST OBS",
    );
    s += &hdr("", "END OF HEADER");

    let sats: Vec<String> = spec.codes.iter().map(|(sys, _)| format!("{sys}01")).collect();
    let mut t = spec.start_gps_s;
    while t <= spec.end_gps_s + 1e-9 {
        let dt = datetime_from_gps_seconds(t);
        if v3 {
            s += &format!(
                "> {}  0{:3}\n",
                dt.format("%Y %m %d %H %M %S.0000000"),
                sats.len()
            );
            for sat in &sats {
                s += &format!("{sat}  20000000.000   105000000.000\n");
            }
        } else {
            let yy = dt.format("%y").to_string();
            let fmt = |v: u32| format!("{v:3}");
            use chrono::{Datelike, Timelike};
            s += &format!(
                " {yy}{}{}{}{}{:11.7}  0{:3}{}\n",
                fmt(dt.month()),
                fmt(dt.day()),
                fmt(dt.hour()),
                fmt(dt.minute()),
                dt.second() as f64,
                sats.len(),
                sats.concat()
            );
            for _ in &sats {
                s += "  20000000.000   105000000.000\n";
            }
        }
        t += spec.interval_s;
    }
    fs::write(path, s)
}
