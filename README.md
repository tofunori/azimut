# Azimut

Open-source PPK geotagging for DJI survey drones, built for research use: fast on large
multispectral datasets, explicit about reference frames, and validated against ground truth.

> **Status: early development.** Azimut scans and diagnoses DJI flight folders, runs PPK
> through RTKLIB-EX, computes camera centres with the MRK lever arm, and writes CSV and
> OpenDroneMap files (Mavic 3M bands included). Coordinate transformations (NAD83(CSRS),
> CGVD2013) and the quality report come next. Everything is tested on synthetic data built
> from public DJI documentation and with stand-in RTKLIB programs; it still has to be
> confirmed on real SD-card copies and against checkpoints.

*En bref (français) : Azimut est un outil libre de géotag PPK pour les drones DJI, développé
au GlacioLab (UQTR). Le moteur est en Rust, lit seulement les en-têtes des images (aucun
pixel) et traite le Mavic 3M multispectral (1 JPEG + 4 TIF par déclenchement). Le calcul
GNSS est confié à RTKLIB-EX.*

## Why

Commercial tools do the job but are closed, Windows-only or slow on large TIFF sets. Free
tools such as Emlid Studio refuse Mavic 3M multispectral data (one MRK line, five files).
The GNSS engine itself is not the problem: RTKLIB-EX is open and mature. What is missing is
the work around it: reading each DJI model reliably, checking the data before processing,
handling multispectral captures, exporting to photogrammetry software, and reporting
quality. Azimut does that work in the open.

## Design rules

- **Headers only.** Metadata is read from JPEG APP1 segments and TIFF tag 700. Pixels are
  never read, so a 4 GB multispectral flight scans in a fraction of a second.
- **Images are read-only by default.** Positions go to external files (ODM `geo.txt`,
  Metashape/Pix4D CSV). Writing into images will be an explicit option that checks the
  radiometric metadata of Mavic 3M TIFFs stays intact.
- **Pair on time, never on file order.** MRK GPS week/time of week is matched to XMP
  `UTCAtExposure`. Whether that field holds GPS time or UTC is decided from the data.
- **Read by meaning, not by position.** MRK values are read by their suffix (`,N`, `,Lat`,
  `,Q`...), bands by XMP `BandName`, captures by `CaptureUUID`.
- **No silent approximations.** Reference frame, epoch and height type will be carried with
  every coordinate; approximate ("ballpark") datum transformations will be refused.
- **Every rule has a public source.** See [SOURCES.md](SOURCES.md).

## Quick start

```sh
cargo build --release
# Try it without data: write a synthetic Mavic 3M flight, then scan it
./target/release/azimut demo /tmp/m3m-demo --captures 100 --full-size   # ~4 GB of TIFFs
./target/release/azimut scan /tmp/m3m-demo
# Real data: point it at an SD-card copy or a whole campaign folder
./target/release/azimut scan /path/to/DCIM --json report.json
# With a base station recorded elsewhere (file or folder, RINEX 2/3/4, Hatanaka headers)
./target/release/azimut scan /path/to/DCIM --base /path/to/base.26o
```

### PPK processing

Azimut drives [RTKLIB-EX](https://github.com/rtklibexplorer/RTKLIB) (BSD-2), which it does
not ship. Build its console programs `convbin` and `rnx2rtkp` (for example with `make` in
`app/consapp/convbin/gcc` and `app/consapp/rnx2rtkp/gcc`, or use the Windows release
binaries), then point Azimut at them. The `rnx2rtkp` makefile links `libgfortran`, which it
does not need without LAPACK: if it is not installed, build with `make LDLIBS=-lm`.

```sh
./target/release/azimut process /path/to/campaign \
    --rtklib /path/to/rtklib/bin \
    --base /path/to/base.26o \
    --base-llh 46.3491234,-72.5512345,31.204 \
    --antenna-height 1.800 \
    --frame "NAD83(CSRS) v8, epoch 2010.0" \
    --out results
```

For each flight folder, `process` converts the aircraft's raw RTCM 3 log to RINEX 3.04
(`convbin`), runs a kinematic, forward/backward-combined PPK against the base
(`rnx2rtkp`, options file kept in `results/work/`), interpolates the antenna to each MRK
exposure (never extrapolating, and refusing gaps over 1 s), adds the MRK lever arm, and
writes:

| File | Content |
|---|---|
| `<flight>_positions.csv` | One row per image file (JPEG and Mavic 3M bands): latitude, longitude, ellipsoidal height, 1-sigma E/N/U, fix status, satellites, frame. Imports into Metashape and Pix4D. `--roles Green` keeps one row per capture for a Metashape multi-camera master band. |
| `<flight>_geo.txt` | OpenDroneMap `geo.txt` (EPSG:4326, ellipsoidal heights), with DJI gimbal yaw/pitch/roll and horizontal/vertical accuracy when the image carries gimbal angles. ODM uses those angles for radiometric calibration, so they are never set to zero. |
| `<flight>_problems.csv` | Exposures without a position, or without an uncertainty, and why. |
| `<flight>_solution.json` | Everything above plus the trajectory statistics and file paths. |

When RTKLIB writes a covariance that is not positive definite (it prints 0 for a negative
variance, seen on kinematic solutions), the uncertainty columns are left empty rather than
written as 0 m, which photogrammetry software would read as a perfect position. The position
is kept and the exposure is listed in `problems.csv`.

Coordinates are in the frame of the base coordinates you give. Without `--base-llh`, the
approximate position from the base RINEX header is used: positions are then precise
relative to each other but not in absolute terms. Heights are ellipsoidal; orthometric
heights (CGVD2013) come with `azimut-geo`.

Defaults for DJI aircraft: L1+L2, GPS + GLONASS + Galileo + BeiDou, 15° mask, continuous
ambiguity resolution with ratio 3, GLONASS ambiguities left float (base and drone receivers
come from different makers), base observations interpolated to the 5 Hz drone epochs.
`--no-beidou` drops BeiDou for logs with reported decoding problems (Matrice 4).

### RINEX checks

RINEX files are summarised from their header, first epochs and last epoch only, so large
base files are checked in milliseconds. RINEX observation files found in the scanned tree
outside the flight folders are also treated as base candidates; the one that best covers
each flight is used.

`scan` exits with code 2 when it finds an error that would break a PPK run.

Example output (100 full-size synthetic captures):

```
Scanned /tmp/m3m-demo in 0.06 s: 1 flight folder(s), 499 images, 4.0 GB on disk, 7.5 MB read.

== .
   499 images, 100 captures (100 multispectral), 100 MRK records, raw GNSS logs: 1, base logs: 0
   [WARN ] 1 of 100 multispectral captures are incomplete: #0006 (missing NIR).
   [info ] Image exposure times (XMP UTCAtExposure) read as GPS time, despite the UTC label.
   [info ] 100 of 100 MRK records paired with a capture (largest time gap 0.000000 s).
   [info ] Real-time status in the MRK: 98.0 % fixed (98 fixed, 2 float, 0 single, 0 none). ...
```

## What `scan` checks

| Check | Why it matters |
|---|---|
| MRK present, raw GNSS log present (`*_PPKRAW.bin`, `*.RTK`) | Without them PPK is impossible |
| Unreadable MRK lines | Silent shifts in pairing |
| Mavic 3M captures complete (1 JPEG + Green, Red, RedEdge, NIR) | Missing bands break multispectral processing |
| Captures grouped without `CaptureUUID` | Weaker grouping, flagged |
| MRK records and captures paired within 0.05 s, GPS-time or UTC clock | Wrong pairing moves every photo |
| Records without capture, captures without record | Deleted photos, mixed folders |
| Real-time fix rate in the MRK | Early warning on GNSS conditions |
| Photo number 999 reached | DJI splits the folder and the GNSS log ends early |
| Numbering not starting at 0001 | Interrupted missions; order-based tools fail here |
| On-board RINEX dated before 2000 | DJI files written before the receiver set its clock |
| Navigation file present | PPK needs orbits |
| Base RINEX covers every exposure, with margin | Photos outside the base cannot be solved |
| Base interval, frequencies, antenna type and height | 30 s or single-frequency bases weaken the solution; a missing antenna type blocks phase-centre corrections |
| Base distance to the flight | About 1 cm more error per 10 km of baseline |

## Supported hardware

The MRK layout is the same in the Matrice 4, Matrice 400, Mavic 3E/3M and Zenmuse P1
manuals. Tested so far on synthetic data only.

| Platform | Status |
|---|---|
| Mavic 3 Multispectral | Grouping and pairing implemented, needs real data |
| Mavic 3 Enterprise | Implemented (same files as 3M, no bands), needs real data |
| Matrice 4T / Matrice 400 (`*.RTK`, `*_D.MRK`) | File types recognised, needs real data |
| Matrice 300 RTK + Zenmuse P1 | Implemented, needs real data |
| Matrice 300 RTK + H20 series | No PPK files documented by DJI |

## Roadmap

1. **`azimut-io`** (this release): header readers, MRK, RINEX summaries, inventory, Mavic 3M
   grouping, pairing, base coverage, diagnosis. Next: local metadata index, folder-split
   merging, gzip support.
2. **`azimut-solve`** (this release): RTKLIB-EX `convbin` / `rnx2rtkp`, combined filter,
   ECEF solutions with full covariance, interpolation, MRK lever arm. Next: per-drone
   profiles tested on real logs, merging logs split at 999 photos, CSRS-PPP base position.
3. **`azimut-geo`**: NAD83(CSRS) with epoch, CGVD2013, refusal of approximate
   transformations.
4. **`azimut-export`** (this release: CSV, ODM `geo.txt`): camera angles and accuracy in
   `geo.txt`, Pix4D-specific CSV, optional in-place writing with ExifTool.
5. **`azimut-report`**: quality report (fix rate, uncertainty map, base distance).
6. **App**: native desktop interface (GPUI).
7. **Validation**: checkpoints (ASPRS Ed. 2), comparison with other tools, open test dataset.

## To confirm on real data

- Whether `UTCAtExposure` holds GPS time or UTC on each model (handled either way).
- Whether the JPEG and TIFF of one Mavic 3M capture carry the same exposure time.
- Exact MRK layout of the Matrice 4 and Matrice 400 (`*_D.MRK` naming).
- Number of XMP packets in Mavic 3M TIFFs (all are read).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). In short: every format rule must cite a public
source, and no code, output, screenshot or text from closed-source tools may enter the
repository.

## License

BSD 2-Clause, compatible with RTKLIB-EX. See [LICENSE](LICENSE).
