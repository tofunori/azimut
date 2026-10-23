//! Positions written into the images, in place, without changing anything else.
//!
//! The existing position fields are overwritten with values of the same size, so the file
//! keeps its length and every other byte:
//!
//! - EXIF GPS: `GPSLatitude`, `GPSLongitude` (degrees, minutes, seconds to 1e-6 s),
//!   `GPSAltitude` (to 0.1 mm) and their reference letters.
//! - DJI XMP: `drone-dji:GpsLatitude`, `GpsLongitude`, `AbsoluteAltitude`, and the 1-sigma
//!   `RtkStdLon` (East), `RtkStdLat` (North) and `RtkStdHgt` (Up), written in DJI's number
//!   format. Values change length only inside the packet's whitespace padding, and the
//!   attribute form is kept: OpenDroneMap reads DJI fields only as attributes.
//!
//! Heights are ellipsoidal in the solution's frame, as DJI writes them (MRK `Ellh`,
//! `GPSAltitude` and `AbsoluteAltitude` hold the same value). `GPSMapDatum`, `RtkFlag` and
//! the other fields are left as they are. A file is written only when it carries every
//! field, the new XMP parses to the same fields apart from the written ones, and the camera
//! has an uncertainty. The original bytes of every change are returned so that the edit can
//! be undone ([`revert`]). See SOURCES.md (Writing positions into images).

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use azimut_io::layout::{locate_fields, FieldLayout, GpsEntry};
use azimut_io::xmp::XmpFields;
use azimut_solve::camera::CameraCentre;
use azimut_solve::FlightSolution;
use serde::{Deserialize, Serialize};

/// XMP fields written, in the order of the values given to [`xmp_values`].
pub const XMP_FIELDS: [&str; 6] = [
    "drone-dji:GpsLatitude",
    "drone-dji:GpsLongitude",
    "drone-dji:AbsoluteAltitude",
    "drone-dji:RtkStdLon",
    "drone-dji:RtkStdLat",
    "drone-dji:RtkStdHgt",
];

/// Bytes replaced at one offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    pub offset: u64,
    /// Original bytes, hexadecimal.
    pub old: String,
    /// Written bytes, hexadecimal.
    pub new: String,
}

/// Every change made to one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEdit {
    pub path: PathBuf,
    /// File length, unchanged by the edit.
    pub len: u64,
    pub patches: Vec<Patch>,
}

/// Outcome of writing a flight's positions.
#[derive(Debug, Default)]
pub struct ImageWriteReport {
    pub written: Vec<FileEdit>,
    pub skipped: Vec<(PathBuf, String)>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd hexadecimal length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// XMP values in DJI's number format, in the order of [`XMP_FIELDS`].
pub fn xmp_values(c: &CameraCentre, sigma_enu_m: [f64; 3]) -> [String; 6] {
    let [e, n, u] = sigma_enu_m;
    [
        format!("{:+.9}", c.lat_deg),
        format!("{:+.9}", c.lon_deg),
        format!("{:+.3}", c.h_m),
        format!("{e:.5}"),
        format!("{n:.5}"),
        format!("{u:.5}"),
    ]
}

/// Replace attribute values in an XMP packet, keeping its length by taking from or giving
/// back to the whitespace padding before `<?xpacket end`.
pub fn patch_xmp(packet: &[u8], fields: &[(&str, String)]) -> Result<Vec<u8>, String> {
    let find_all = |pat: &[u8]| -> Vec<usize> {
        packet
            .windows(pat.len())
            .enumerate()
            .filter(|(_, w)| *w == pat)
            .map(|(i, _)| i)
            .collect()
    };
    let mut edits: Vec<(usize, usize, &str)> = Vec::new(); // value start, value end, new value
    for (name, value) in fields {
        let mut hits = Vec::new();
        for quote in *b"\"'" {
            let pat = [name.as_bytes(), b"=", &[quote]].concat();
            for at in find_all(&pat) {
                let start = at + pat.len();
                let Some(len) = packet[start..].iter().position(|&b| b == quote) else {
                    return Err(format!("unterminated XMP attribute {name}"));
                };
                hits.push((start, start + len));
            }
        }
        match hits.len() {
            1 => edits.push((hits[0].0, hits[0].1, value.as_str())),
            0 if !find_all(format!("<{name}>").as_bytes()).is_empty() => {
                return Err(format!(
                    "{name} is an XMP element, not an attribute: the file was rewritten by \
                     another tool"
                ))
            }
            0 => return Err(format!("no {name} in the XMP")),
            _ => return Err(format!("{name} appears more than once in the XMP")),
        }
    }
    edits.sort_by_key(|e| e.0);
    let mut out = Vec::with_capacity(packet.len());
    let mut at = 0;
    for (start, end, value) in &edits {
        out.extend_from_slice(&packet[at..*start]);
        out.extend_from_slice(value.as_bytes());
        at = *end;
    }
    out.extend_from_slice(&packet[at..]);

    // Give the length difference back to the padding.
    let tail = b"<?xpacket end";
    let end = out
        .windows(tail.len())
        .rposition(|w| w == tail)
        .ok_or("no <?xpacket end> in the XMP")?;
    let pad_start = out[..end]
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    let padding = end - pad_start;
    if out.len() > packet.len() {
        let extra = out.len() - packet.len();
        if extra > padding {
            return Err(format!(
                "the XMP padding ({padding} bytes) is too small for the new values"
            ));
        }
        out.drain(end - extra..end);
    } else {
        let missing = packet.len() - out.len();
        out.splice(end..end, std::iter::repeat_n(b' ', missing));
    }
    debug_assert_eq!(out.len(), packet.len());
    Ok(out)
}

fn put_u32(little: bool, v: u32) -> [u8; 4] {
    if little {
        v.to_le_bytes()
    } else {
        v.to_be_bytes()
    }
}

/// Degrees, minutes and seconds (to 1e-6 s) as three TIFF RATIONALs.
pub fn dms_rationals(deg: f64, little: bool) -> Vec<u8> {
    let a = deg.abs();
    let mut d = a.floor() as u32;
    let minutes = (a - d as f64) * 60.0;
    let mut m = minutes.floor() as u32;
    let mut micro_s = ((minutes - m as f64) * 60.0 * 1e6).round() as u32;
    if micro_s >= 60_000_000 {
        micro_s -= 60_000_000;
        m += 1;
    }
    if m >= 60 {
        m -= 60;
        d += 1;
    }
    [(d, 1), (m, 1), (micro_s, 1_000_000)]
        .iter()
        .flat_map(|&(n, q)| [put_u32(little, n), put_u32(little, q)])
        .flatten()
        .collect()
}

/// Read back three DMS rationals as degrees.
pub fn rationals_deg(b: &[u8], little: bool) -> f64 {
    let u = |i: usize| {
        let a = [b[i], b[i + 1], b[i + 2], b[i + 3]];
        if little {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    };
    let r = |k: usize| u(8 * k) as f64 / u(8 * k + 4).max(1) as f64;
    r(0) + r(1) / 60.0 + r(2) / 3600.0
}

/// New bytes for the EXIF GPS entries, as (entry, bytes).
fn gps_values(layout: &FieldLayout, c: &CameraCentre) -> Result<Vec<(GpsEntry, Vec<u8>)>, String> {
    let little = layout.little_endian;
    let entry = |tag: u16, field_type: u16, count: u32| -> Result<GpsEntry, String> {
        let e = layout
            .gps
            .iter()
            .find(|e| e.tag == tag)
            .ok_or_else(|| format!("no EXIF GPS tag {tag}"))?;
        if e.field_type != field_type || e.count != count {
            return Err(format!(
                "EXIF GPS tag {tag} has type {} x {}, expected {field_type} x {count}",
                e.field_type, e.count
            ));
        }
        Ok(*e)
    };
    let alt_units = (c.h_m.abs() * 1e4).round();
    if alt_units > u32::MAX as f64 {
        return Err("height out of the EXIF range".into());
    }
    Ok(vec![
        (
            entry(1, 2, 2)?,
            vec![if c.lat_deg < 0.0 { b'S' } else { b'N' }, 0],
        ),
        (entry(2, 5, 3)?, dms_rationals(c.lat_deg, little)),
        (
            entry(3, 2, 2)?,
            vec![if c.lon_deg < 0.0 { b'W' } else { b'E' }, 0],
        ),
        (entry(4, 5, 3)?, dms_rationals(c.lon_deg, little)),
        (entry(5, 1, 1)?, vec![u8::from(c.h_m < 0.0)]),
        (
            entry(6, 5, 1)?,
            [put_u32(little, alt_units as u32), put_u32(little, 10_000)].concat(),
        ),
    ])
}

/// Split two equal-length buffers into runs that differ, merging runs closer than 16 bytes.
fn diff_runs(offset: u64, old: &[u8], new: &[u8]) -> Vec<Patch> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for i in (0..old.len()).filter(|&i| old[i] != new[i]) {
        match out.last_mut() {
            Some(last) if i - last.1 < 16 => last.1 = i + 1,
            _ => out.push((i, i + 1)),
        }
    }
    out.into_iter()
        .map(|(a, b)| Patch {
            offset: offset + a as u64,
            old: hex(&old[a..b]),
            new: hex(&new[a..b]),
        })
        .collect()
}

fn read_at(f: &mut std::fs::File, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(offset))?;
    let mut b = vec![0u8; len as usize];
    f.read_exact(&mut b)?;
    Ok(b)
}

/// Work out the edit that writes a camera centre into one file, without writing anything.
pub fn plan(path: &Path, c: &CameraCentre) -> Result<FileEdit, String> {
    let sigma = c
        .sigma_enu_m
        .ok_or("no uncertainty: DJI's RtkStd fields would keep the aircraft's values")?;
    let layout = locate_fields(path).map_err(|e| e.to_string())?;
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut patches = Vec::new();

    // XMP: the one packet holding the DJI position.
    let mut packets = Vec::new();
    for &(off, n) in &layout.xmp {
        let bytes = read_at(&mut f, off, n).map_err(|e| e.to_string())?;
        if bytes.windows(21).any(|w| w == b"drone-dji:GpsLatitude") {
            packets.push((off, bytes));
        }
    }
    let [(off, old)] = packets.as_slice() else {
        return Err(format!(
            "{} XMP packets with a DJI position, expected one",
            packets.len()
        ));
    };
    let values = xmp_values(c, sigma);
    let fields: Vec<(&str, String)> = XMP_FIELDS.iter().copied().zip(values.clone()).collect();
    let new = patch_xmp(old, &fields)?;
    let before = XmpFields::parse(&[String::from_utf8_lossy(old)]).qualified;
    let after = XmpFields::parse(&[String::from_utf8_lossy(&new)]).qualified;
    for (k, v) in &before {
        if !XMP_FIELDS.contains(&k.as_str()) && after.get(k) != Some(v) {
            return Err(format!("XMP field {k} would change; not written"));
        }
    }
    if after.len() != before.len() {
        return Err("the XMP would gain or lose fields; not written".into());
    }
    for (k, v) in XMP_FIELDS.iter().zip(&values) {
        if after.get(*k) != Some(v) {
            return Err(format!("XMP field {k} not written as expected"));
        }
    }
    patches.extend(diff_runs(*off, old, &new));

    // EXIF GPS.
    for (e, bytes) in gps_values(&layout, c)? {
        let old = read_at(&mut f, e.value_offset, bytes.len() as u64).map_err(|e| e.to_string())?;
        patches.extend(diff_runs(e.value_offset, &old, &bytes));
    }
    Ok(FileEdit {
        // Absolute, so that the record can be replayed from any folder.
        path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        len,
        patches,
    })
}

/// Write `to` bytes over `from` bytes at every patch, after checking that the file still
/// holds `from` everywhere. Re-reads every patch afterwards.
fn swap(edit: &FileEdit, forward: bool) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&edit.path)
        .map_err(|e| e.to_string())?;
    if f.metadata().map_err(|e| e.to_string())?.len() != edit.len {
        return Err("file length changed since the edit was planned".into());
    }
    let mut plan = Vec::new();
    for p in &edit.patches {
        let (from, to) = if forward {
            (unhex(&p.old)?, unhex(&p.new)?)
        } else {
            (unhex(&p.new)?, unhex(&p.old)?)
        };
        let now = read_at(&mut f, p.offset, from.len() as u64).map_err(|e| e.to_string())?;
        if now != from {
            return Err(format!(
                "bytes at offset {} are not the expected ones; file left unchanged",
                p.offset
            ));
        }
        plan.push((p.offset, to));
    }
    for (offset, to) in &plan {
        f.seek(SeekFrom::Start(*offset)).map_err(|e| e.to_string())?;
        f.write_all(to).map_err(|e| e.to_string())?;
    }
    f.sync_all().map_err(|e| e.to_string())?;
    for (offset, to) in &plan {
        if &read_at(&mut f, *offset, to.len() as u64).map_err(|e| e.to_string())? != to {
            return Err(format!("read-back mismatch at offset {offset}"));
        }
    }
    Ok(())
}

/// Apply a planned edit.
pub fn apply(edit: &FileEdit) -> Result<(), String> {
    swap(edit, true)
}

/// Undo an applied edit. An edit that is already undone is left alone.
pub fn revert(edit: &FileEdit) -> Result<bool, String> {
    match swap(edit, false) {
        Ok(()) => Ok(true),
        Err(e) => {
            // Already holding the original bytes?
            let back = FileEdit {
                patches: edit
                    .patches
                    .iter()
                    .map(|p| Patch {
                        old: p.old.clone(),
                        new: p.old.clone(),
                        ..p.clone()
                    })
                    .collect(),
                ..edit.clone()
            };
            if swap(&back, true).is_ok() {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

/// Plan the edits of every positioned file of a flight, without writing anything.
pub fn plan_flight(sol: &FlightSolution) -> ImageWriteReport {
    let mut report = ImageWriteReport::default();
    for e in &sol.exposures {
        for f in &e.files {
            let Some(c) = f.camera else {
                continue;
            };
            match plan(&f.path, &c) {
                Ok(edit) => report.written.push(edit),
                Err(reason) => report.skipped.push((f.path.clone(), reason)),
            }
        }
    }
    report
}
