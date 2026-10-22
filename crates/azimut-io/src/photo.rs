//! Per-image metadata, read from headers only.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::headers::{read_jpeg_xmp, read_tiff_xmp};
use crate::time::parse_dji_clock;
use crate::xmp::XmpFields;

/// Spectral band of a multispectral image.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Band {
    Green,
    Red,
    RedEdge,
    Nir,
    Other(String),
}

impl Band {
    /// Bands of the Mavic 3 Multispectral camera, in DJI's sensor-index order (1 to 4).
    pub const MAVIC_3M: [Band; 4] = [Band::Green, Band::Red, Band::RedEdge, Band::Nir];

    /// Read a band from an XMP `BandName` value (`Green`, `Red`, `RedEdge`, `NIR`).
    pub fn from_xmp(v: &str) -> Band {
        let k: String = v
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match k.as_str() {
            "green" | "g" => Band::Green,
            "red" | "r" => Band::Red,
            "rededge" | "re" => Band::RedEdge,
            "nir" | "nearinfrared" => Band::Nir,
            _ => Band::Other(v.trim().to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Band::Green => "Green",
            Band::Red => "Red",
            Band::RedEdge => "RedEdge",
            Band::Nir => "NIR",
            Band::Other(s) => s,
        }
    }
}

/// What an image is, as far as grouping is concerned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "band")]
pub enum ImageKind {
    /// A JPEG without a band name (visible camera; wide, zoom or thermal on some models).
    Jpeg,
    /// One band of a multispectral capture.
    Band(Band),
    /// A TIFF without a recognised band.
    Tiff,
}

/// Metadata of one image file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Photo {
    pub path: PathBuf,
    pub file_name: String,
    pub kind: ImageKind,
    /// Size on disk, bytes.
    pub size: u64,
    /// Bytes read to get the metadata.
    pub bytes_read: u64,
    /// XMP `CaptureUUID`, shared by all images of one Mavic 3M capture.
    pub capture_uuid: Option<String>,
    /// XMP `GimbalYawDegree`, `GimbalPitchDegree`, `GimbalRollDegree`, degrees.
    pub gimbal_ypr_deg: Option<[f64; 3]>,
    /// Raw XMP `UTCAtExposure` string.
    pub exposure_raw: Option<String>,
    /// `UTCAtExposure` as seconds since the GPS epoch, read "as written".
    pub exposure_clock_s: Option<f64>,
    /// Sequence number taken from a DJI file name (`DJI_<datetime>_<NNNN>_<suffix>`).
    pub sequence: Option<u32>,
    /// File name suffix after the sequence number, e.g. `D`, `W`, `T`, `MS_NIR`.
    pub suffix: Option<String>,
    /// Problem met while reading, if any.
    pub error: Option<String>,
}

fn name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^DJI_\d{8,14}_(\d{3,5})(?:_([A-Z0-9_]+))?\.[a-z]+$").expect("valid regex")
    })
}

/// Sequence number and suffix from a DJI file name.
pub fn parse_file_name(name: &str) -> (Option<u32>, Option<String>) {
    match name_re().captures(name) {
        Some(c) => (
            c.get(1).and_then(|m| m.as_str().parse().ok()),
            c.get(2).map(|m| m.as_str().to_ascii_uppercase()),
        ),
        None => (None, None),
    }
}

fn band_from_suffix(suffix: &str) -> Option<Band> {
    let s = suffix.trim_start_matches("MS_");
    match s {
        "G" => Some(Band::Green),
        "R" => Some(Band::Red),
        "RE" => Some(Band::RedEdge),
        "NIR" => Some(Band::Nir),
        _ => None,
    }
}

/// Image file types Azimut reads.
pub fn is_jpeg(path: &Path) -> bool {
    matches!(ext(path).as_deref(), Some("jpg" | "jpeg"))
}

pub fn is_tiff(path: &Path) -> bool {
    matches!(ext(path).as_deref(), Some("tif" | "tiff"))
}

pub(crate) fn ext(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// Read the metadata of one image. Never fails: problems are recorded in `error`.
pub fn read_photo(path: &Path) -> Photo {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (sequence, suffix) = parse_file_name(&file_name);
    let tiff = is_tiff(path);
    let raw = if tiff {
        read_tiff_xmp(path)
    } else {
        read_jpeg_xmp(path)
    };
    let (fields, bytes_read, error) = match raw {
        Ok(r) => (XmpFields::parse(&r.packets), r.bytes_read, None),
        Err(e) => (XmpFields::default(), 0, Some(e.to_string())),
    };

    let band = fields
        .get_local("BandName")
        .map(Band::from_xmp)
        .filter(|b| !matches!(b, Band::Other(s) if s.eq_ignore_ascii_case("rgb")))
        .or_else(|| {
            if tiff {
                suffix.as_deref().and_then(band_from_suffix)
            } else {
                None
            }
        });
    let kind = match (band, tiff) {
        (Some(b), _) => ImageKind::Band(b),
        (None, true) => ImageKind::Tiff,
        (None, false) => ImageKind::Jpeg,
    };
    let exposure_raw = fields.get_local("UTCAtExposure").map(str::to_string);
    let exposure_clock_s = exposure_raw.as_deref().and_then(parse_dji_clock);
    let capture_uuid = fields
        .get_local("CaptureUUID")
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let angle = |k: &str| {
        fields
            .get_local(k)
            .and_then(|v| v.trim().trim_start_matches('+').parse::<f64>().ok())
    };
    let gimbal_ypr_deg = match (
        angle("GimbalYawDegree"),
        angle("GimbalPitchDegree"),
        angle("GimbalRollDegree"),
    ) {
        (Some(y), Some(p), Some(r)) => Some([y, p, r]),
        _ => None,
    };
    let error = error.or_else(|| {
        if fields.qualified.is_empty() {
            Some("no XMP metadata found".to_string())
        } else {
            None
        }
    });

    Photo {
        path: path.to_path_buf(),
        file_name,
        kind,
        size,
        bytes_read,
        capture_uuid,
        gimbal_ypr_deg,
        exposure_raw,
        exposure_clock_s,
        sequence,
        suffix,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dji_names() {
        assert_eq!(
            parse_file_name("DJI_20250611160251_0001_MS_NIR.TIF"),
            (Some(1), Some("MS_NIR".into()))
        );
        assert_eq!(
            parse_file_name("DJI_20250611160251_0042_D.JPG"),
            (Some(42), Some("D".into()))
        );
        assert_eq!(
            parse_file_name("dji_20250611160251_0042_w.jpg"),
            (Some(42), Some("W".into()))
        );
        assert_eq!(parse_file_name("IMG_0001.JPG"), (None, None));
    }

    #[test]
    fn band_names() {
        assert_eq!(Band::from_xmp("RedEdge"), Band::RedEdge);
        assert_eq!(Band::from_xmp("Red Edge"), Band::RedEdge);
        assert_eq!(Band::from_xmp("NIR"), Band::Nir);
        assert_eq!(band_from_suffix("MS_G"), Some(Band::Green));
        assert_eq!(band_from_suffix("RE"), Some(Band::RedEdge));
        assert_eq!(band_from_suffix("D"), None);
    }
}
