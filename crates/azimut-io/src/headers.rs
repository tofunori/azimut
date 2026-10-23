//! Header-only metadata readers for JPEG and TIFF.
//!
//! Only the bytes that hold metadata are read. Pixel data is never decoded or even read,
//! which keeps scans of large multispectral TIFF sets fast, also over a network share.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::Error;

/// Marker that starts a standard XMP APP1 segment.
const XMP_APP1_ID: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
/// TIFF tag holding XMP.
const TAG_XMP: u16 = 700;
/// Upper bound on one metadata block, as a guard against corrupt offsets.
const MAX_BLOCK: u64 = 16 * 1024 * 1024;
/// Upper bound on the number of IFDs followed in a chain.
const MAX_IFDS: usize = 32;

/// Reader wrapper that counts the bytes actually read from disk.
pub struct CountingReader<R> {
    inner: R,
    count: u64,
}

impl<R> CountingReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner, count: 0 }
    }
    pub fn bytes_read(&self) -> u64 {
        self.count
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        Ok(n)
    }
}

impl<R: Seek> Seek for CountingReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// XMP packets found in a file, plus how many bytes were read to find them.
#[derive(Debug, Default, Clone)]
pub struct RawXmp {
    pub packets: Vec<String>,
    pub bytes_read: u64,
}

/// Read every XMP packet stored in a JPEG's APP1 segments, stopping at the image data.
pub fn read_jpeg_xmp(path: &Path) -> Result<RawXmp, Error> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    // Small buffer: we seek past large segments (EXIF thumbnails) instead of reading them.
    let mut r = BufReader::with_capacity(8 * 1024, CountingReader::new(file));
    let packets = jpeg_xmp(&mut r).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => Error::Format {
            path: path.to_path_buf(),
            expected: "JPEG",
        },
        _ => Error::io(path, e),
    })?;
    Ok(RawXmp {
        packets,
        bytes_read: r.get_ref().bytes_read(),
    })
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn jpeg_xmp<R: Read + Seek>(r: &mut BufReader<R>) -> io::Result<Vec<String>> {
    let mut b2 = [0u8; 2];
    r.read_exact(&mut b2)?;
    if b2 != [0xFF, 0xD8] {
        return Err(invalid("missing SOI"));
    }
    let mut packets = Vec::new();
    loop {
        // Find the next marker, skipping fill bytes.
        let mut byte = [0u8; 1];
        r.read_exact(&mut byte)?;
        if byte[0] != 0xFF {
            return Err(invalid("expected marker"));
        }
        let mut marker = 0xFFu8;
        while marker == 0xFF {
            r.read_exact(&mut byte)?;
            marker = byte[0];
        }
        match marker {
            // Start of scan or end of image: metadata is over.
            0xDA | 0xD9 => break,
            // Markers without a length field.
            0x01 | 0xD0..=0xD7 => continue,
            _ => {}
        }
        r.read_exact(&mut b2)?;
        let len = u16::from_be_bytes(b2) as u64;
        if len < 2 {
            return Err(invalid("bad segment length"));
        }
        let payload = len - 2;
        if marker == 0xE1 && payload as usize >= XMP_APP1_ID.len() {
            let mut id = vec![0u8; XMP_APP1_ID.len()];
            r.read_exact(&mut id)?;
            let rest = payload - id.len() as u64;
            if id == XMP_APP1_ID {
                let mut data = vec![0u8; rest as usize];
                r.read_exact(&mut data)?;
                packets.push(String::from_utf8_lossy(&data).into_owned());
            } else {
                r.seek_relative(rest as i64)?;
            }
        } else {
            r.seek_relative(payload as i64)?;
        }
    }
    Ok(packets)
}

/// Read the XMP tag (700) of every IFD in a TIFF or BigTIFF chain.
pub fn read_tiff_xmp(path: &Path) -> Result<RawXmp, Error> {
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
    let mut r = BufReader::with_capacity(8 * 1024, CountingReader::new(file));
    let packets = tiff_xmp(&mut r, len).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => Error::Format {
            path: path.to_path_buf(),
            expected: "TIFF",
        },
        _ => Error::io(path, e),
    })?;
    Ok(RawXmp {
        packets,
        bytes_read: r.get_ref().bytes_read(),
    })
}

#[derive(Clone, Copy)]
pub(crate) struct Endian(pub(crate) bool); // true = little endian

impl Endian {
    pub(crate) fn u16(self, b: &[u8]) -> u16 {
        let a = [b[0], b[1]];
        if self.0 {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        }
    }
    pub(crate) fn u32(self, b: &[u8]) -> u32 {
        let a = [b[0], b[1], b[2], b[3]];
        if self.0 {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    }
    fn u64(self, b: &[u8]) -> u64 {
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[..8]);
        if self.0 {
            u64::from_le_bytes(a)
        } else {
            u64::from_be_bytes(a)
        }
    }
}

fn tiff_xmp<R: Read + Seek>(r: &mut BufReader<R>, file_len: u64) -> io::Result<Vec<String>> {
    let mut hdr = [0u8; 8];
    r.read_exact(&mut hdr)?;
    let endian = match &hdr[0..2] {
        b"II" => Endian(true),
        b"MM" => Endian(false),
        _ => return Err(invalid("bad byte order")),
    };
    let magic = endian.u16(&hdr[2..4]);
    let big = match magic {
        42 => false,
        43 => true,
        _ => return Err(invalid("bad TIFF magic")),
    };
    let mut next_ifd: u64 = if big {
        let mut rest = [0u8; 8];
        r.read_exact(&mut rest)?;
        endian.u64(&rest)
    } else {
        endian.u32(&hdr[4..8]) as u64
    };

    let (count_size, entry_size, next_size) = if big { (8u64, 20u64, 8u64) } else { (2, 12, 4) };
    let mut packets = Vec::new();
    let mut seen = Vec::new();
    while next_ifd != 0 && seen.len() < MAX_IFDS {
        if seen.contains(&next_ifd) || next_ifd >= file_len {
            break; // loop or offset past the end of the file
        }
        seen.push(next_ifd);
        r.seek(SeekFrom::Start(next_ifd))?;
        let mut cbuf = vec![0u8; count_size as usize];
        r.read_exact(&mut cbuf)?;
        let n = if big {
            endian.u64(&cbuf)
        } else {
            endian.u16(&cbuf) as u64
        };
        if n == 0 || n > 4096 {
            break;
        }
        let mut entries = vec![0u8; (n * entry_size) as usize];
        r.read_exact(&mut entries)?;
        let mut nbuf = vec![0u8; next_size as usize];
        r.read_exact(&mut nbuf)?;
        let following = if big {
            endian.u64(&nbuf)
        } else {
            endian.u32(&nbuf) as u64
        };

        for e in entries.chunks_exact(entry_size as usize) {
            let tag = endian.u16(&e[0..2]);
            if tag != TAG_XMP {
                continue;
            }
            let typ = endian.u16(&e[2..4]);
            // XMP is BYTE (1) or UNDEFINED (7); one byte per element.
            if typ != 1 && typ != 7 {
                continue;
            }
            let (count, inline_cap, value) = if big {
                (endian.u64(&e[4..12]), 8u64, &e[12..20])
            } else {
                (endian.u32(&e[4..8]) as u64, 4u64, &e[8..12])
            };
            if count == 0 || count > MAX_BLOCK {
                continue;
            }
            let data = if count <= inline_cap {
                value[..count as usize].to_vec()
            } else {
                let off = if big {
                    endian.u64(value)
                } else {
                    endian.u32(value) as u64
                };
                if off.saturating_add(count) > file_len {
                    continue;
                }
                r.seek(SeekFrom::Start(off))?;
                let mut d = vec![0u8; count as usize];
                r.read_exact(&mut d)?;
                d
            };
            packets.push(String::from_utf8_lossy(&data).into_owned());
        }
        next_ifd = following;
    }
    Ok(packets)
}
