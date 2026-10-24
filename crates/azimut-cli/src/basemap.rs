//! Satellite background of the report map, fetched over the network on request.
//!
//! `--basemap sentinel2` takes EOxCloudless 2016 by EOX: a cloud-free Sentinel-2 mosaic at
//! 10 m, the one year EOX releases under CC BY 4.0, so its tiles may be stored in a report
//! and passed on with the credit. The later years are non-commercial only. Google Maps and
//! Esri World Imagery tiles are not offered: their terms forbid storing tiles outside their
//! own applications, which a report that opens offline does. Any other XYZ tile server
//! can be given as a URL with its credit; its terms are then the user's to check.
//!
//! A background that cannot be fetched (no network, server error) is left out with a
//! message: the report is written all the same.

use std::io::Read as _;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use azimut_export::basemap::{tile_range, Basemap, Tile};
use azimut_export::report::FlightReport;

/// EOxCloudless 2016, Web Mercator, XYZ order.
const SENTINEL2_URL: &str = "https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless_3857/default/g/{z}/{y}/{x}.jpg";
/// Credit required by EOX for the 2016 layer.
const SENTINEL2_CREDIT: &str = "EOxCloudless https://cloudless.eox.at by EOX IT Services GmbH \
     (Contains modified Copernicus Sentinel data 2016), CC BY 4.0";
/// 10 m imagery: zoom 16 pixels are already 1.7 m at 45 degrees.
const SENTINEL2_MAX_ZOOM: u8 = 16;
/// Any server: zoom 19 is the usual last level of XYZ imagery.
const CUSTOM_MAX_ZOOM: u8 = 19;
/// About 1 MB of tiles in the page at most.
const MAX_TILES: usize = 48;
const MAX_TILE_BYTES: u64 = 2 << 20;
const USER_AGENT: &str = concat!(
    "azimut/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/tofunori/azimut)"
);

#[derive(clap::Args, Clone, Debug, Default)]
pub struct BasemapArgs {
    /// Satellite background for the report map, fetched over the network: `none`,
    /// `sentinel2` (EOxCloudless 2016 by EOX, 10 m, CC BY 4.0), or an XYZ tile URL with
    /// {z}, {x} and {y} (then give --basemap-attribution). Without network the report is
    /// written without it.
    #[arg(long, env = "AZIMUT_BASEMAP", default_value = "none", value_name = "SOURCE")]
    pub basemap: String,
    /// Credit shown under the map for a tile URL, as its licence requires.
    #[arg(long, env = "AZIMUT_BASEMAP_ATTRIBUTION", value_name = "TEXT")]
    pub basemap_attribution: Option<String>,
}

/// Where the tiles come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    None,
    Tiles {
        template: String,
        attribution: String,
        max_zoom: u8,
    },
}

impl BasemapArgs {
    /// Check the options before any work, so a mistake stops the run at once.
    pub fn source(&self) -> Result<Source> {
        let v = self.basemap.trim();
        match v.to_ascii_lowercase().as_str() {
            "" | "none" => return Ok(Source::None),
            "sentinel2" | "sentinel-2" | "s2cloudless" => {
                return Ok(Source::Tiles {
                    template: SENTINEL2_URL.into(),
                    attribution: SENTINEL2_CREDIT.into(),
                    max_zoom: SENTINEL2_MAX_ZOOM,
                })
            }
            _ => {}
        }
        if !(v.starts_with("http://") || v.starts_with("https://")) {
            bail!("--basemap: expected none, sentinel2 or an http(s) tile URL, got {v:?}");
        }
        for k in ["{z}", "{x}", "{y}"] {
            if !v.contains(k) {
                bail!("--basemap: the tile URL has no {k}");
            }
        }
        let attribution = self
            .basemap_attribution
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .context(
                "--basemap with a tile URL needs --basemap-attribution (the credit its licence requires)",
            )?;
        Ok(Source::Tiles {
            template: v.into(),
            attribution: attribution.into(),
            max_zoom: CUSTOM_MAX_ZOOM,
        })
    }
}

/// The background of `rep`'s map, or `None` (with a message) when there is none to fetch or
/// it could not be fetched.
pub fn fetch(source: &Source, rep: &FlightReport) -> Option<Basemap> {
    let Source::Tiles {
        template,
        attribution,
        max_zoom,
    } = source
    else {
        return None;
    };
    let bounds = rep.map_bounds()?;
    let range = tile_range(&bounds, *max_zoom, MAX_TILES);
    match fetch_tiles(template, range.zoom, range.tiles()) {
        Ok(tiles) => {
            println!("   basemap: {} tile(s) at zoom {}", tiles.len(), range.zoom);
            Some(Basemap {
                zoom: range.zoom,
                tiles,
                attribution: attribution.clone(),
            })
        }
        Err(e) => {
            println!("   basemap left out: {e:#}");
            None
        }
    }
}

fn fetch_tiles(template: &str, zoom: u8, cells: impl Iterator<Item = (u32, u32)>) -> Result<Vec<Tile>> {
    let mut builder = ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20));
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    if let Some(p) = azimut_csrs::proxy_for(template, env) {
        builder = builder.proxy(ureq::Proxy::new(&p).with_context(|| format!("proxy {p}"))?);
    }
    let agent = builder.build();
    let mut tiles = Vec::new();
    // One missing tile would leave a hole: all or nothing.
    for (x, y) in cells {
        let url = template
            .replace("{z}", &zoom.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string());
        let resp = agent.get(&url).call().with_context(|| format!("GET {url}"))?;
        let mime = resp.content_type().to_ascii_lowercase();
        if !matches!(mime.as_str(), "image/jpeg" | "image/png" | "image/webp") {
            bail!("{url} answered {mime}, not an image");
        }
        let mut data = Vec::new();
        resp.into_reader()
            .take(MAX_TILE_BYTES)
            .read_to_end(&mut data)
            .with_context(|| format!("reading {url}"))?;
        tiles.push(Tile { x, y, mime, data });
    }
    Ok(tiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(basemap: &str, credit: Option<&str>) -> BasemapArgs {
        BasemapArgs {
            basemap: basemap.into(),
            basemap_attribution: credit.map(String::from),
        }
    }

    #[test]
    fn sources_are_checked_up_front() {
        assert_eq!(args("none", None).source().unwrap(), Source::None);
        let Source::Tiles { attribution, .. } = args("sentinel2", None).source().unwrap() else {
            panic!()
        };
        assert!(attribution.contains("EOX") && attribution.contains("2016"));
        let url = "https://tiles.example.org/{z}/{x}/{y}.png";
        assert!(args(url, None).source().is_err());
        assert!(args(url, Some("  ")).source().is_err());
        assert!(args(url, Some("Example")).source().is_ok());
        assert!(args("https://tiles.example.org/{z}/{x}.png", Some("E"))
            .source()
            .is_err());
        assert!(args("esri", None).source().is_err());
    }
}
