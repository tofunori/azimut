//! `azimut` command-line tool.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use azimut_io::matching::DEFAULT_TOLERANCE_S;
use azimut_io::scan::AuxKind;
use azimut_io::synthetic::{write_flight, SyntheticFlight, M3M_BAND_SIZE};
use azimut_io::{scan, ScanOptions, ScanReport, Severity};

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
        Command::Scan { dir, json, tolerance } => {
            anyhow::ensure!(dir.is_dir(), "{} is not a folder", dir.display());
            let report = scan(
                &dir,
                &ScanOptions {
                    tolerance_s: tolerance,
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
