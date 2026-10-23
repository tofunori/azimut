//! `azimut` command-line tool.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod mcp;

use anyhow::{Context, Result};
use clap::{ArgGroup, Parser, Subcommand};

use azimut_csrs::{Client as CsrsClient, ProcessType, RefSystem, Request as CsrsRequest, WaitOptions};
use azimut_export::images::{self, FileEdit};
use azimut_export::report::{FlightReport, ImagesInfo, Lang};
use azimut_export::{odm_geo_txt, pix4dmatic_csv, positions_csv_with_heights, problems_csv, RowFilter};
use azimut_geo::GeoidGrid;
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
    #[command(group(ArgGroup::new("ppp_source").args(["base_ppp", "submit_ppp"])))]
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
        /// Submit the base RINEX to CSRS-PPP, wait for the result and use it as --base-ppp
        /// would. Needs --email; the results are kept in <out>/csrs_ppp.
        #[arg(long, conflicts_with_all = ["base_ppp", "base_llh", "antenna_height", "antenna_type"])]
        submit_ppp: bool,
        #[command(flatten)]
        csrs: CsrsArgs,
        /// Reference system to take from the CSRS-PPP summary when it holds several.
        #[arg(long, value_name = "SYSTEM", requires = "ppp_source")]
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
        /// Geoid grid in the PROJ GeoTIFF format, for orthometric heights in the CSV, e.g.
        /// ca_nrc_CGG2013an83.tif (CGVD2013, for NAD83(CSRS) positions) from cdn.proj.org. It
        /// must apply to the frame of the positions (--frame).
        #[arg(long, value_name = "GRID.tif")]
        geoid: Option<PathBuf>,
        #[command(flatten)]
        images: ImageArgs,
        /// Language of the HTML quality report.
        #[arg(long, value_enum, default_value = "en")]
        report_lang: ReportLang,
        /// Largest trajectory gap around an exposure, seconds.
        #[arg(long, default_value_t = 1.0)]
        max_gap: f64,
        /// Pairing tolerance between MRK and image times, seconds.
        #[arg(long, default_value_t = DEFAULT_TOLERANCE_S)]
        tolerance: f64,
    },
    /// Move the real-time (RTK) camera positions of the MRK by a constant base correction,
    /// e.g. after a PPP solution of the RTK base, and write them for every flight. Images are
    /// modified only with --write-images.
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
        /// Geoid grid in the PROJ GeoTIFF format, for orthometric heights in the CSV, e.g.
        /// ca_nrc_CGG2013an83.tif (CGVD2013, for NAD83(CSRS) positions) from cdn.proj.org. It
        /// must apply to the frame of the positions (--frame).
        #[arg(long, value_name = "GRID.tif")]
        geoid: Option<PathBuf>,
        #[command(flatten)]
        images: ImageArgs,
        /// Language of the HTML quality report.
        #[arg(long, value_enum, default_value = "en")]
        report_lang: ReportLang,
        /// Pairing tolerance between MRK and image times, seconds.
        #[arg(long, default_value_t = DEFAULT_TOLERANCE_S)]
        tolerance: f64,
    },
    /// Submit a base-station RINEX to NRCan CSRS-PPP, wait for the result and save it
    /// (summary, positions, PDF). Give the summary to `process --base-ppp` or `shift --to-ppp`.
    #[command(group(ArgGroup::new("job").required(true).args(["rinex", "resume"])))]
    Ppp {
        /// Base-station RINEX observation file, as recorded (compressed files are accepted).
        rinex: Option<PathBuf>,
        #[command(flatten)]
        csrs: CsrsArgs,
        /// Fetch the results of an earlier submission (key printed when it was submitted)
        /// instead of submitting again.
        #[arg(long, value_name = "KEY")]
        resume: Option<String>,
        /// Output folder.
        #[arg(long, default_value = "azimut_out/csrs_ppp")]
        out: PathBuf,
    },
    /// Write the positions of a computed flight (<flight>_solution.json from `process` or
    /// `shift`) into its images, in place or into copies. Use --dry-run first to see what
    /// would be written.
    WriteImages {
        /// Solution file written by `process` or `shift`.
        solution: PathBuf,
        /// `in-place` (original bytes saved for `restore-images`) or `copy` (originals untouched).
        #[arg(long, value_enum)]
        mode: ImageMode,
        /// Folder for the copies (default: images/<flight> next to the solution file).
        #[arg(long)]
        images_dir: Option<PathBuf>,
        /// With `copy`, also copy images without a position (they keep the aircraft's own).
        #[arg(long)]
        copy_unpositioned: bool,
        /// Show what would be written without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Undo --write-images: put back the original bytes saved in <flight>_image_edits.json.
    RestoreImages {
        /// Edit record written by --write-images.
        file: PathBuf,
    },
    /// Run as a Model Context Protocol server on stdio, so that an agent (Claude, Codex...)
    /// can scan, process and write images for a user, asking at each decision.
    Mcp,
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
        /// Fly a survey pattern (back-and-forth lines) with varying uncertainties.
        #[arg(long)]
        survey: bool,
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
            submit_ppp,
            csrs,
            ppp_system,
            antex,
            rtklib,
            no_beidou,
            out,
            roles,
            geoid,
            images,
            report_lang,
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

            let mut ppp_notes = None;
            let (ppp, ppp_path, ppp_system) = if submit_ppp {
                let rinex = submitted_base(&report)?;
                let r = csrs_fetch(Some(&rinex), None, &csrs, &out.join("csrs_ppp"))?;
                let system = ppp_system.or_else(|| system_of(&r.summary, csrs.sysref));
                ppp_notes = r.notes;
                (Some(r.summary), Some(r.summary_path), system)
            } else {
                match &base_ppp {
                    Some(path) => (Some(read_ppp(path)?), Some(path.clone()), ppp_system),
                    None => (None, None, ppp_system),
                }
            };
            let base_setup = match (&ppp, &ppp_path) {
                (Some(sum), Some(path)) => {
                    let (mut setup, point) = BaseSetup::from_ppp(sum, ppp_system.as_deref(), antex.is_some())
                        .map_err(anyhow::Error::msg)
                        .with_context(|| format!("CSRS-PPP summary {}", path.display()))?;
                    print_ppp(sum, ppp_system.as_deref(), &point)?;
                    print_ppp_notes(ppp_notes.as_deref());
                    if let Some(f) = frame {
                        setup.frame = f;
                    }
                    setup
                }
                _ => {
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
            let outputs = Outputs {
                filter: row_filter(roles),
                geoid: open_geoid(geoid.as_deref(), &opts.base.frame)?,
                images,
                lang: report_lang.into(),
            };
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
                        let base = f.base.as_ref().map(|b| (b, &report.rinex[b.rinex]));
                        if finish_flight(&out, &name, &sol, Some(f), base, &outputs)? {
                            failures += 1;
                        }
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
            geoid,
            images,
            report_lang,
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

            let outputs = Outputs {
                filter: row_filter(roles),
                geoid: open_geoid(geoid.as_deref(), &frame)?,
                images,
                lang: report_lang.into(),
            };
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
                if finish_flight(&out, &name, &sol, Some(f), None, &outputs)? {
                    failures += 1;
                }
            }
            std::process::exit(if failures > 0 { 2 } else { 0 });
        }
        Command::Ppp {
            rinex,
            csrs,
            resume,
            out,
        } => {
            let r = csrs_fetch(rinex.as_deref(), resume.as_deref(), &csrs, &out)?;
            let system = system_of(&r.summary, csrs.sysref);
            let (_, point) =
                BaseSetup::from_ppp(&r.summary, system.as_deref(), true).map_err(anyhow::Error::msg)?;
            print_ppp(&r.summary, system.as_deref(), &point)?;
            print_ppp_notes(r.notes.as_deref());
            if r.summary.positions.len() > 1 {
                let all: Vec<&str> = r.summary.positions.iter().map(|p| p.system.as_str()).collect();
                println!(
                    "   The summary also holds {}; choose with --ppp-system.",
                    all.join(", ")
                );
            }
            println!(
                "\nNext: azimut process <flights> --base {} --base-ppp {}",
                rinex
                    .as_deref()
                    .map_or("<base RINEX>".into(), |p| p.display().to_string()),
                r.summary_path.display()
            );
            Ok(())
        }
        Command::WriteImages {
            solution,
            mode,
            images_dir,
            copy_unpositioned,
            dry_run,
        } => {
            let text = std::fs::read_to_string(&solution)
                .with_context(|| format!("reading {}", solution.display()))?;
            let sol: FlightSolution = serde_json::from_str(&text)
                .with_context(|| format!("{} is not an Azimut solution file", solution.display()))?;
            let out = solution.parent().unwrap_or(Path::new(".")).to_path_buf();
            let name = flight_name(&sol.dir);
            let args = ImageArgs {
                write_images: Some(mode),
                images_dir,
                copy_unpositioned,
                dry_run,
            };
            println!("== Images of {name} ({})", sol.frame);
            let (_, problem) = write_images_step(&out, &name, &sol, &args)?;
            std::process::exit(if problem { 2 } else { 0 });
        }
        Command::RestoreImages { file } => {
            let text =
                std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
            let record: serde_json::Value = serde_json::from_str(&text)?;
            let edits: Vec<FileEdit> = serde_json::from_value(record["edits"].clone())
                .with_context(|| format!("{} is not an Azimut image edit record", file.display()))?;
            let (mut restored, mut already, mut failed) = (0, 0, 0);
            for edit in &edits {
                match images::revert(edit) {
                    Ok(true) => restored += 1,
                    Ok(false) => already += 1,
                    Err(e) => {
                        println!("   {}: {e}", edit.path.display());
                        failed += 1;
                    }
                }
            }
            println!("{restored} image(s) restored, {already} already original, {failed} not restored.");
            std::process::exit(if failed > 0 { 2 } else { 0 });
        }
        Command::Mcp => mcp::serve(),
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
            survey,
        } => {
            let spec = SyntheticFlight {
                captures,
                clock_is_utc: utc,
                survey,
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

/// CSRS-PPP submission options, shared by `ppp` and `process --submit-ppp`.
#[derive(clap::Args, Clone)]
struct CsrsArgs {
    /// E-mail of your CSRS-PPP account (NRCan), for submissions.
    #[arg(long, env = "AZIMUT_CSRS_EMAIL", value_name = "ADDRESS")]
    email: Option<String>,
    /// Reference system CSRS-PPP computes in: ITRF at the epoch of the observations, or
    /// NAD83(CSRS) at the current epoch with CGVD2013 heights.
    #[arg(long, value_enum, default_value = "itrf")]
    sysref: Sysref,
    /// CSRS-PPP processing mode of the base RINEX.
    #[arg(long, value_enum, default_value = "static")]
    ppp_mode: PppMode,
    /// Longest wait for the CSRS-PPP result, minutes; the tracking key is printed to fetch
    /// it later (`azimut ppp --resume`).
    #[arg(long, default_value_t = 15.0, value_name = "MIN")]
    ppp_timeout: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Sysref {
    Itrf,
    Nad83,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PppMode {
    Static,
    Kinematic,
}

/// Submit `rinex` to CSRS-PPP (or take the earlier submission `resume`), wait, then write
/// the results archive and its files into `out`.
fn csrs_fetch(
    rinex: Option<&Path>,
    resume: Option<&str>,
    args: &CsrsArgs,
    out: &Path,
) -> Result<azimut_csrs::Results> {
    anyhow::ensure!(
        args.ppp_timeout > 0.0 && args.ppp_timeout <= 24.0 * 60.0,
        "--ppp-timeout must be between 0 and 1440 minutes"
    );
    let client = CsrsClient::new()?;
    let key = match (resume, rinex) {
        (Some(key), _) => key.trim().to_string(),
        (None, Some(rinex)) => {
            let email = args.email.as_deref().context(
                "CSRS-PPP needs the e-mail of your account: give --email or set AZIMUT_CSRS_EMAIL",
            )?;
            let req = CsrsRequest {
                rinex,
                email,
                process_type: match args.ppp_mode {
                    PppMode::Static => ProcessType::Static,
                    PppMode::Kinematic => ProcessType::Kinematic,
                },
                system: match args.sysref {
                    Sysref::Itrf => RefSystem::Itrf,
                    Sysref::Nad83 => RefSystem::Nad83,
                },
            };
            println!("\nSubmitting {} to CSRS-PPP...", rinex.display());
            let key = client
                .submit(&req)
                .with_context(|| format!("submitting {} to CSRS-PPP", rinex.display()))?;
            println!("   tracking key: {key}");
            key
        }
        (None, None) => anyhow::bail!("give a RINEX file to submit, or --resume KEY"),
    };
    let opts = WaitOptions {
        timeout: Duration::from_secs_f64(args.ppp_timeout * 60.0),
        ..WaitOptions::default()
    };
    print!("   processing");
    std::io::stdout().flush().ok();
    let start = std::time::Instant::now();
    let wait = client.wait(&key, &opts, |_| {
        print!(".");
        std::io::stdout().flush().ok();
    });
    println!();
    if let Err(e) = wait {
        // The submission may still finish: say how to fetch it.
        if matches!(
            e,
            azimut_csrs::Error::Timeout { .. } | azimut_csrs::Error::Http { .. }
        ) {
            anyhow::bail!(
                "{e}\n   fetch them with: azimut ppp --resume {key} --out {}",
                out.display()
            );
        }
        return Err(e.into());
    }
    println!("   done after {} s", start.elapsed().as_secs());
    let archive = client.download(&key)?;
    let r = azimut_csrs::extract(&key, &archive, out)?;
    let stem = r
        .summary_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "results".into());
    let zip = out.join(format!("{stem}_csrs_ppp.zip"));
    std::fs::write(&zip, &archive).with_context(|| format!("writing {}", zip.display()))?;
    println!(
        "   results in {} ({} files and the archive)",
        out.display(),
        r.files.len()
    );
    Ok(r)
}

/// Warnings CSRS-PPP wrote in errors.txt alongside its solution.
fn print_ppp_notes(notes: Option<&str>) {
    if let Some(text) = notes {
        println!("   CSRS-PPP notes (errors.txt):");
        for line in text.lines() {
            println!("     {line}");
        }
    }
}

/// The summary system matching the reference system asked for, when it holds several.
fn system_of(sum: &PppSummary, sysref: Sysref) -> Option<String> {
    if sum.positions.len() < 2 {
        return None;
    }
    let nad83 = |s: &str| s.to_ascii_uppercase().contains("NAD83");
    sum.positions
        .iter()
        .find(|p| nad83(&p.system) == (sysref == Sysref::Nad83))
        .map(|p| p.system.clone())
}

/// The one base RINEX the flights use, for `--submit-ppp`.
fn submitted_base(report: &ScanReport) -> Result<PathBuf> {
    let mut bases: Vec<&Path> = report
        .flights
        .iter()
        .filter_map(|f| f.base.as_ref())
        .map(|b| report.rinex[b.rinex].path.as_path())
        .collect();
    bases.sort();
    bases.dedup();
    match bases.as_slice() {
        [one] => Ok(one.to_path_buf()),
        [] => anyhow::bail!("--submit-ppp: no base RINEX covers the flights (use --base)"),
        many => anyhow::bail!(
            "--submit-ppp: the flights use {} base RINEX files ({}); submit each with `azimut ppp` \
             and process each group of flights with its own --base and --base-ppp",
            many.len(),
            many.iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
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

/// How positions are written into images.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ImageMode {
    /// Change the original files (the original bytes are saved for `restore-images`).
    InPlace,
    /// Write into copies; the originals are not touched.
    Copy,
}

#[derive(clap::Args, Clone)]
struct ImageArgs {
    /// Also write the positions into the images (EXIF GPS and DJI XMP): `in-place` (original
    /// bytes saved to <flight>_image_edits.json for `restore-images`) or `copy` (copies in
    /// --images-dir, originals untouched).
    #[arg(long, value_enum, value_name = "MODE")]
    write_images: Option<ImageMode>,
    /// Folder for the copies (default: <out>/images/<flight>).
    #[arg(long, requires = "write_images")]
    images_dir: Option<PathBuf>,
    /// With `copy`, also copy images without a position (they keep the aircraft's own).
    #[arg(long, requires = "write_images")]
    copy_unpositioned: bool,
    /// Show what would be written into the images without writing anything.
    #[arg(long, requires = "write_images")]
    dry_run: bool,
}

/// Language of the HTML report.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ReportLang {
    En,
    Fr,
}

impl From<ReportLang> for Lang {
    fn from(l: ReportLang) -> Lang {
        match l {
            ReportLang::En => Lang::En,
            ReportLang::Fr => Lang::Fr,
        }
    }
}

/// What to write besides the position files.
struct Outputs {
    filter: RowFilter,
    geoid: Option<GeoidGrid>,
    images: ImageArgs,
    lang: Lang,
}

/// Open the geoid grid and check that it applies to the frame of the positions.
fn open_geoid(path: Option<&Path>, frame: &str) -> Result<Option<GeoidGrid>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let grid = GeoidGrid::open(path)?;
    match grid.check_frame(frame) {
        Ok(note) => {
            println!(
                "\nOrthometric heights: {}, from {} (grid for {}).",
                grid.vertical_datum(),
                path.display(),
                grid.source_frame()
            );
            if let Some(n) = note {
                println!("Note: {n}");
            }
            Ok(Some(grid))
        }
        Err(e) => anyhow::bail!("--geoid: {e}"),
    }
}

/// Write every output of a solved flight: position files, images on request, report.
/// Returns true when something went wrong with the images.
fn finish_flight(
    out: &Path,
    name: &str,
    sol: &FlightSolution,
    flight: Option<&azimut_io::Flight>,
    base: Option<(&azimut_io::scan::BaseCheck, &azimut_io::rinex::RinexSummary)>,
    outputs: &Outputs,
) -> Result<bool> {
    let mut written = write_outputs(out, name, sol, outputs)?;
    print_solution(sol);
    let mut rep = FlightReport::build(sol, flight, base, outputs.geoid.as_ref());
    let mut problem = false;
    if outputs.images.write_images.is_some() {
        let (info, p) = write_images_step(out, name, sol, &outputs.images)?;
        rep.images = Some(info);
        problem = p;
    }
    let json = out.join(format!("{name}_report.json"));
    let html = out.join(format!("{name}_report.html"));
    written.push(json.clone());
    written.push(html.clone());
    rep.outputs = written;
    std::fs::write(&json, serde_json::to_string_pretty(&rep)?)?;
    std::fs::write(&html, rep.html_in(outputs.lang))?;
    println!(
        "   written: {} (and {} more files)",
        rep.outputs[0].display(),
        rep.outputs.len() - 1
    );
    println!("   report: {}", html.display());
    for a in &rep.attention {
        println!("   to check: {}", a.text);
    }
    Ok(problem)
}

/// Write the position files of one flight (CSV, geo.txt, PIX4Dmatic, solution JSON and,
/// when needed, problems). Returns their paths, the CSV first.
fn write_outputs(out: &Path, name: &str, sol: &FlightSolution, outputs: &Outputs) -> Result<Vec<PathBuf>> {
    let filter = &outputs.filter;
    let geoid = outputs.geoid.as_ref();
    let mut written = Vec::new();
    let mut put = |file: String, text: String| -> Result<()> {
        let path = out.join(file);
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
        Ok(())
    };
    put(
        format!("{name}_positions.csv"),
        positions_csv_with_heights(sol, filter, geoid),
    )?;
    if let Some(g) = geoid {
        let outside = sol
            .exposures
            .iter()
            .flat_map(|e| &e.files)
            .filter_map(|f| f.camera)
            .filter(|c| g.undulation(c.lat_deg, c.lon_deg).is_none())
            .count();
        if outside > 0 {
            println!("   WARNING: {outside} file(s) outside the geoid grid: orthometric height left empty");
        }
    }
    put(format!("{name}_geo.txt"), odm_geo_txt(sol, filter))?;

    let (p4d, left_out) = pix4dmatic_csv(sol, filter, None);
    put(format!("{name}_pix4dmatic.csv"), p4d)?;
    let mut ortho = None;
    if let Some(g) = geoid {
        let (text, n) = pix4dmatic_csv(sol, filter, Some(g));
        put(format!("{name}_pix4dmatic_orthometric.csv"), text)?;
        ortho = Some((g.vertical_datum().to_string(), n));
    }
    put(
        format!("{name}_pix4dmatic.txt"),
        pix4dmatic_instructions(name, sol, left_out, ortho.as_ref()),
    )?;

    put(
        format!("{name}_solution.json"),
        serde_json::to_string_pretty(sol)?,
    )?;
    if sol.exposures.iter().any(|e| e.problem.is_some()) {
        put(format!("{name}_problems.csv"), problems_csv(sol))?;
    }
    Ok(written)
}

/// How to import the PIX4Dmatic geolocation files, following PIX4D's documentation.
fn pix4dmatic_instructions(
    name: &str,
    sol: &FlightSolution,
    left_out: usize,
    ortho: Option<&(String, usize)>,
) -> String {
    let mut s = format!(
        "Importing the positions of {name} into PIX4Dmatic\n\
         (steps from PIX4D's documentation, \"How to import and export image geolocation and orientation files\")\n\n\
         1. Create a new project and import the images of this flight.\n\
         2. File > Import > Image geolocation and orientation, and choose {name}_pix4dmatic.csv.\n\
         3. Coordinate reference system: horizontal = the geographic (latitude, longitude) system of\n   \
         the frame below; vertical = ellipsoidal height.\n\
         4. Apply.\n\n\
         Frame of the positions: {}\n\
         Accuracies: 1-sigma in metres, horizontal radial (sqrt(sE^2 + sN^2)) and vertical.\n\
         The file holds no angles: PIX4Dmatic keeps the orientation read from the images.\n",
        sol.frame
    );
    if left_out > 0 {
        s += &format!(
            "{left_out} file(s) without an uncertainty are not in the file: PIX4Dmatic keeps the\n\
             position read from those images.\n"
        );
    }
    if let Some((datum, n)) = ortho {
        s += &format!(
            "\nOrthometric heights: {name}_pix4dmatic_orthometric.csv holds {datum} heights instead.\n\
             Choose that vertical system (no geoid) when importing it.\n"
        );
        if *n > 0 {
            s += &format!(
                "{n} file(s) without an uncertainty or outside the geoid grid are left out of it.\n"
            );
        }
    }
    s
}

/// Write the positions into the images of one flight, in place or into copies, or show what
/// would be written. In place, the edit record is saved before any image is touched.
/// Returns what was done, and true when some image could not be written.
fn write_images_step(
    out: &Path,
    name: &str,
    sol: &FlightSolution,
    args: &ImageArgs,
) -> Result<(ImagesInfo, bool)> {
    let mode = args.write_images.unwrap_or(ImageMode::InPlace);
    let show_skipped = |skipped: &[(PathBuf, String)]| {
        for (p, why) in skipped.iter().take(5) {
            println!(
                "      skipped {}: {why}",
                p.file_name().unwrap_or_default().to_string_lossy()
            );
        }
        if skipped.len() > 5 {
            println!("      ... and {} more", skipped.len() - 5);
        }
    };
    if args.dry_run {
        let planned = images::plan_flight(sol);
        let bytes: usize = planned
            .written
            .iter()
            .flat_map(|e| &e.patches)
            .map(|p| p.new.len() / 2)
            .sum();
        println!(
            "   images (dry run, nothing written): {} file(s) would be written {}, {bytes} bytes rewritten; {} skipped",
            planned.written.len(),
            if mode == ImageMode::Copy { "into copies" } else { "in place" },
            planned.skipped.len()
        );
        show_skipped(&planned.skipped);
        let info = ImagesInfo {
            mode: "dry_run".into(),
            written: planned.written.len(),
            skipped: planned.skipped.len(),
            failed: 0,
            location: out.to_path_buf(),
        };
        return Ok((info, !planned.skipped.is_empty()));
    }
    match mode {
        ImageMode::InPlace => {
            let planned = images::plan_flight(sol);
            let record = out.join(format!("{name}_image_edits.json"));
            let skipped: Vec<_> = planned
                .skipped
                .iter()
                .map(|(p, why)| serde_json::json!({ "path": p, "reason": why }))
                .collect();
            let doc = serde_json::json!({
                "frame": sol.frame,
                "heights": "ellipsoidal",
                "edits": planned.written,
                "skipped": skipped,
            });
            std::fs::write(&record, serde_json::to_string_pretty(&doc)?)
                .with_context(|| format!("writing {}", record.display()))?;
            let mut failed = 0;
            for edit in &planned.written {
                if let Err(e) = images::apply(edit) {
                    println!("   image not written: {}: {e}", edit.path.display());
                    failed += 1;
                }
            }
            println!(
                "   images: {} written in place (EXIF GPS and DJI XMP, ellipsoidal heights), {} skipped, {failed} failed",
                planned.written.len() - failed,
                planned.skipped.len()
            );
            show_skipped(&planned.skipped);
            println!(
                "   original bytes saved: {} (undo: azimut restore-images)",
                record.display()
            );
            let info = ImagesInfo {
                mode: "in_place".into(),
                written: planned.written.len() - failed,
                skipped: planned.skipped.len(),
                failed,
                location: record,
            };
            Ok((info, failed > 0 || !planned.skipped.is_empty()))
        }
        ImageMode::Copy => {
            let dest = args
                .images_dir
                .clone()
                .unwrap_or_else(|| out.join("images").join(name));
            let r = images::copy_flight(sol, &dest, args.copy_unpositioned).map_err(anyhow::Error::msg)?;
            let intended = |why: &String| why.starts_with("no position") || why.starts_with("copied without");
            let errors = r.skipped.iter().filter(|(_, why)| !intended(why)).count();
            println!(
                "   images: {} copied with their position into {}, {} not written ({errors} error(s))",
                r.written.len(),
                dest.display(),
                r.skipped.len()
            );
            show_skipped(&r.skipped);
            let info = ImagesInfo {
                mode: "copy".into(),
                written: r.written.len(),
                skipped: r.skipped.len(),
                failed: errors,
                location: dest,
            };
            Ok((info, errors > 0))
        }
    }
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
