//! `azimut` command-line tool.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use azimut_export::{odm_geo_txt, positions_csv, problems_csv, RowFilter};
use azimut_io::matching::DEFAULT_TOLERANCE_S;
use azimut_io::rinex::is_nav_name;
use azimut_io::scan::AuxKind;
use azimut_io::synthetic::{write_flight, write_rinex, SyntheticFlight, SyntheticRinex, M3M_BAND_SIZE};
use azimut_io::{scan, ScanOptions, ScanReport, Severity};
use azimut_solve::{process_flight, BasePosition, BaseSetup, PpkConfig, ProcessOptions, Rtklib};

#[derive(Parser)]
#[command(name = "azimut", version, about = "Open-source PPK geotagging for DJI drones")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inventory a folder and diagnose it before any processing (headers only, fast).
    Scan {
        /// Folder to scan: an SD card copy, a mission folder or a whole campaign.
        dir: PathBuf,
        /// Write the full report as JSON to this file.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Pairing tolerance between MRK and image times, seconds.
        #[arg(long, default_value_t = DEFAULT_TOLERANCE_S)]
        tolerance: f64,
        /// Base-station RINEX file or folder outside DIR (repeatable). RINEX files found
        /// under DIR outside flight folders are also considered as bases.
        #[arg(long)]
        base: Vec<PathBuf>,
    },
    /// Scan, then run PPK (RTKLIB-EX) and write camera positions for every flight.
    Process {
        /// Folder to process: an SD card copy, a mission folder or a whole campaign.
        dir: PathBuf,
        /// Base-station RINEX file or folder (repeatable); see `scan --base`.
        #[arg(long)]
        base: Vec<PathBuf>,
        /// Known base mark coordinates: latitude,longitude,ellipsoidal height (degrees, m).
        /// Without it the approximate position in the base RINEX header is used.
        #[arg(long, value_name = "LAT,LON,H")]
        base_llh: Option<String>,
        /// Reference frame and epoch of --base-llh, written into every output row,
        /// e.g. "NAD83(CSRS) v8, epoch 2010.0" or "ITRF2020, epoch 2026.45".
        #[arg(long)]
        frame: Option<String>,
        /// Base antenna height above the mark (m). Default: from the RINEX header.
        #[arg(long)]
        antenna_height: Option<f64>,
        /// IGS base antenna type with radome (needs --antex to have an effect).
        #[arg(long)]
        antenna_type: Option<String>,
        /// ANTEX file with antenna phase-centre models (e.g. igs20.atx).
        #[arg(long)]
        antex: Option<PathBuf>,
        /// Folder holding RTKLIB-EX convbin and rnx2rtkp (else AZIMUT_RTKLIB_DIR, then PATH).
        #[arg(long)]
        rtklib: Option<PathBuf>,
        /// Leave BeiDou out (reported decoding problems on Matrice 4 logs).
        #[arg(long)]
        no_beidou: bool,
        /// Output folder.
        #[arg(long, default_value = "azimut_out")]
        out: PathBuf,
        /// Only write rows for these file roles: jpeg, Green, Red, RedEdge, NIR, tiff.
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
        /// Largest trajectory gap around an exposure, seconds.
        #[arg(long, default_value_t = 1.0)]
        max_gap: f64,
        /// Pairing tolerance between MRK and image times, seconds.
        #[arg(long, default_value_t = DEFAULT_TOLERANCE_S)]
        tolerance: f64,
    },
    /// Write a synthetic RINEX 3 base-station file covering the default demo flight.
    DemoBase {
        /// Output file, e.g. base/BASE_MO.rnx
        file: PathBuf,
    },
    /// Write a synthetic Mavic 3M flight (for trying the tool without real data).
    Demo {
        /// Output folder.
        dir: PathBuf,
        /// Number of captures.
        #[arg(long, default_value_t = 20)]
        captures: usize,
        /// Write full-size band images (about 10 MB each) to measure scan speed.
        #[arg(long)]
        full_size: bool,
        /// Write image times as UTC instead of GPS time.
        #[arg(long)]
        utc: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan {
            dir,
            json,
            tolerance,
            base,
        } => {
            anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
            let report = scan(
                &dir,
                &ScanOptions {
                    tolerance_s: tolerance,
                    base,
                    ..Default::default()
                },
            );
            print_report(&report);
            if let Some(path) = json {
                let text = serde_json::to_string_pretty(&report)?;
                std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
                println!("\nJSON report: {}", path.display());
            }
            let errors = report
                .flights
                .iter()
                .flat_map(|f| &f.findings)
                .any(|f| f.severity == Severity::Error);
            std::process::exit(if errors { 2 } else { 0 });
        }
        Command::Process {
            dir,
            base,
            base_llh,
            frame,
            antenna_height,
            antenna_type,
            antex,
            rtklib,
            no_beidou,
            out,
            roles,
            max_gap,
            tolerance,
        } => {
            anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
            let report = scan(
                &dir,
                &ScanOptions {
                    tolerance_s: tolerance,
                    base,
                    ..Default::default()
                },
            );
            print_report(&report);
            let rtk = Rtklib::locate(rtklib.as_deref())?;

            let position = match &base_llh {
                Some(t) => {
                    let v: Vec<f64> = t
                        .split(',')
                        .map(|x| x.trim().parse::<f64>())
                        .collect::<Result<_, _>>()
                        .context("--base-llh expects LAT,LON,H")?;
                    anyhow::ensure!(v.len() == 3, "--base-llh expects LAT,LON,H");
                    BasePosition::Llh {
                        lat_deg: v[0],
                        lon_deg: v[1],
                        h_ell_m: v[2],
                    }
                }
                None => BasePosition::RinexHeader,
            };
            let frame = frame.unwrap_or_else(|| match position {
                BasePosition::RinexHeader => "approximate (base RINEX header position)".into(),
                _ => "as given with --base-llh (frame not stated)".into(),
            });
            if position == BasePosition::RinexHeader {
                println!(
                    "\nNote: no --base-llh given. The base RINEX header position is often only metre-level:"
                );
                println!("      positions are precise relative to each other, not in absolute terms.");
            }
            let mut ppk = if no_beidou {
                PpkConfig::dji_without_beidou()
            } else {
                PpkConfig::dji()
            };
            ppk.antex = antex;
            let opts = ProcessOptions {
                ppk,
                base: BaseSetup {
                    position,
                    antenna_height_m: antenna_height,
                    antenna_type,
                    frame,
                },
                max_gap_s: max_gap,
                work_dir: out.join("work"),
            };
            let filter = if roles.is_empty() {
                RowFilter::All
            } else {
                RowFilter::Roles(roles)
            };
            std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;

            let mut failures = 0;
            for f in &report.flights {
                let name = f
                    .dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "flight".into());
                println!("\n== Processing {name}");
                if f.matching.pairs.is_empty() {
                    println!("   skipped: no exposure paired with the MRK");
                    failures += 1;
                    continue;
                }
                let Some(b) = &f.base else {
                    println!("   skipped: no base RINEX covers this flight (use --base)");
                    failures += 1;
                    continue;
                };
                let base_obs = &report.rinex[b.rinex].path;
                let base_navs = sibling_navs(base_obs);
                match process_flight(f, base_obs, &base_navs, &rtk, &opts) {
                    Ok(sol) => {
                        let csv = out.join(format!("{name}_positions.csv"));
                        std::fs::write(&csv, positions_csv(&sol, &filter))?;
                        std::fs::write(out.join(format!("{name}_geo.txt")), odm_geo_txt(&sol, &filter))?;
                        std::fs::write(
                            out.join(format!("{name}_solution.json")),
                            serde_json::to_string_pretty(&sol)?,
                        )?;
                        let st = &sol.stats;
                        if st.exposures_missing + st.exposures_without_sigma > 0 {
                            std::fs::write(out.join(format!("{name}_problems.csv")), problems_csv(&sol))?;
                        }
                        let med = |k: usize| {
                            median(
                                sol.exposures
                                    .iter()
                                    .filter_map(|e| e.camera.and_then(|c| c.sigma_enu_m).map(|s| s[k])),
                            )
                        };
                        println!(
                            "   {} exposures: {} fixed, {} float, {} other, {} without position",
                            st.exposures,
                            st.exposures_fixed,
                            st.exposures_float,
                            st.exposures_other,
                            st.exposures_missing
                        );
                        let sigmas = if st.exposures_without_sigma < st.exposures - st.exposures_missing {
                            format!("{:.3} / {:.3} / {:.3} m", med(0), med(1), med(2))
                        } else {
                            "none".into()
                        };
                        println!(
                            "   trajectory: {} epochs, {:.1} % fixed; median sigma E/N/U: {sigmas}",
                            st.epochs,
                            100.0 * st.epochs_fixed as f64 / st.epochs.max(1) as f64,
                        );
                        if st.exposures_without_sigma > 0 {
                            println!(
                                "   WARNING: {} exposure(s) without uncertainty: RTKLIB covariance not \
                                 positive definite (sigma left empty, see problems.csv)",
                                st.exposures_without_sigma
                            );
                        }
                        if st.max_height_check_m > 0.001 {
                            println!(
                                "   WARNING: lever-arm height check off by {:.4} m",
                                st.max_height_check_m
                            );
                        }
                        println!("   written: {}", csv.display());
                    }
                    Err(e) => {
                        println!("   failed: {e}");
                        failures += 1;
                    }
                }
            }
            std::process::exit(if failures > 0 { 2 } else { 0 });
        }
        Command::DemoBase { file } => {
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_rinex(&file, &SyntheticRinex::default())?;
            println!("Wrote synthetic base RINEX {} (not real data).", file.display());
            Ok(())
        }
        Command::Demo {
            dir,
            captures,
            full_size,
            utc,
        } => {
            let spec = SyntheticFlight {
                captures,
                clock_is_utc: utc,
                band_size: if full_size { M3M_BAND_SIZE } else { (1, 1) },
                drop_nir_at: if captures > 5 { vec![5] } else { vec![] },
                float_at: if captures > 3 { vec![2, 3] } else { vec![] },
                ..Default::default()
            };
            let files = write_flight(&dir, &spec)?;
            println!(
                "Wrote {} files to {} (synthetic data, not real DJI files).",
                files.len(),
                dir.display()
            );
            println!("Try: azimut scan {}", dir.display());
            Ok(())
        }
    }
}

/// Navigation files next to a base observation file.
fn sibling_navs(obs: &Path) -> Vec<PathBuf> {
    let Some(dir) = obs.parent() else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| is_nav_name(p) && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn median(it: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = it.collect();
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn print_report(r: &ScanReport) {
    println!(
        "Scanned {} in {:.2} s: {} flight folder(s), {} images, {} on disk, {} read.",
        r.root.display(),
        r.elapsed_s,
        r.flights.len(),
        r.images,
        human_bytes(r.bytes_on_disk),
        human_bytes(r.bytes_read)
    );
    if !r.rinex.is_empty() {
        println!(
            "{} RINEX observation file(s) checked (header, first and last epoch).",
            r.rinex.len()
        );
    }
    for f in &r.flights {
        let rel = f
            .dir
            .strip_prefix(&r.root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| f.dir.display().to_string());
        let rel = if rel.is_empty() { ".".to_string() } else { rel };
        let ms = f.captures.iter().filter(|c| c.is_multispectral()).count();
        println!("\n== {rel}");
        println!(
            "   {} images, {} captures ({} multispectral), {} MRK records, raw GNSS logs: {}, base logs: {}",
            f.photos.len(),
            f.captures.len(),
            ms,
            f.mrk.len(),
            f.aux_files(AuxKind::RawGnss).len(),
            f.aux_files(AuxKind::BaseLog).len()
        );
        for x in &f.findings {
            let tag = match x.severity {
                Severity::Error => "ERROR",
                Severity::Warning => "WARN ",
                Severity::Info => "info ",
            };
            println!("   [{tag}] {}", x.message);
        }
    }
}
