//! Geoid grids in the PROJ GeoTIFF format, and orthometric heights.
//!
//! A grid of type `VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL` holds the geoid undulation N of a
//! vertical datum above the ellipsoid of one geographic frame, so the orthometric height is
//! H = h - N, with h the ellipsoidal height in that same frame. N is interpolated bilinearly
//! between the four surrounding nodes, PROJ's default; a point with a missing node gets no
//! height. Tiles are decoded on demand, so only the area of a flight is read.
//!
//! A grid applies to one frame: NRCan publishes CGG2013a for NAD83(CSRS) and for ITRF2008,
//! and ellipsoidal heights in the two frames differ. [`GeoidGrid::check_frame`] refuses
//! positions in a frame of another family. See SOURCES.md (Geoid grids).

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

/// GDAL metadata TIFF tag (XML); nodata is `Tag::GdalNodata`.
const TAG_GDAL_METADATA: u16 = 42112;
/// GeoKeys: model type (2 = geographic) and raster type (1 = pixel is area, 2 = point).
const KEY_MODEL_TYPE: u16 = 1024;
const KEY_RASTER_TYPE: u16 = 1025;
const GRID_TYPE: &str = "VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL";

#[derive(Debug, thiserror::Error)]
pub enum GeoidError {
    #[error("{path}: {reason}")]
    Grid { path: PathBuf, reason: String },
}

/// An opened geoid grid.
pub struct GeoidGrid {
    path: PathBuf,
    width: u32,
    height: u32,
    /// Longitude and latitude of the first (north-west) node, degrees.
    lon0: f64,
    lat0: f64,
    /// Node spacing in longitude and latitude, degrees; latitude decreases with the row.
    dlon: f64,
    dlat: f64,
    scale: f64,
    offset: f64,
    /// Raw value marking a missing node.
    nodata: Option<f64>,
    chunk: (u32, u32),
    chunks_across: u32,
    source_frame: String,
    vertical_datum: String,
    decoder: Mutex<Decoder<BufReader<File>>>,
    chunks: Mutex<HashMap<u32, (u32, Vec<f64>)>>,
}

impl std::fmt::Debug for GeoidGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoidGrid")
            .field("path", &self.path)
            .field("source_frame", &self.source_frame)
            .field("vertical_datum", &self.vertical_datum)
            .finish_non_exhaustive()
    }
}

/// Text of a GDAL metadata item, e.g. `<Item name="TYPE">...</Item>`.
fn metadata_item(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("name=\"{name}\""))?;
    let rest = &xml[start..];
    let open = rest.find('>')? + 1;
    let close = rest[open..].find("</Item>")?;
    Some(rest[open..open + close].trim().to_string())
}

fn to_f64(r: DecodingResult) -> Option<Vec<f64>> {
    Some(match r {
        DecodingResult::F32(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::F64(v) => v,
        DecodingResult::I32(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::I16(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::I8(v) => v.into_iter().map(f64::from).collect(),
        DecodingResult::U8(v) => v.into_iter().map(f64::from).collect(),
        _ => return None,
    })
}

/// The part of a frame name shared by all its realisations: `NAD83(CSRS)` for
/// `NAD83(CSRS)v6`, `ITRF` for `ITRF2008`.
fn frame_family(source: &str) -> String {
    let name = source.split(" (").next().unwrap_or(source).trim();
    match name.find(')') {
        Some(i) => name[..=i].to_string(),
        None => name.chars().take_while(|c| c.is_ascii_alphabetic()).collect(),
    }
}

fn squeeze(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_uppercase()
}

impl GeoidGrid {
    /// Open a grid and read its georeferencing. No grid values are read yet.
    pub fn open(path: &Path) -> Result<Self, GeoidError> {
        let err = |reason: String| GeoidError::Grid {
            path: path.to_path_buf(),
            reason,
        };
        let file = File::open(path).map_err(|e| err(e.to_string()))?;
        let mut d =
            Decoder::new(BufReader::new(file)).map_err(|e| err(format!("not a readable TIFF ({e})")))?;
        let (width, height) = d.dimensions().map_err(|e| err(e.to_string()))?;
        if width < 2 || height < 2 {
            return Err(err("grid smaller than 2 x 2 nodes".into()));
        }
        let samples = d
            .find_tag_unsigned::<u16>(Tag::SamplesPerPixel)
            .map_err(|e| err(e.to_string()))?
            .unwrap_or(1);
        if samples != 1 {
            return Err(err(format!("{samples} samples per node; a geoid grid has one")));
        }

        let meta = d
            .get_tag_ascii_string(Tag::Unknown(TAG_GDAL_METADATA))
            .map_err(|_| err("no GDAL metadata: grid type unknown".into()))?;
        let kind = metadata_item(&meta, "TYPE").unwrap_or_default();
        if kind != GRID_TYPE {
            return Err(err(format!(
                "grid type \"{kind}\", expected {GRID_TYPE} (a geoid model)"
            )));
        }
        if let Some(desc) = metadata_item(&meta, "DESCRIPTION") {
            if desc != "geoid_undulation" {
                return Err(err(format!("values are \"{desc}\", not geoid_undulation")));
            }
        }
        if let Some(unit) = metadata_item(&meta, "UNITTYPE") {
            if unit != "metre" {
                return Err(err(format!("values in \"{unit}\", not metre")));
            }
        }
        let number = |name: &str, default: f64| -> Result<f64, GeoidError> {
            match metadata_item(&meta, name) {
                Some(v) => v.parse::<f64>().map_err(|_| err(format!("bad {name} \"{v}\""))),
                None => Ok(default),
            }
        };
        let scale = number("SCALE", 1.0)?;
        let offset = number("OFFSET", 0.0)?;
        let nodata = match d.get_tag_ascii_string(Tag::GdalNodata) {
            Ok(s) => Some(
                s.trim()
                    .parse::<f64>()
                    .map_err(|_| err(format!("bad nodata value \"{}\"", s.trim())))?,
            ),
            Err(_) => None,
        };

        let keys = d
            .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
            .map_err(|_| err("no GeoTIFF keys".into()))?;
        let key = |id: u16| {
            keys.get(4..)
                .unwrap_or(&[])
                .chunks_exact(4)
                .find(|k| k[0] == id && k[1] == 0)
                .map(|k| k[3])
        };
        if key(KEY_MODEL_TYPE) != Some(2) {
            return Err(err("not in geographic coordinates".into()));
        }
        let half = match key(KEY_RASTER_TYPE) {
            Some(2) => 0.0,
            Some(1) => 0.5,
            _ => return Err(err("raster type (pixel is point or area) not given".into())),
        };
        let px = d
            .get_tag_f64_vec(Tag::ModelPixelScaleTag)
            .map_err(|_| err("no pixel scale".into()))?;
        let tie = d
            .get_tag_f64_vec(Tag::ModelTiepointTag)
            .map_err(|_| err("no tie point".into()))?;
        if px.len() < 2 || tie.len() < 6 || px[0] <= 0.0 || px[1] <= 0.0 {
            return Err(err("bad georeferencing".into()));
        }
        let (dlon, dlat) = (px[0], px[1]);
        let lon0 = tie[3] + (half - tie[0]) * dlon;
        let lat0 = tie[4] - (half - tie[1]) * dlat;

        let description = d
            .get_tag_ascii_string(Tag::ImageDescription)
            .map_err(|_| err("no description: the frame the grid applies to is unknown".into()))?;
        let Some((source, rest)) = description.split_once(" to ") else {
            return Err(err(format!(
                "description \"{description}\" does not name the frame the grid applies to"
            )));
        };
        let vertical_datum = rest.split(". ").next().unwrap_or(rest).trim();

        let chunk = d.chunk_dimensions();
        if chunk.0 == 0 || chunk.1 == 0 {
            return Err(err("bad tile or strip size".into()));
        }
        Ok(Self {
            path: path.to_path_buf(),
            width,
            height,
            lon0,
            lat0,
            dlon,
            dlat,
            scale,
            offset,
            nodata,
            chunk,
            chunks_across: width.div_ceil(chunk.0),
            source_frame: source.trim().to_string(),
            vertical_datum: vertical_datum.trim_end_matches('.').to_string(),
            decoder: Mutex::new(d),
            chunks: Mutex::new(HashMap::new()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Frame whose ellipsoidal heights the grid applies to, as the grid names it, e.g.
    /// `NAD83(CSRS)v6 (EPSG:8251)`.
    pub fn source_frame(&self) -> &str {
        &self.source_frame
    }

    /// Vertical datum of the heights, e.g. `CGVD2013(CGG2013a) height (EPSG:9245)`.
    pub fn vertical_datum(&self) -> &str {
        &self.vertical_datum
    }

    /// Check that positions in `frame` can use this grid. Refuses a frame of another family
    /// (NAD83(CSRS) positions with an ITRF grid, say); within a family, returns a note that
    /// the difference between realisations (NAD83(CSRS) v6 and v8, ITRF2008 and IGS20) is
    /// not applied. ITRF grids accept IGS frames, which are aligned to ITRF.
    pub fn check_frame(&self, frame: &str) -> Result<Option<String>, String> {
        let family = frame_family(&self.source_frame);
        let f = squeeze(frame);
        let fam = squeeze(&family);
        let same_family = !fam.is_empty() && (f.contains(&fam) || (fam == "ITRF" && f.contains("IGS")));
        if !same_family {
            return Err(format!(
                "the geoid grid {} applies to ellipsoidal heights in {}, but the positions are in \
                 \"{frame}\". Use the grid made for that frame (for CGVD2013: \
                 ca_nrc_CGG2013an83.tif for NAD83(CSRS), ca_nrc_CGG2013ai08.tif for ITRF), or \
                 give the frame with --frame.",
                self.path.display(),
                self.source_frame
            ));
        }
        let realisation = squeeze(self.source_frame.split(" (").next().unwrap_or(""));
        Ok((!f.contains(&realisation)).then(|| {
            format!(
                "the geoid grid applies to {}; the positions are in \"{frame}\". The difference \
                 between these two realisations of {family} is not applied.",
                self.source_frame
            )
        }))
    }

    /// Grid value of node (column `i`, row `j`), metres; `None` for a missing node.
    fn node(&self, i: u32, j: u32) -> Option<f64> {
        let (cw, ch) = self.chunk;
        let index = (j / ch) * self.chunks_across + i / cw;
        let mut chunks = self.chunks.lock().ok()?;
        if let Entry::Vacant(slot) = chunks.entry(index) {
            let mut d = self.decoder.lock().ok()?;
            let (data_w, _) = d.chunk_data_dimensions(index);
            let values = to_f64(d.read_chunk(index).ok()?)?;
            slot.insert((data_w, values));
        }
        let (data_w, values) = chunks.get(&index)?;
        let raw = *values.get(((j % ch) * data_w + i % cw) as usize)?;
        if self.nodata == Some(raw) || !raw.is_finite() {
            return None;
        }
        Some(self.offset + self.scale * raw)
    }

    /// Geoid undulation N at a point, metres, or `None` outside the grid or next to a missing
    /// node.
    pub fn undulation(&self, lat_deg: f64, lon_deg: f64) -> Option<f64> {
        let x = (lon_deg - self.lon0).rem_euclid(360.0) / self.dlon;
        let y = (self.lat0 - lat_deg) / self.dlat;
        let (w, h) = ((self.width - 1) as f64, (self.height - 1) as f64);
        if !(0.0..=w).contains(&x) || !(0.0..=h).contains(&y) {
            return None;
        }
        let i = (x.floor() as u32).min(self.width - 2);
        let j = (y.floor() as u32).min(self.height - 2);
        let (fx, fy) = (x - i as f64, y - j as f64);
        let n00 = self.node(i, j)?;
        let n10 = self.node(i + 1, j)?;
        let n01 = self.node(i, j + 1)?;
        let n11 = self.node(i + 1, j + 1)?;
        Some(n00 * (1.0 - fx) * (1.0 - fy) + n10 * fx * (1.0 - fy) + n01 * (1.0 - fx) * fy + n11 * fx * fy)
    }

    /// Orthometric height H = h - N, metres.
    pub fn orthometric_height(&self, lat_deg: f64, lon_deg: f64, h_ellipsoidal_m: f64) -> Option<f64> {
        self.undulation(lat_deg, lon_deg).map(|n| h_ellipsoidal_m - n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_families() {
        assert_eq!(frame_family("NAD83(CSRS)v6 (EPSG:8251)"), "NAD83(CSRS)");
        assert_eq!(frame_family("ITRF2008 (EPSG:7911)"), "ITRF");
        assert_eq!(frame_family("NAD83(2011)"), "NAD83(2011)");
    }

    #[test]
    fn metadata_items() {
        let xml = "<GDALMetadata>  <Item name=\"TYPE\">VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL</Item>  \
                   <Item name=\"SCALE\" sample=\"0\" role=\"scale\">0.001</Item></GDALMetadata>";
        assert_eq!(metadata_item(xml, "TYPE").as_deref(), Some(GRID_TYPE));
        assert_eq!(metadata_item(xml, "SCALE").as_deref(), Some("0.001"));
        assert_eq!(metadata_item(xml, "OFFSET"), None);
    }
}
