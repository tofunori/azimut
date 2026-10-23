//! Reader for NRCan CSRS-PPP summary files (`*.sum`).
//!
//! Each line starts with a three-letter keyword. The position block has one line per
//! coordinate and reference system: `POS LAT <SYST> <EPOCH> <a priori> <estimated> <diff>
//! <sigma 95 %> <correlations>`, latitude and longitude in degrees, minutes and seconds.
//! `ANT` names the antenna CSRS-PPP found in its calibration file (`NOT FOUND` otherwise) and
//! `ARP` gives the antenna offsets it applied, which come from the RINEX `ANTENNA: DELTA
//! H/E/N` record. Lines are read by keyword, so records added in later versions are skipped.
//! Layout checked on versions 3.54.2 (2022) and 4.14.11 (2024). See SOURCES.md (CSRS-PPP).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Estimated position of the station in one reference system.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PppPosition {
    /// System label as written, e.g. `IGS20`, `IGb14`.
    pub system: String,
    /// Epoch as written, e.g. `24:153:64110` (year, day of year, second of day).
    pub epoch: String,
    /// The epoch as a decimal year, when it has the `yy:ddd:sssss` form.
    pub epoch_year: Option<f64>,
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Ellipsoidal height (GRS80), metres.
    pub h_ell_m: f64,
    /// 95 % uncertainties of latitude, longitude and height, metres.
    pub sigma95_m: [f64; 3],
}

/// What the coordinates refer to, from the `ANT` and `ARP` records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PppPoint {
    /// Antenna not found and no offsets: the antenna phase centre, without calibration.
    PhaseCentre,
    /// Antenna calibrated, no offsets: the antenna reference point.
    ReferencePoint,
    /// Offsets applied: the mark under the antenna.
    Mark,
}

/// The parts of a CSRS-PPP summary that Azimut uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PppSummary {
    /// `VER`: CSRS-PPP version.
    pub version: Option<String>,
    /// `RNX`: name of the RINEX file submitted.
    pub rinex: Option<String>,
    /// `MKR`: marker name.
    pub marker: Option<String>,
    /// `MOD`: processing mode (`STATIC`, `KINEMATIC`).
    pub mode: Option<String>,
    /// `ANT`: antenna type with radome, `None` when CSRS-PPP did not find it.
    pub antenna: Option<String>,
    /// `ARP`: antenna offsets applied, metres, in the order written.
    pub arp_m: Option<[f64; 3]>,
    /// `IAR`: percentage of ambiguities fixed.
    pub fixed_ambiguities_pct: Option<f64>,
    /// `OHT`: orthometric height system, geoid model and height (m).
    pub orthometric: Option<(String, String, f64)>,
    /// `GHT`: geoid height, metres.
    pub geoid_m: Option<f64>,
    /// One entry per reference system found in the position block.
    pub positions: Vec<PppPosition>,
}

impl PppSummary {
    /// Which point the coordinates refer to.
    pub fn point(&self) -> PppPoint {
        let offsets = self.arp_m.is_some_and(|a| a.iter().any(|v| *v != 0.0));
        match (offsets, &self.antenna) {
            (true, _) => PppPoint::Mark,
            (false, Some(_)) => PppPoint::ReferencePoint,
            (false, None) => PppPoint::PhaseCentre,
        }
    }

    /// The position in `system`, or the only one when `system` is `None`.
    pub fn position(&self, system: Option<&str>) -> Result<&PppPosition, String> {
        let systems = || {
            self.positions
                .iter()
                .map(|p| p.system.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match system {
            Some(s) => self
                .positions
                .iter()
                .find(|p| p.system.eq_ignore_ascii_case(s))
                .ok_or_else(|| format!("no {s} position in the summary (found: {})", systems())),
            None => match self.positions.as_slice() {
                [one] => Ok(one),
                [] => Err("no position (POS LAT/LON/HGT) in the summary".into()),
                _ => Err(format!(
                    "several reference systems in the summary ({}); choose one",
                    systems()
                )),
            },
        }
    }
}

/// Degrees, minutes, seconds; the sign of the degrees applies to the whole angle.
fn dms(d: &str, m: &str, s: &str) -> Option<f64> {
    let deg: f64 = d.parse().ok()?;
    let min: f64 = m.parse().ok()?;
    let sec: f64 = s.parse().ok()?;
    let v = deg.abs() + min / 60.0 + sec / 3600.0;
    Some(if d.trim_start().starts_with('-') { -v } else { v })
}

/// `yy:ddd:sssss` to a decimal year.
fn epoch_year(epoch: &str) -> Option<f64> {
    let mut it = epoch.split(':');
    let yy: i32 = it.next()?.parse().ok()?;
    let doy: f64 = it.next()?.parse().ok()?;
    let sod: f64 = it.next()?.parse().ok()?;
    let year = if yy < 80 { 2000 + yy } else { 1900 + yy };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = if leap { 366.0 } else { 365.0 };
    Some(year as f64 + (doy - 1.0 + sod / 86_400.0) / days)
}

/// Parse the text of a summary file.
pub fn parse_str(text: &str) -> Result<PppSummary, String> {
    let mut sum = PppSummary {
        version: None,
        rinex: None,
        marker: None,
        mode: None,
        antenna: None,
        arp_m: None,
        fixed_ambiguities_pct: None,
        orthometric: None,
        geoid_m: None,
        positions: Vec::new(),
    };
    // (system, epoch) -> lat, lon, h with their sigmas.
    type Parts = (Option<(f64, f64)>, Option<(f64, f64)>, Option<(f64, f64)>);
    let mut blocks: Vec<((String, String), Parts)> = Vec::new();

    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end();
        let Some((key, rest)) = line.split_once(' ') else {
            continue;
        };
        let rest = rest.trim();
        let t: Vec<&str> = rest.split_whitespace().collect();
        let bad = |what: &str| format!("line {}: unreadable {what}", n + 1);
        match key {
            "VER" => sum.version = Some(rest.to_string()),
            "RNX" => sum.rinex = Some(rest.to_string()),
            "MKR" => sum.marker = Some(rest.to_string()),
            "MOD" => sum.mode = Some(rest.to_string()),
            "ANT" => {
                sum.antenna =
                    (!rest.eq_ignore_ascii_case("NOT FOUND") && !rest.is_empty()).then(|| rest.to_string())
            }
            "ARP" => {
                let v: Vec<f64> = t.iter().filter_map(|x| x.parse().ok()).collect();
                sum.arp_m = Some(match v.as_slice() {
                    [a, b, c] => [*a, *b, *c],
                    _ => return Err(bad("ARP offsets")),
                });
            }
            // Version 4: `IAR 98.71%`. Version 5.15: one line per constellation, then `IAR ALL 89.30%`.
            "IAR" => match t.as_slice() {
                [pct] | ["ALL", pct] => sum.fixed_ambiguities_pct = pct.trim_end_matches('%').parse().ok(),
                _ => {}
            },
            "GHT" => sum.geoid_m = t.first().and_then(|v| v.parse().ok()),
            "OHT" if t.len() >= 3 && t[0] != "SYST" => {
                let h = t[2].parse().map_err(|_| bad("orthometric height"))?;
                sum.orthometric = Some((t[0].to_string(), t[1].to_string(), h));
            }
            "POS" if t.len() >= 3 && matches!(t[0], "LAT" | "LON" | "HGT") => {
                let key = (t[1].to_string(), t[2].to_string());
                let i = match blocks.iter().position(|(k, _)| *k == key) {
                    Some(i) => i,
                    None => {
                        blocks.push((key, (None, None, None)));
                        blocks.len() - 1
                    }
                };
                let parts = &mut blocks[i].1;
                match t[0] {
                    // A priori d m s, estimated d m s, diff, sigma.
                    "LAT" | "LON" => {
                        if t.len() < 12 {
                            return Err(bad("latitude or longitude"));
                        }
                        let v = dms(t[6], t[7], t[8]).ok_or_else(|| bad("angle"))?;
                        let s: f64 = t[10].parse().map_err(|_| bad("sigma"))?;
                        if t[0] == "LAT" {
                            parts.0 = Some((v, s));
                        } else {
                            parts.1 = Some((v, s));
                        }
                    }
                    // A priori, estimated, diff, sigma.
                    _ => {
                        if t.len() < 7 {
                            return Err(bad("height"));
                        }
                        let v: f64 = t[4].parse().map_err(|_| bad("height"))?;
                        let s: f64 = t[6].parse().map_err(|_| bad("sigma"))?;
                        parts.2 = Some((v, s));
                    }
                }
            }
            _ => {}
        }
    }

    for ((system, epoch), parts) in blocks {
        let (Some(lat), Some(lon), Some(h)) = parts else {
            return Err(format!("incomplete {system} position (needs LAT, LON and HGT)"));
        };
        sum.positions.push(PppPosition {
            epoch_year: epoch_year(&epoch),
            system,
            epoch,
            lat_deg: lat.0,
            lon_deg: lon.0,
            h_ell_m: h.0,
            sigma95_m: [lat.1, lon.1, h.1],
        });
    }
    if sum.positions.is_empty() {
        return Err("no position (POS LAT/LON/HGT): not a CSRS-PPP summary file?".into());
    }
    Ok(sum)
}

/// Read a summary file.
pub fn read(path: &Path) -> Result<PppSummary, Error> {
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    parse_str(&String::from_utf8_lossy(&bytes)).map_err(|reason| Error::Parse {
        path: path.to_path_buf(),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same layout as real version 4 summaries; values are made up.
    const SAMPLE: &str = "\
HDR GRP LEVÉS GÉODÉSIQUES DU CANADA
VER 4.14.11 (2024-11-29)
RNX BASE1530.24o
MKR BASE
MOD STATIC
REC Unknown
ANT NOT FOUND
ARP    0.000    0.000    0.000
IAR 98.71%
APR PPP
ELL GRS80
POS CRD  SYST        EPOCH          A_PRIORI         ESTIMATED       DIFF SIGMA(95%) CORRELATIONS
POS   X IGS20 24:153:64110      1196000.0000      1196000.5000     0.5000     0.0043  1.0000
POS LAT IGS20 24:153:64110    46 20 56.90000    46 20 56.84444    -1.7150     0.0052  1.0000
POS LON IGS20 24:153:64110   -72 33  4.40000   -72 33  4.44444    -0.9500     0.0043  0.0109  1.0000
POS HGT IGS20 24:153:64110           33.7062           31.2040    -2.5022     0.0195  0.2589 -0.0595  1.0000
OHT     SYST    MODEL             HEIGHT
OHT CGVD2013 CGG2013a            60.9121
GHT -29.7081
";

    #[test]
    fn reads_position_antenna_and_heights() {
        let s = parse_str(SAMPLE).unwrap();
        assert_eq!(s.version.as_deref(), Some("4.14.11 (2024-11-29)"));
        assert_eq!(s.rinex.as_deref(), Some("BASE1530.24o"));
        assert_eq!(s.antenna, None);
        assert_eq!(s.arp_m, Some([0.0; 3]));
        assert_eq!(s.point(), PppPoint::PhaseCentre);
        assert_eq!(s.fixed_ambiguities_pct, Some(98.71));
        assert_eq!(s.geoid_m, Some(-29.7081));
        assert_eq!(
            s.orthometric,
            Some(("CGVD2013".into(), "CGG2013a".into(), 60.9121))
        );
        let p = s.position(None).unwrap();
        assert_eq!(p.system, "IGS20");
        assert!((p.lat_deg - (46.0 + 20.0 / 60.0 + 56.84444 / 3600.0)).abs() < 1e-12);
        assert!((p.lon_deg + (72.0 + 33.0 / 60.0 + 4.44444 / 3600.0)).abs() < 1e-12);
        assert_eq!(p.h_ell_m, 31.204);
        assert_eq!(p.sigma95_m, [0.0052, 0.0043, 0.0195]);
        // 2024 is a leap year: day 153 at 64110 s.
        assert!((p.epoch_year.unwrap() - (2024.0 + (152.0 + 64110.0 / 86400.0) / 366.0)).abs() < 1e-12);
    }

    #[test]
    fn reads_per_constellation_ambiguity_lines() {
        // Version 5.15 writes one IAR line per constellation, then the total.
        let v5 = SAMPLE.replace(
            "IAR 98.71%\n",
            "IAR GAL OFF\nIAR GPS 89.30%\nIAR GLO OFF\nIAR ALL 89.30%\n",
        );
        assert_eq!(parse_str(&v5).unwrap().fixed_ambiguities_pct, Some(89.3));
    }

    #[test]
    fn point_follows_antenna_and_offsets() {
        let found = SAMPLE.replace("ANT NOT FOUND", "ANT TRM59800.00     NONE");
        let s = parse_str(&found).unwrap();
        assert_eq!(s.antenna.as_deref(), Some("TRM59800.00     NONE"));
        assert_eq!(s.point(), PppPoint::ReferencePoint);
        let mark = found.replace("ARP    0.000", "ARP    1.800");
        assert_eq!(parse_str(&mark).unwrap().point(), PppPoint::Mark);
    }

    #[test]
    fn several_systems_must_be_chosen() {
        let both = format!(
            "{SAMPLE}POS LAT NAD83 10:001:00000    46 20 56.9 46 20 56.8 0 0.0052 1\n\
             POS LON NAD83 10:001:00000   -72 33 4.4 -72 33 4.4 0 0.0043 0 1\n\
             POS HGT NAD83 10:001:00000   33.0 32.0 0 0.0195 0 0 1\n"
        );
        let s = parse_str(&both).unwrap();
        assert!(s.position(None).unwrap_err().contains("IGS20, NAD83"));
        assert_eq!(s.position(Some("nad83")).unwrap().h_ell_m, 32.0);
        assert!(s.position(Some("ITRF2020")).is_err());
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse_str("not a summary\n").is_err());
        let partial: String = SAMPLE
            .lines()
            .filter(|l| !l.starts_with("POS HGT"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_str(&partial).unwrap_err().contains("incomplete"));
    }
}
