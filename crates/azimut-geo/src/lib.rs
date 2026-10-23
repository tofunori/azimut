//! # azimut-geo
//!
//! Height datums for Azimut.
//!
//! - [`geoid`]: geoid grids in the PROJ GeoTIFF format, such as NRCan's CGG2013a for
//!   CGVD2013 (`ca_nrc_CGG2013an83.tif` on cdn.proj.org), and orthometric heights.

pub mod geoid;
#[cfg(feature = "synthetic")]
pub mod synthetic;

pub use geoid::{GeoidError, GeoidGrid};
