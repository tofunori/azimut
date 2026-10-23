//! Small geoid grids in the PROJ GeoTIFF layout, for tests. Not real geoid models.

use std::fs::File;
use std::io;
use std::path::Path;

use tiff::encoder::{colortype::GrayI32, TiffEncoder};
use tiff::tags::Tag;

/// Description of a synthetic geoid grid.
#[derive(Debug, Clone)]
pub struct SyntheticGrid {
    /// GDAL metadata `TYPE`.
    pub kind: String,
    /// Raster type: node at the tie point (true) or cell corner at the tie point (false).
    pub pixel_is_point: bool,
    /// `ImageDescription`: "<source frame> to <vertical datum>. ..."
    pub description: String,
    /// Raw value written at node (col 3, row 3) and declared as nodata.
    pub nodata: Option<i32>,
}

impl Default for SyntheticGrid {
    fn default() -> Self {
        Self {
            kind: "VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL".into(),
            pixel_is_point: true,
            description: "NAD83(CSRS)v6 (EPSG:8251) to CGVD2013(CGG2013a) height (EPSG:9245). \
                          Synthetic test grid"
                .into(),
            nodata: None,
        }
    }
}

impl SyntheticGrid {
    /// Geoid undulation the grid holds at a point (bilinear in a linear field), metres.
    pub fn expected(lat_deg: f64, lon_deg: f64) -> f64 {
        let col = (lon_deg + 73.0) / 0.5;
        let row = (47.0 - lat_deg) / 0.5;
        -30.0 + 0.1 * col + 0.01 * row
    }

    /// Write 4 x 5 nodes every 0.5 degree from 47 N, 73 W (to 45 N, 71.5 W), in millimetres
    /// with a 0.001 scale: N = -30 + 0.1 column + 0.01 row metres. Strips of two rows.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let (w, h) = (4u32, 5u32);
        let mut values: Vec<i32> = (0..h)
            .flat_map(|r| (0..w).map(move |c| -30000 + 100 * c as i32 + 10 * r as i32))
            .collect();
        if let Some(nd) = self.nodata {
            values[(3 * w + 3) as usize] = nd;
        }
        let e = |e: tiff::TiffError| io::Error::other(e.to_string());
        let mut enc = TiffEncoder::new(File::create(path)?).map_err(e)?;
        let mut img = enc.new_image::<GrayI32>(w, h).map_err(e)?;
        img.rows_per_strip(2).map_err(e)?;
        let d = img.encoder();
        d.write_tag(Tag::ImageDescription, self.description.as_str())
            .map_err(e)?;
        d.write_tag(Tag::ModelPixelScaleTag, &[0.5f64, 0.5, 0.0][..])
            .map_err(e)?;
        let (lon, lat) = if self.pixel_is_point {
            (-73.0, 47.0)
        } else {
            (-73.25, 47.25)
        };
        d.write_tag(Tag::ModelTiepointTag, &[0.0f64, 0.0, 0.0, lon, lat, 0.0][..])
            .map_err(e)?;
        let raster = if self.pixel_is_point { 2 } else { 1 };
        d.write_tag(
            Tag::GeoKeyDirectoryTag,
            &[1u16, 1, 1, 3, 1024, 0, 1, 2, 1025, 0, 1, raster, 2048, 0, 1, 8252][..],
        )
        .map_err(e)?;
        let meta = format!(
            "<GDALMetadata><Item name=\"TYPE\">{}</Item>\
             <Item name=\"SCALE\" sample=\"0\" role=\"scale\">0.001</Item>\
             <Item name=\"UNITTYPE\" sample=\"0\" role=\"unittype\">metre</Item>\
             <Item name=\"DESCRIPTION\" sample=\"0\" role=\"description\">geoid_undulation</Item>\
             </GDALMetadata>",
            self.kind
        );
        d.write_tag(Tag::Unknown(42112), meta.as_str()).map_err(e)?;
        if let Some(nd) = self.nodata {
            d.write_tag(Tag::GdalNodata, nd.to_string().as_str()).map_err(e)?;
        }
        img.write_data(&values).map_err(e)
    }
}
