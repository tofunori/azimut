//! Grouping images into captures (one trigger of the camera).
//!
//! A Mavic 3M trigger writes one RGB JPEG and four band TIFFs that share the XMP
//! `CaptureUUID`. Images are grouped on that UUID first; when it is missing, on the sequence
//! number in the DJI file name. Band identity comes from XMP `BandName`, the file name only
//! serves as a fallback. See SOURCES.md (Mavic 3M).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::photo::{Band, ImageKind, Photo};

/// How the images of a capture were tied together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKey {
    CaptureUuid,
    Sequence,
    FileName,
}

/// All images written by one trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    /// Indices into the flight's photo list.
    pub members: Vec<usize>,
    pub key: GroupKey,
    /// Exposure clock reading of the capture (from the JPEG when there is one).
    pub exposure_clock_s: Option<f64>,
    /// Smallest sequence number among members.
    pub sequence: Option<u32>,
    pub jpegs: usize,
    pub bands: Vec<Band>,
    /// Largest spread of exposure clock readings among members, seconds.
    pub clock_spread_s: f64,
}

impl Capture {
    pub fn is_multispectral(&self) -> bool {
        !self.bands.is_empty()
    }

    /// Mavic 3M bands missing from this capture.
    pub fn missing_bands(&self) -> Vec<Band> {
        Band::MAVIC_3M
            .iter()
            .filter(|b| !self.bands.contains(b))
            .cloned()
            .collect()
    }

    /// True when a multispectral capture has exactly one JPEG and the four Mavic 3M bands.
    pub fn is_complete_m3m(&self) -> bool {
        self.jpegs == 1 && self.bands.len() == 4 && self.missing_bands().is_empty()
    }
}

/// Group a flight's photos into captures, sorted by exposure time then sequence number.
pub fn group(photos: &[Photo]) -> Vec<Capture> {
    let mut by_key: BTreeMap<(u8, String), (GroupKey, Vec<usize>)> = BTreeMap::new();
    for (i, p) in photos.iter().enumerate() {
        let (rank, key, kind) = if let Some(u) = &p.capture_uuid {
            (0u8, u.clone(), GroupKey::CaptureUuid)
        } else if let Some(s) = p.sequence {
            (1u8, format!("{s:06}"), GroupKey::Sequence)
        } else {
            (2u8, p.file_name.clone(), GroupKey::FileName)
        };
        by_key
            .entry((rank, key))
            .or_insert_with(|| (kind, Vec::new()))
            .1
            .push(i);
    }

    let mut out: Vec<Capture> = by_key
        .into_values()
        .map(|(key, members)| build(photos, members, key))
        .collect();
    out.sort_by(|a, b| {
        let ta = a.exposure_clock_s.unwrap_or(f64::INFINITY);
        let tb = b.exposure_clock_s.unwrap_or(f64::INFINITY);
        ta.total_cmp(&tb).then(a.sequence.cmp(&b.sequence))
    });
    out
}

fn build(photos: &[Photo], members: Vec<usize>, key: GroupKey) -> Capture {
    let mut jpegs = 0;
    let mut bands = Vec::new();
    let mut jpeg_time = None;
    let mut times = Vec::new();
    for &i in &members {
        let p = &photos[i];
        match &p.kind {
            ImageKind::Jpeg => {
                jpegs += 1;
                if jpeg_time.is_none() {
                    jpeg_time = p.exposure_clock_s;
                }
            }
            ImageKind::Band(b) => bands.push(b.clone()),
            ImageKind::Tiff => {}
        }
        if let Some(t) = p.exposure_clock_s {
            times.push(t);
        }
    }
    bands.sort();
    let exposure_clock_s = jpeg_time.or_else(|| times.iter().cloned().reduce(f64::min));
    let clock_spread_s = match (
        times.iter().cloned().reduce(f64::min),
        times.iter().cloned().reduce(f64::max),
    ) {
        (Some(a), Some(b)) => b - a,
        _ => 0.0,
    };
    let sequence = members.iter().filter_map(|&i| photos[i].sequence).min();
    Capture {
        members,
        key,
        exposure_clock_s,
        sequence,
        jpegs,
        bands,
        clock_spread_s,
    }
}
