//! # azimut-io
//!
//! Fast, header-only readers for DJI drone survey folders, the first building block of
//! Azimut, an open-source PPK geotagging toolkit.
//!
//! - [`mrk`]: DJI `*.MRK` exposure logs, read by field suffix.
//! - [`headers`]: XMP from JPEG APP1 segments and TIFF tag 700, without reading pixels.
//! - [`xmp`]: the `drone-dji:` fields Azimut uses.
//! - [`photo`]: per-image metadata (band, `CaptureUUID`, exposure time, file number).
//! - [`rinex`]: RINEX header, first and last epoch, without reading the whole file.
//! - [`capture`]: grouping images of one trigger (Mavic 3M: 1 JPEG + 4 bands).
//! - [`matching`]: pairing MRK records with captures on time, never on file order.
//! - [`scan`]: folder inventory and diagnosis before any processing.
//! - [`csrs_ppp`]: NRCan CSRS-PPP summary files (precise base-station position).
//!
//! Every format rule is tied to a public source in `SOURCES.md` at the repository root.

pub mod capture;
pub mod csrs_ppp;
pub mod error;
pub mod headers;
pub mod matching;
pub mod mrk;
pub mod photo;
pub mod rinex;
pub mod scan;
pub mod time;
pub mod xmp;

#[cfg(feature = "synthetic")]
pub mod synthetic;

pub use error::Error;
pub use scan::{scan, Finding, Flight, ScanOptions, ScanReport, Severity};
