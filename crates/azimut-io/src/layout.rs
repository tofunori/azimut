//! Where the position fields of a JPEG or TIFF sit in the file, without reading pixels.
//!
//! Used to write positions in place: the XMP packet (JPEG APP1, TIFF tag 700) and the
//! entries of the EXIF GPS IFD (pointer tag 34853 in IFD0, inside the JPEG `Exif` APP1 or
//! in the TIFF itself). Offsets are absolute file offsets. Classic TIFF only: a BigTIFF
//! header is reported as unsupported.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::Error;
use crate::headers::Endian;

const XMP_APP1_ID: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const EXIF_APP1_ID: &[u8] = b"Exif\0\0";
const TAG_XMP: u16 = 700;
const TAG_GPS_IFD: u16 = 34853;
/// Upper bound on entries in one IFD, as a guard against corrupt files.
const MAX_ENTRIES: u16 = 1024;

/// One entry of the GPS IFD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpsEntry {
    pub tag: u16,
    /// TIFF field type (1 BYTE, 2 ASCII, 5 RATIONAL...).
    pub field_type: u16,
    pub count: u32,
    /// Absolute offset of the value bytes (inside the entry when they fit in 4 bytes).
    pub value_offset: u64,
}

impl GpsEntry {
    /// Size of the value in bytes, for the field types used by GPS tags.
    pub fn value_len(&self) -> Option<u64> {
        let unit = match self.field_type {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        Some(unit * self.count as u64)
    }
}

/// Position fields found in one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldLayout {
    /// Byte order of the EXIF/TIFF structure.
    pub little_endian: bool,
    /// Offset and length of each XMP packet.
    pub xmp: Vec<(u64, u64)>,
    /// GPS IFD entries; empty when there is no GPS IFD.
    pub gps: Vec<GpsEntry>,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Locate the XMP packets and GPS entries of a JPEG or TIFF.
pub fn locate_fields(path: &Path) -> Result<FieldLayout, Error> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
    let mut r = BufReader::with_capacity(8 * 1024, file);
    let mut head = [0u8; 4];
    r.read_exact(&mut head).map_err(|e| Error::io(path, e))?;
    r.seek(SeekFrom::Start(0)).map_err(|e| Error::io(path, e))?;
    let result = if head[..2] == [0xFF, 0xD8] {
        jpeg_layout(&mut r, len)
    } else {
        tiff_layout(&mut r, 0, len, true)
    };
    result.map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => Error::Parse {
            path: path.to_path_buf(),
            reason: format!("cannot locate metadata fields: {e}"),
        },
        _ => Error::io(path, e),
    })
}

fn jpeg_layout<R: Read + Seek>(r: &mut R, len: u64) -> io::Result<FieldLayout> {
    let mut out = FieldLayout::default();
    let mut pos = 2u64;
    loop {
        r.seek(SeekFrom::Start(pos))?;
        let mut m = [0u8; 2];
        r.read_exact(&mut m)?;
        if m[0] != 0xFF {
            return Err(invalid("expected a JPEG marker"));
        }
        let mut marker = m[1];
        pos += 2;
        while marker == 0xFF {
            let mut b = [0u8; 1];
            r.read_exact(&mut b)?;
            marker = b[0];
            pos += 1;
        }
        match marker {
            0xDA | 0xD9 => break,
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        let mut l = [0u8; 2];
        r.read_exact(&mut l)?;
        let seg_len = u16::from_be_bytes(l) as u64;
        if seg_len < 2 {
            return Err(invalid("bad segment length"));
        }
        let payload = pos + 2;
        let payload_len = seg_len - 2;
        if marker == 0xE1 {
            let mut id = vec![0u8; XMP_APP1_ID.len().min(payload_len as usize)];
            r.read_exact(&mut id)?;
            if id == XMP_APP1_ID {
                let off = payload + XMP_APP1_ID.len() as u64;
                out.xmp.push((off, payload_len - XMP_APP1_ID.len() as u64));
            } else if id.starts_with(EXIF_APP1_ID) && out.gps.is_empty() {
                let base = payload + EXIF_APP1_ID.len() as u64;
                let exif = tiff_layout(r, base, (payload + payload_len).min(len), false)?;
                out.little_endian = exif.little_endian;
                out.gps = exif.gps;
            }
        }
        pos = payload + payload_len;
    }
    Ok(out)
}

/// Read IFD0 of a TIFF structure starting at `base`; offsets in it are relative to `base`.
fn tiff_layout<R: Read + Seek>(r: &mut R, base: u64, end: u64, with_xmp: bool) -> io::Result<FieldLayout> {
    r.seek(SeekFrom::Start(base))?;
    let mut hdr = [0u8; 8];
    r.read_exact(&mut hdr)?;
    let endian = match &hdr[0..2] {
        b"II" => Endian(true),
        b"MM" => Endian(false),
        _ => return Err(invalid("bad byte order")),
    };
    match endian.u16(&hdr[2..4]) {
        42 => {}
        43 => return Err(invalid("BigTIFF is not supported for writing")),
        _ => return Err(invalid("bad TIFF magic")),
    }
    let mut out = FieldLayout {
        little_endian: endian.0,
        ..Default::default()
    };
    let ifd0 = base + endian.u32(&hdr[4..8]) as u64;
    let entries = read_ifd(r, endian, ifd0, end)?;
    for (_, e) in &entries {
        let tag = endian.u16(&e[0..2]);
        let typ = endian.u16(&e[2..4]);
        let count = endian.u32(&e[4..8]) as u64;
        if tag == TAG_XMP && with_xmp && (typ == 1 || typ == 7) && count > 4 {
            let off = base + endian.u32(&e[8..12]) as u64;
            if off + count <= end {
                out.xmp.push((off, count));
            }
        }
        if tag == TAG_GPS_IFD && (typ == 4 || typ == 13) && count == 1 {
            let gps_at = base + endian.u32(&e[8..12]) as u64;
            for (at, g) in read_ifd(r, endian, gps_at, end)? {
                let entry = GpsEntry {
                    tag: endian.u16(&g[0..2]),
                    field_type: endian.u16(&g[2..4]),
                    count: endian.u32(&g[4..8]),
                    value_offset: 0,
                };
                let Some(size) = entry.value_len() else {
                    continue;
                };
                let value_offset = if size <= 4 {
                    at + 8
                } else {
                    base + endian.u32(&g[8..12]) as u64
                };
                if value_offset + size <= end {
                    out.gps.push(GpsEntry {
                        value_offset,
                        ..entry
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Entries of one IFD, each with its absolute offset.
fn read_ifd<R: Read + Seek>(
    r: &mut R,
    endian: Endian,
    at: u64,
    end: u64,
) -> io::Result<Vec<(u64, [u8; 12])>> {
    if at + 2 > end {
        return Err(invalid("IFD offset past the end"));
    }
    r.seek(SeekFrom::Start(at))?;
    let mut n = [0u8; 2];
    r.read_exact(&mut n)?;
    let n = endian.u16(&n);
    if n == 0 || n > MAX_ENTRIES || at + 2 + 12 * n as u64 > end {
        return Err(invalid("bad IFD"));
    }
    let mut v = Vec::with_capacity(n as usize);
    for i in 0..n as u64 {
        let mut e = [0u8; 12];
        r.read_exact(&mut e)?;
        v.push((at + 2 + 12 * i, e));
    }
    Ok(v)
}
