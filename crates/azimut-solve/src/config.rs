//! RTKLIB-EX processing configuration, written as a `rnx2rtkp -k` options file.
//!
//! Option names and values follow RTKLIB-EX `src/options.c`. See SOURCES.md (RTKLIB).

use std::fmt::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where the base antenna is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum BasePosition {
    /// Approximate position in the base RINEX header. Often only metre-level: fine for
    /// relative work, not for absolute coordinates.
    RinexHeader,
    /// Known coordinates of the ground mark: latitude, longitude (degrees) and ellipsoidal
    /// height (m). Never an orthometric height.
    Llh {
        lat_deg: f64,
        lon_deg: f64,
        h_ell_m: f64,
    },
    /// Known ECEF coordinates of the ground mark, metres.
    Ecef { xyz_m: [f64; 3] },
}

/// Base station setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseSetup {
    pub position: BasePosition,
    /// Antenna reference point height above the mark, metres. `None`: take type and height
    /// from the RINEX header (`ant2-anttype=*`).
    pub antenna_height_m: Option<f64>,
    /// IGS antenna type with radome, e.g. `"TRM59800.00     NONE"`. Only used with an ANTEX file.
    pub antenna_type: Option<String>,
    /// Reference frame and epoch of the base coordinates, carried into every output.
    pub frame: String,
}

impl Default for BaseSetup {
    fn default() -> Self {
        Self {
            position: BasePosition::RinexHeader,
            antenna_height_m: None,
            antenna_type: None,
            frame: "unspecified (frame of the base coordinates)".into(),
        }
    }
}

/// GNSS systems to process.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Systems {
    pub gps: bool,
    pub glonass: bool,
    pub galileo: bool,
    pub beidou: bool,
    pub qzss: bool,
}

impl Systems {
    /// RTKLIB `pos1-navsys` bit mask (1 GPS, 4 GLONASS, 8 Galileo, 16 QZSS, 32 BeiDou).
    pub fn mask(self) -> u32 {
        (self.gps as u32)
            | (self.glonass as u32) << 2
            | (self.galileo as u32) << 3
            | (self.qzss as u32) << 4
            | (self.beidou as u32) << 5
    }
}

/// Processing options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PpkConfig {
    pub systems: Systems,
    /// 1: L1, 2: L1+L2, 3: L1+L2+L5.
    pub frequencies: u8,
    pub elevation_mask_deg: f64,
    /// Forward and backward filters combined (recommended for post-processing).
    pub combined: bool,
    /// Integer ambiguity validation ratio threshold.
    pub ar_ratio: f64,
    /// GLONASS ambiguity resolution. Off by default: base and drone receivers usually come
    /// from different makers, and GLONASS inter-channel biases then differ.
    pub glonass_ar: bool,
    pub beidou_ar: bool,
    /// Interpolate base observations to rover epochs (base 1 s, drone 5 Hz).
    pub base_time_interpolation: bool,
    /// ANTEX file for satellite and receiver antenna corrections.
    pub antex: Option<PathBuf>,
    /// Extra `key=value` options appended last (they override the ones above).
    pub extra: Vec<(String, String)>,
}

impl Default for PpkConfig {
    fn default() -> Self {
        Self::dji()
    }
}

impl PpkConfig {
    /// Defaults for DJI aircraft (dual-frequency RTCM 3 logs at 5 Hz).
    pub fn dji() -> Self {
        Self {
            systems: Systems {
                gps: true,
                glonass: true,
                galileo: true,
                beidou: true,
                qzss: false,
            },
            frequencies: 2,
            elevation_mask_deg: 15.0,
            combined: true,
            ar_ratio: 3.0,
            glonass_ar: false,
            beidou_ar: true,
            base_time_interpolation: true,
            antex: None,
            extra: Vec::new(),
        }
    }

    /// Matrice 4 series: BeiDou decoding problems have been reported; process without it.
    pub fn dji_without_beidou() -> Self {
        let mut c = Self::dji();
        c.systems.beidou = false;
        c
    }

    /// The options file text for `rnx2rtkp -k`.
    pub fn to_conf(&self, base: &BaseSetup) -> String {
        let mut s = String::new();
        let mut kv = |k: &str, v: &str| {
            let _ = writeln!(s, "{k:<18}={v}");
        };
        kv("pos1-posmode", "kinematic");
        kv(
            "pos1-frequency",
            match self.frequencies {
                0 | 1 => "l1",
                2 => "l1+l2",
                _ => "l1+l2+l5",
            },
        );
        kv("pos1-soltype", if self.combined { "combined" } else { "forward" });
        kv("pos1-elmask", &format!("{}", self.elevation_mask_deg));
        kv("pos1-ionoopt", "brdc");
        kv("pos1-tropopt", "saas");
        kv("pos1-sateph", "brdc");
        kv("pos1-navsys", &self.systems.mask().to_string());
        kv("pos2-armode", "continuous");
        kv("pos2-gloarmode", if self.glonass_ar { "on" } else { "off" });
        kv("pos2-bdsarmode", if self.beidou_ar { "on" } else { "off" });
        kv("pos2-arthres", &format!("{}", self.ar_ratio));
        kv("out-solformat", "xyz");
        kv("out-outhead", "on");
        kv("out-outopt", "on");
        kv("out-timesys", "gpst");
        kv("out-timeform", "tow");
        kv("out-timendec", "3");
        kv("out-height", "ellipsoidal");
        match &base.position {
            BasePosition::RinexHeader => kv("ant2-postype", "rinexhead"),
            BasePosition::Llh {
                lat_deg,
                lon_deg,
                h_ell_m,
            } => {
                kv("ant2-postype", "llh");
                kv("ant2-pos1", &format!("{lat_deg:.11}"));
                kv("ant2-pos2", &format!("{lon_deg:.11}"));
                kv("ant2-pos3", &format!("{h_ell_m:.4}"));
            }
            BasePosition::Ecef { xyz_m } => {
                kv("ant2-postype", "xyz");
                kv("ant2-pos1", &format!("{:.4}", xyz_m[0]));
                kv("ant2-pos2", &format!("{:.4}", xyz_m[1]));
                kv("ant2-pos3", &format!("{:.4}", xyz_m[2]));
            }
        }
        match base.antenna_height_m {
            None => kv("ant2-anttype", "*"),
            Some(h) => {
                kv("ant2-anttype", base.antenna_type.as_deref().unwrap_or(""));
                kv("ant2-antdele", "0");
                kv("ant2-antdeln", "0");
                kv("ant2-antdelu", &format!("{h:.4}"));
            }
        }
        kv(
            "misc-timeinterp",
            if self.base_time_interpolation { "on" } else { "off" },
        );
        if let Some(a) = &self.antex {
            kv("file-satantfile", &a.display().to_string());
            kv("file-rcvantfile", &a.display().to_string());
        }
        for (k, v) in &self.extra {
            kv(k, v);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navsys_mask() {
        assert_eq!(PpkConfig::dji().systems.mask(), 1 + 4 + 8 + 32);
        assert_eq!(PpkConfig::dji_without_beidou().systems.mask(), 1 + 4 + 8);
    }

    #[test]
    fn conf_lines() {
        let base = BaseSetup {
            position: BasePosition::Llh {
                lat_deg: 46.35,
                lon_deg: -72.55,
                h_ell_m: 30.123,
            },
            antenna_height_m: Some(1.8),
            antenna_type: Some("EMLIDREACHRS4   NONE".into()),
            frame: "NAD83(CSRS) v8 2010.0".into(),
        };
        let mut cfg = PpkConfig::dji();
        cfg.extra.push(("pos1-elmask".into(), "17".into()));
        let c = cfg.to_conf(&base);
        for line in [
            "pos1-posmode      =kinematic",
            "pos1-frequency    =l1+l2",
            "pos1-soltype      =combined",
            "pos1-navsys       =45",
            "pos2-gloarmode    =off",
            "out-solformat     =xyz",
            "out-timeform      =tow",
            "ant2-postype      =llh",
            "ant2-pos1         =46.35000000000",
            "ant2-antdelu      =1.8000",
            "misc-timeinterp   =on",
        ] {
            assert!(c.contains(line), "missing '{line}' in:\n{c}");
        }
        // Extra options come last, so they win.
        assert!(c.trim_end().ends_with("pos1-elmask       =17"));
    }

    #[test]
    fn header_antenna_by_default() {
        let c = PpkConfig::dji().to_conf(&BaseSetup::default());
        assert!(c.contains("ant2-postype      =rinexhead"));
        assert!(c.contains("ant2-anttype      =*"));
    }
}
