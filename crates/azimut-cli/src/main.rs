//! `azimut` command-line tool.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};

use azimut_export::{odm_geo_txt, positions_csv, problems_csv, RowFilter};
use azimut_io::csrs_ppp::{self, PppPoint, PppSummary};
use azimut_io::matching::DEFAULT_TOLERANCE_S;
use azimut_io::rinex::is_nav_name;
use azimut_io::scan::AuxKind;
use azimut_io::synthetic::{write_flight, write_rinex, SyntheticFlight, SyntheticRinex, M3M_BAND_SIZE};
use azimut_io::{scan, ScanOptions, ScanReport, Severity};
use azimut_solve::{
    process_flight, shift_flight, BasePosition, BaseSetup, FlightSolution, Method, PpkConfig, ProcessOptions,
    Rtklib, ShiftSpec,
};

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
        /// IGS base antenna type with radome (needs --antex to have an effect). Requires
        /// --antenna-height: without both, type and height come from the RINEX header.
        #[arg(long, requires = "antenna_height")]
        antenna_type: Option<String>,
        /// CSRS-PPP summary (.sum) of the base RINEX: base position, antenna handling and
        /// frame are taken from it. The RINEX given with --base must be the one submitted.
        #[arg(
            long,
            value_name = "FILE.sum",
            conflicts_with_all = ["base_llh", "antenna_height", "antenna_type"]
        )]
        base_ppp: Option<PathBuf>,
        /// Reference system to take from the CSRS-PPP summary when it holds several.
        #[arg(long, value_name = "SYSTEM", requires = "base_ppp")]
        ppp_system: Option<String>,
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
    /// Move the real-time (RTK) camera positions of the MRK by a constant base correction,
    /// e.g. after a PPP solution of the RTK base, and write them for every flight. Images are
    /// not modified.
    #[command(group(ArgGroup::new("shift").required(true).args(["from", "enu"])))]
    #[command(group(ArgGroup::new("target").args(["to", "to_ppp"])))]
    Shift {
        /// Folder holding the flights: an SD card copy, a mission folder or a whole campaign.
        dir: PathBuf,
        /// Base coordinates the aircraft used, as the RTK base broadcast them:
        /// latitude,longitude,ellipsoidal height (degrees, m).
        #[arg(
            long,
            value_name = "LAT,LON,H",
            requires = "target",
            allow_hyphen_values = true
        )]
        from: Option<String>,
        /// Correct coordinates of the same physical point, same format.
        #[arg(long, value_name = "LAT,LON,H", requires = "from", allow_hyphen_values = true)]
        to: Option<String>,
        /// CSRS-PPP summary (.sum) of the base, instead of --to. --from must then be the point
        /// it refers to (printed at the start).
        #[arg(long, value_name = "FILE.sum", requires = "from")]
        to_ppp: Option<PathBuf>,
        /// Reference system to take from the CSRS-PPP summary when it holds several.
        #[arg(long, value_name = "SYSTEM", requires = "to_ppp")]
        ppp_system: Option<String>,
        /// Shift as East,North,Up in metres, instead of --from and --to.
        #[arg(long, value_name = "E,N,U", allow_hyphen_values = true)]
        enu: Option<String>,
        /// Reference frame and epoch of the corrected positions (those of --to), written
        /// into every output row, e.g. "NAD83(CSRS) v8, epoch 2010.0".
        #[arg(long)]
        frame: Option<String>,
        /// Output folder.
        #[arg(long, default_value = "azimut_out")]
        out: PathBuf,
        /// Only write rows for these file roles: jpeg, Green, Red, RedEdge, NIR, tiff.
        #[arg(long, value_delimiter = ',')]
        roles: Vec<String>,
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
            base_ppp,
            ppp_system,
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

            let ppp = match &base_ppp {
                Some(path) => Some(read_ppp(path)?),
                None => None,
            };
            let base_setup = match &ppp {
                Some(sum) => {
                    let (mut setup, point) = BaseSetup::from_ppp(sum, ppp_system.as_deref(), antex.is_some())
                        .map_err(anyhow::Error::msg)
                        .with_context(|| format!("--base-ppp {}", base_ppp.as_ref().unwrap().display()))?;
                    print_ppp(sum, ppp_system.as_deref(), &point)?;
                    if let Some(f) = frame {
                        setup.frame = f;
                    }
                    setup
                }
                None => {
                    let position = match &base_llh {
                        Some(t) => {
                            let [lat_deg, lon_deg, h_ell_m] = triplet(t, "--base-llh", "LAT,LON,H")?;
                            BasePosition::Llh {
                                lat_deg,
                                lon_deg,
                                h_ell_m,
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
                            "\nNote: no --base-llh or --base-ppp given. The base RINEX header position is \
                             often only metre-level:"
                        );
                        println!(
                            "      positions are precise relative to each other, not in absolute terms."
                        );
                    }
                    BaseSetup {
                        position,
                        antenna_height_m: antenna_height,
                        antenna_type,
                        frame,
                    }
                }
            };
            let mut ppk = if no_beidou {
                PpkConfig::dji_without_beidou()
            } else {
                PpkConfig::dji()
            };
            ppk.antex = antex;
            let opts = ProcessOptions {
                ppk,
                base: base_setup,
                max_gap_s: max_gap,
                work_dir: out.join("work"),
            };
            let filter = row_filter(roles);
            std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;

            let mut failures = 0;
            for f in &report.flights {
                let name = flight_name(&f.dir);
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
                if let Some(sum) = &ppp {
                    check_ppp_rinex(sum, &report.rinex[b.rinex]);
                }
                let base_navs = sibling_navs(base_obs);
                match process_flight(f, base_obs, &base_navs, &rtk, &opts) {
                    Ok(sol) => {
                        let csv = write_outputs(&out, &name, &sol, &filter)?;
                        print_solution(&sol);
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
        Command::Shift {
            dir,
            from,
            to,
            to_ppp,
            ppp_system,
            enu,
            frame,
            out,
            roles,
            tolerance,
        } => {
            anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
            let mut ppp_frame = None;
            let spec = match (&from, &to, &to_ppp, &enu) {
                (Some(f), Some(t), None, None) => ShiftSpec::Points {
                    from_llh: triplet(f, "--from", "LAT,LON,H")?,
                    to_llh: triplet(t, "--to", "LAT,LON,H")?,
                },
                (Some(f), None, Some(path), None) => {
                    let sum = read_ppp(path)?;
                    // Antenna checks only matter for PPK; here the point itself is what counts.
                    let (setup, point) = BaseSetup::from_ppp(&sum, ppp_system.as_deref(), true)
                        .map_err(anyhow::Error::msg)
                        .with_context(|| format!("--to-ppp {}", path.display()))?;
                    print_ppp(&sum, ppp_system.as_deref(), &point)?;
                    println!("--from must be the base coordinates of this same point.");
                    ppp_frame = Some(setup.frame);
                    let p = sum.position(ppp_system.as_deref()).map_err(anyhow::Error::msg)?;
                    ShiftSpec::Points {
                        from_llh: triplet(f, "--from", "LAT,LON,H")?,
                        to_llh: [p.lat_deg, p.lon_deg, p.h_ell_m],
                    }
                }
                (None, None, None, Some(e)) => ShiftSpec::Enu {
                    enu_m: triplet(e, "--enu", "E,N,U")?,
                },
                _ => anyhow::bail!("give --from with --to or --to-ppp, or --enu"),
            };
            let [e, n, u] = spec.enu_m();
            let horizontal = e.hypot(n);
            anyhow::ensure!(
                horizontal.hypot(u) < 500.0,
                "a shift of {:.0} m is not a base correction; check the coordinates",
                horizontal.hypot(u)
            );
            let frame = frame.or(ppp_frame).unwrap_or_else(|| match spec {
                ShiftSpec::Points { .. } => "as given with --to (frame not stated)".into(),
                ShiftSpec::Enu { .. } => "shifted RTK positions (frame not stated)".into(),
            });

            let report = scan(
                &dir,
                &ScanOptions {
                    tolerance_s: tolerance,
                    ..Default::default()
                },
            );
            print_report(&report);
            println!(
                "\nShift East / North / Up: {e:+.3} / {n:+.3} / {u:+.3} m (horizontal {horizontal:.3} m)"
            );
            if u.abs() > 5.0 {
                println!(
                    "WARNING: vertical shift of {u:.1} m. Are both heights ellipsoidal? An orthometric height \
                     (CGVD2013) differs from the ellipsoidal one by the geoid height, about -30 m in Quebec."
                );
            }
            if horizontal > 20.0 {
                println!(
                    "WARNING: horizontal shift of {horizontal:.1} m. Check that --from and --to are the same \
                     point and that latitude comes before longitude."
                );
            }

            let filter = row_filter(roles);
            std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
            let mut failures = 0;
            for f in &report.flights {
                let name = flight_name(&f.dir);
                println!("\n== Shifting {name}");
                if f.matching.pairs.is_empty() {
                    println!("   skipped: no exposure paired with the MRK");
                    failures += 1;
                    continue;
                }
                let sol = shift_flight(f, &spec, &frame);
                if sol.stats.exposures_fixed + sol.stats.exposures_float == 0 {
                    println!("   skipped: no RTK position in the MRK (the aircraft was not in RTK mode)");
                    failures += 1;
                    continue;
                }
                let csv = write_outputs(&out, &name, &sol, &filter)?;
                print_solution(&sol);
                println!("   written: {}", csv.display());
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

fn read_ppp(path: &Path) -> Result<PppSummary> {
    csrs_ppp::read(path).with_context(|| format!("reading CSRS-PPP summary {}", path.display()))
}

/// Print what a CSRS-PPP summary gives, and what its coordinates refer to.
fn print_ppp(sum: &PppSummary, system: Option<&str>, point: &str) -> Result<()> {
    let p = sum.position(system).map_err(anyhow::Error::msg)?;
    println!(
        "\nBase from CSRS-PPP {} ({}, RINEX {}):",
        sum.version.as_deref().unwrap_or("?"),
        sum.mode.as_deref().unwrap_or("?"),
        sum.rinex.as_deref().unwrap_or("?")
    );
    println!(
        "   {:.9}, {:.9}, h {:.4} m ellipsoidal  [{} epoch {}]",
        p.lat_deg, p.lon_deg, p.h_ell_m, p.system, p.epoch
    );
    println!(
        "   95 % uncertainty: lat {:.4} m, lon {:.4} m, h {:.4} m; ambiguities fixed: {}",
        p.sigma95_m[0],
        p.sigma95_m[1],
        p.sigma95_m[2],
        sum.fixed_ambiguities_pct
            .map(|v| format!("{v:.1} %"))
            .unwrap_or_else(|| "not reported".into())
    );
    if let Some((sys, model, h)) = &sum.orthometric {
        println!("   orthometric {sys} ({model}): {h:.4} m (outputs stay ellipsoidal)");
    }
    println!("   These coordinates are {point}.");
    if sum.point() == PppPoint::PhaseCentre {
        println!(
            "   WARNING: the base RINEX had no antenna type or height. Photos are positioned \
             correctly, but this base point is not the mark on the ground: add the antenna type and \
             height to the RINEX header before submitting to CSRS-PPP to tie the survey to the mark."
        );
    }
    Ok(())
}

/// Warn when the base RINEX used for a flight is not the one CSRS-PPP processed.
fn check_ppp_rinex(sum: &PppSummary, base: &azimut_io::rinex::RinexSummary) {
    let name = base
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(r) = &sum.rinex {
        if !r.eq_ignore_ascii_case(&name) {
            println!(
                "   WARNING: base RINEX is {name}, CSRS-PPP processed {r}. Antenna height and type \
                 must be the same in both."
            );
        }
    }
    if sum.point() == PppPoint::Mark {
        if let (Some(arp), Some(hen)) = (sum.arp_m, base.antenna_delta_hen_m) {
            if (0..3).any(|k| (arp[k] - hen[k]).abs() > 0.0005) {
                println!("   WARNING: antenna offsets differ: RINEX header {hen:?} m, CSRS-PPP {arp:?} m.");
            }
        }
    }
}

/// Parse `A,B,C` into three numbers.
fn triplet(text: &str, option: &str, layout: &str) -> Result<[f64; 3]> {
    let v: Vec<f64> = text
        .split(',')
        .map(|x| x.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("{option} expects {layout}"))?;
    anyhow::ensure!(v.len() == 3, "{option} expects {layout}");
    Ok([v[0], v[1], v[2]])
}

fn flight_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "flight".into())
}

fn row_filter(roles: Vec<String>) -> RowFilter {
    if roles.is_empty() {
        RowFilter::All
    } else {
        RowFilter::Roles(roles)
    }
}

/// Write the CSV, geo.txt, JSON and (when needed) problems files of one flight. Returns the
/// CSV path.
fn write_outputs(out: &Path, name: &str, sol: &FlightSolution, filter: &RowFilter) -> Result<PathBuf> {
    let csv = out.join(format!("{name}_positions.csv"));
    std::fs::write(&csv, positions_csv(sol, filter))?;
    std::fs::write(out.join(format!("{name}_geo.txt")), odm_geo_txt(sol, filter))?;
    std::fs::write(
        out.join(format!("{name}_solution.json")),
        serde_json::to_string_pretty(sol)?,
    )?;
    if sol.exposures.iter().any(|e| e.problem.is_some()) {
        std::fs::write(out.join(format!("{name}_problems.csv")), problems_csv(sol))?;
    }
    Ok(csv)
}

fn print_solution(sol: &FlightSolution) {
    let st = &sol.stats;
    let med = |k: usize| {
        median(
            sol.exposures
                .iter()
                .filter_map(|e| e.camera.and_then(|c| c.sigma_enu_m).map(|s| s[k])),
        )
    };
    println!(
        "   {} exposures: {} fixed, {} float, {} other, {} without position",
        st.exposures, st.exposures_fixed, st.exposures_float, st.exposures_other, st.exposures_missing
    );
    let sigmas = if st.exposures_without_sigma < st.exposures - st.exposures_missing {
        format!("{:.3} / {:.3} / {:.3} m", med(0), med(1), med(2))
    } else {
        "none".into()
    };
    match &sol.method {
        Method::Ppk { .. } => {
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
        }
        Method::RtkShift(_) => {
            println!("   median sigma E/N/U from the MRK (relative to the base): {sigmas}");
            if st.exposures_without_sigma > 0 {
                println!(
                    "   WARNING: {} exposure(s) without standard deviations in the MRK (sigma left empty)",
                    st.exposures_without_sigma
                );
            }
            let not_shifted = sol
                .exposures
                .iter()
                .filter(|e| e.camera.is_none() && e.gps_s.is_some())
                .count();
            if not_shifted > 0 {
                println!(
                    "   WARNING: {not_shifted} exposure(s) not shifted: no RTK position (see problems.csv)"
                );
            }
        }
    }
    if st.files_without_position > 0 {
        println!(
            "   WARNING: {} file(s) could not be positioned at their own exposure time (see problems.csv)",
            st.files_without_position
        );
    }
    if st.max_height_check_m > 0.001 {
        println!(
            "   WARNING: lever-arm height check off by {:.4} m",
            st.max_height_check_m
        );
    }
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
