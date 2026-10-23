# Azimut

Open-source PPK geotagging for DJI survey drones, built for research use: fast on large
multispectral datasets, explicit about reference frames, and validated against ground truth.

> **Status: early development.** Azimut scans and diagnoses DJI flight folders, runs PPK
> through RTKLIB-EX, computes camera centres with the MRK lever arm, and writes CSV and
> OpenDroneMap files (Mavic 3M bands included), with CGVD2013 heights from a geoid grid and,
> on request, the positions written into the images. Frame transformations (ITRF to
> NAD83(CSRS)) and the quality report come next. Everything is tested on synthetic data built
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
  Metashape/Pix4D CSV). `--write-images` writes them into the images in place, changing only
  the position fields and keeping every other byte (Mavic 3M radiometric metadata
  included); the original bytes are saved so that `restore-images` can undo it.
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
(`rnx2rtkp`, options file kept in `results/work/`), interpolates the antenna to each image's
own exposure time (never extrapolating, and refusing gaps over 1 s), adds the MRK lever arm,
and writes:

| File | Content |
|---|---|
| `<flight>_positions.csv` | One row per image file (JPEG and Mavic 3M bands): latitude, longitude, ellipsoidal height, 1-sigma E/N/U, fix status, satellites, frame. Imports into Metashape and Pix4D. `--roles Green` keeps one row per capture for a Metashape multi-camera master band. |
| `<flight>_geo.txt` | OpenDroneMap `geo.txt` (EPSG:4326, ellipsoidal heights), with DJI gimbal yaw/pitch/roll and horizontal/vertical accuracy when the image carries gimbal angles. ODM uses those angles for radiometric calibration, so they are never set to zero. |
| `<flight>_pix4dmatic.csv` | PIX4Dmatic image geolocation file: latitude, longitude, ellipsoidal height and 1-sigma horizontal and vertical accuracy per file, no angles (see below). `<flight>_pix4dmatic.txt` says how to import it. |
| `<flight>_problems.csv` | Exposures without a position, or without an uncertainty, and why. |
| `<flight>_solution.json` | Everything above plus the method (PPK file paths, or the RTK shift), statistics and file paths. |
| `<flight>_report.html`, `_report.json` | Quality report: points to check, captures fixed, trajectory fix rate, horizontal and vertical uncertainty (median and 95th percentile), base, a map of the cameras (status by colour and shape, flight order, scale, north, direction of the base), the 1-sigma E/N/U of each capture, scan findings, the capture table and the files written. A single HTML file without scripts, light and dark, printable; `--report-lang fr` writes it in French. |

When RTKLIB writes a covariance that is not positive definite (it prints 0 for a negative
variance, seen on kinematic solutions), the uncertainty columns are left empty rather than
written as 0 m, which photogrammetry software would read as a perfect position. The position
is kept and the exposure is listed in `problems.csv`.

#### Base position from CSRS-PPP

Submit the base RINEX to [CSRS-PPP](https://webapp.csrs-scrs.nrcan-rncan.gc.ca/geod/tools-outils/ppp.php)
(NRCan), then give its summary file instead of `--base-llh`:

```sh
./target/release/azimut process /path/to/campaign --rtklib /path/to/rtklib/bin \
    --base /path/to/base.26o --base-ppp /path/to/12345678.sum --antex igs20.atx --out results
```

Azimut reads the position, its reference system and epoch (written into every output row),
the 95 % uncertainties and the antenna records, and sets up the base so that RTKLIB uses the
point CSRS-PPP estimated:

| CSRS-PPP summary | The coordinates are | Azimut sets |
|---|---|---|
| `ANT NOT FOUND`, `ARP 0 0 0` | the antenna phase centre (no antenna type or height in the RINEX) | no antenna height, no antenna type; warns that this point is not the ground mark |
| antenna found, `ARP 0 0 0` | the antenna reference point | antenna type, no height; requires `--antex` |
| `ARP` offsets applied | the ground mark | type and offsets from the RINEX header, as CSRS-PPP did; requires `--antex` if the antenna was found |

The RINEX given with `--base` must be the one submitted (Azimut warns when the name or the
antenna offsets differ). `--ppp-system` picks the system when a summary holds several.
CSRS-PPP's orthometric height is printed but outputs stay ellipsoidal.

Coordinates are in the frame of the base coordinates you give. Without `--base-llh`, the
approximate position from the base RINEX header is used: positions are then precise
relative to each other but not in absolute terms. Heights are ellipsoidal; `--geoid` adds
orthometric heights (see below).

Defaults for DJI aircraft: L1+L2, GPS + GLONASS + Galileo + BeiDou, 15° mask, continuous
ambiguity resolution with ratio 3, GLONASS ambiguities left float (base and drone receivers
come from different makers), base observations interpolated to the 5 Hz drone epochs.
`--no-beidou` drops BeiDou for logs with reported decoding problems (Matrice 4).

### RTK flights: shifting to a corrected base position

When the aircraft flew in RTK mode with its own base (a D-RTK 2 or a GNSS receiver), its
real-time positions are relative to the coordinates that base broadcast. If those were only
approximate, for example a base that averaged its own position for a few minutes, every
photo is off by the same vector. Once the base position is known precisely, for example
from a CSRS-PPP solution of the base's RINEX, `shift` moves every real-time camera position
of the MRK by the difference:

```sh
./target/release/azimut shift /path/to/campaign \
    --from 46.3490000,-72.5510000,60.000 \
    --to 46.3489901,-72.5510153,59.412 \
    --frame "NAD83(CSRS) v8, epoch 2010.0" \
    --out results
```

`--from` is what the base broadcast and `--to` the corrected coordinates of the same physical
point, both with ellipsoidal heights. `--to-ppp 12345678.sum` takes the corrected coordinates
from a CSRS-PPP summary instead, and prints which point they refer to (`--from` must be that
point). `--enu E,N,U` gives the shift in metres instead. The
outputs are the same files as `process`. The MRK positions are already camera centres, so no
lever arm is added. Only RTK fixed and float positions are relative to the base: single-point
positions are not shifted and are listed in `problems.csv`. The uncertainties are the
aircraft's own, relative to the base; the uncertainty of the corrected base position is not
added. A vertical shift over 5 m is flagged (an orthometric height given by mistake differs
by the geoid height, about -30 m in Quebec) and a shift over 500 m is refused.

Files exposed at another time than the RGB JPEG (Mavic 3M bands) are placed by linear
interpolation between this MRK record and the next one, if both are RTK and less than 10 s
apart; otherwise they get no position and are listed in `problems.csv`.

Images are not modified unless `--write-images` is given (see below): the positions go to
the CSV and `geo.txt` files that Metashape, Pix4D and ODM import.

### Orthometric heights (CGVD2013)

`--geoid` (with `process` or `shift`) adds `orthometric_height` and `vertical_datum` columns
to the CSV, H = h - N, with the geoid undulation N interpolated bilinearly in a grid in the
PROJ GeoTIFF format. For CGVD2013, download NRCan's CGG2013a converted by PROJ (Open
Government Licence - Canada):

| Positions in | Grid |
|---|---|
| NAD83(CSRS), e.g. from CSRS-PPP | [`ca_nrc_CGG2013an83.tif`](https://cdn.proj.org/ca_nrc_CGG2013an83.tif) |
| ITRF or IGS frames | [`ca_nrc_CGG2013ai08.tif`](https://cdn.proj.org/ca_nrc_CGG2013ai08.tif) |

```sh
./target/release/azimut process /path/to/campaign ... --base-ppp 12345678.sum \
    --geoid ca_nrc_CGG2013an83.tif --out results
```

A grid applies to ellipsoidal heights in one frame, and the two grids differ by 0.1 to 1.1 m
at 12 points across Canada, so Azimut refuses a grid made for another frame family than `--frame`
(NAD83(CSRS) positions with the ITRF grid, say). Within a family (NAD83(CSRS) v6 and v8,
ITRF2008 and IGS20) it notes that the difference between realisations is not applied.
Positions outside the grid get an empty height. `geo.txt` and the images keep ellipsoidal
heights.

### PIX4Dmatic

PIX4D documents a geolocation import format for PIX4Dmatic; its project files are not
documented, so Azimut does not write them. `<flight>_pix4dmatic.csv` uses the "geographic
coordinates with accuracy values" layout: no header (one `#` comment line), then image,
latitude, longitude, z, horizontal and vertical accuracy (1-sigma: sqrt(sE² + sN²) and sU).
Without angles, PIX4Dmatic keeps the orientation read from the images. Create a project with
the images (the originals, or the copies written by `--write-images copy`), then File >
Import > Image geolocation and orientation, and choose the geographic system of the frame
with ellipsoidal heights. With `--geoid`, `<flight>_pix4dmatic_orthometric.csv` holds the
orthometric heights instead. Files without an uncertainty get no row, so PIX4Dmatic keeps the
position read from them.

### Writing positions into the images

`--write-images in-place` or `--write-images copy` (with `process` or `shift`) also writes
each file's position into the image: in the original file, or in a copy in `--images-dir`
(default `<out>/images/<flight>`), leaving the original untouched. Copies take as much disk
space as the flight; images without a position are not copied unless `--copy-unpositioned`
is given. `--dry-run` shows what would be written. The same can be done later from the
solution file, after looking at the report:

```sh
./target/release/azimut write-images results/<flight>_solution.json --mode copy --dry-run
./target/release/azimut write-images results/<flight>_solution.json --mode copy
```

The fields written are:

- EXIF GPS: `GPSLatitude`, `GPSLongitude` (to 1e-6 arc-second), `GPSAltitude` (to 0.1 mm)
  and their reference letters;
- DJI XMP: `drone-dji:GpsLatitude`, `GpsLongitude`, `AbsoluteAltitude`, and the 1-sigma
  `RtkStdLon` (East), `RtkStdLat` (North), `RtkStdHgt` (Up).

Heights are ellipsoidal in the frame of the positions, as DJI writes them: on the Mavic 3M,
EXIF `GPSAltitude`, XMP `AbsoluteAltitude` and the MRK `Ellh` hold the same value, although
EXIF defines the altitude above sea level. `GPSMapDatum` (`WGS-84`), `RtkFlag` and all other
fields are left as they are: the CSV records the frame.

Only the bytes of those values change. Each value is written in the existing field, at the
same size: EXIF rationals keep their 8 bytes, and an XMP value that gets longer or shorter
takes from or gives back to the packet's whitespace padding. The file keeps its length, its
other bytes and the attribute form of the DJI XMP, which matters because OpenDroneMap reads
DJI XMP fields (sensor gain, exposure time, `CaptureUUID`, angles, altitude, accuracy) only
as attributes. General tools rewrite the whole packet: ExifTool 12.76, asked to change one
DJI XMP field, rewrites every field as an element. Before writing, Azimut checks that the new
XMP parses to the same fields apart from the written ones; a file without these fields, with
its XMP already rewritten, or without an uncertainty for the position is skipped and listed.

In place, the original bytes of every change are saved first to `<flight>_image_edits.json`, and

```sh
./target/release/azimut restore-images results/<flight>_image_edits.json
```

puts them back (a file that changed since is left alone). Keep a copy of the SD card anyway.

### Running Azimut from an agent (MCP)

`azimut mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/) server on stdio,
so that an agent such as Claude Code or Codex can run Azimut for a user, in the user's
project folder. Add it once per project:

```sh
claude mcp add azimut -- azimut mcp          # Claude Code
codex mcp add azimut -- azimut mcp           # Codex
```

or share a `.mcp.json` in the project (set `AZIMUT_RTKLIB_DIR` when RTKLIB-EX is not on the
PATH):

```json
{ "mcpServers": { "azimut": { "command": "azimut", "args": ["mcp"],
  "env": { "AZIMUT_RTKLIB_DIR": "/path/to/rtklib/bin" } } } }
```

Then ask, for example: "Do the PPK of the flights in `field/2024-08-09` with the base in
`base/`, coordinates from `base/12345678.sum`, with CGVD2013 heights."

| Tool | Does |
|---|---|
| `scan_folder` | Inventory and diagnosis (read only). |
| `read_ppp_summary` | Position, systems, uncertainty and point of a CSRS-PPP summary (read only). |
| `process_ppk` | PPK; writes the outputs and report, returns the report summary. Images untouched. |
| `shift_rtk` | RTK correction; same outputs. |
| `write_images` | Positions into the images, in place or into copies; dry run unless `confirm: true`. |
| `restore_images` | Undo an in-place write. |
| `read_report` | Full report of a flight. |

The server never takes a decision that is the user's. When the base files, the base
coordinates, their frame, the shift, the image mode or the consent to write images are
missing, the tool does nothing and returns `status: "needs_decision"` (or
`"needs_confirmation"` after a dry run) with the questions for the agent to ask. Coordinates
are never accepted without a frame. The server also gives the agent working instructions and
a `ppk_workflow` prompt (step by step, asking at each decision). Long runs send progress
notifications when the client asks for them; if the client times out tool calls, raise its
limit (Claude Code: `MCP_TOOL_TIMEOUT`).

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
   profiles tested on real logs, merging logs split at 999 photos. This release: CSRS-PPP
   summaries as base position (`--base-ppp`, `--to-ppp`), and `shift` for RTK flights.
3. **`azimut-geo`** (this release: CGVD2013 from PROJ geoid grids, with a frame check):
   NAD83(CSRS) with epoch, refusal of approximate transformations.
4. **`azimut-export`** (this release: CSV, ODM `geo.txt` with angles and accuracy, PIX4Dmatic
   geolocation file, writing into the images in place with undo or into copies, quality
   report in JSON and HTML).
5. **Agents** (this release: `azimut mcp`).
6. **App**: native desktop interface (GPUI).
7. **Validation**: checkpoints (ASPRS Ed. 2), comparison with other tools, open test dataset.

## Checked on real data

On one Mavic 3M flight (firmware 11.08.01.41, August 2024, 13 captures and the full MRK):

- `UTCAtExposure` holds GPS time: it matches the MRK GPS time within 1 µs. The camera
  clock (file names, `ModifyDate`) is therefore 18 s ahead of UTC.
- The MRK times the RGB JPEG. **The four band TIFFs are exposed 26.3 ± 0.3 ms later**, all
  at the same time. At 3.6 m/s that is 9.5 cm, at 10 m/s 26 cm, so Azimut positions every
  file at its own exposure time. The offset between the RGB lens and the band lenses is not
  documented and not applied.
- One XMP packet per JPEG and per TIFF; `CaptureUUID` shared by the five files, `BandName`
  `Green`, `Red`, `RedEdge`, `NIR`; MRK record number equals the file number.
- The MRK can hold more records than images (here 454 for 450 captures, the last capture
  with only JPEG and Green): unpaired records and incomplete captures are reported.
- PPK of the same flight (no RTK: all 454 MRK records single) against the IGS station KUJ2,
  7.6 km away, logging every 30 s (Septentrio PolarX5, `AOAD/M_T_RFI_T NONE` from
  `igs20.atx`, base at its daily IGS20 position from the Nevada Geodetic Laboratory):
  95.6 % of 9278 trajectory epochs fixed, all 13 captures fixed, median sigma 2 / 3 / 4 mm
  (E / N / U), in 1 min 40 s. The fixed camera centres sit a nearly constant 1.1 m north,
  0.4 m east and 3 m below the MRK single-point positions, and the bands 9.4 cm along track
  from their JPEG.
- `--write-images` on copies of the same 63 files: 2998 bytes changed in all, every one in a
  recorded position field; ExifTool reads the new EXIF and XMP positions without warning;
  the DJI XMP keeps its 98 attributes (read as OpenDroneMap does, with `xmltodict`), with
  the new altitude and accuracies; `restore-images` gives files identical to the originals.
- Through `azimut mcp`, a PPK of the same flight (86 s) sends progress notifications and
  returns the report summary; `write_images` stops at a dry run until confirmed.
- CGVD2013: the geoid undulations Azimut interpolates in both CGG2013a grids match PROJ 9.8.1
  within 0.05 mm at 12 points across Canada.

## To confirm on real data

- The same measurements on other firmware versions and on the Mavic 3E, Matrice 4 and
  Matrice 400 (`*_D.MRK` naming).
- The position fields and XMP padding on other DJI models before writing into their images
  (Azimut skips any file without them).
- The meaning of the `*_F.JPG` file found once in a Mavic 3M flight (2592 × 1944, no exposure
  time, same `CaptureUUID` as the first capture).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). In short: every format rule must cite a public
source, and no code, output, screenshot or text from closed-source tools may enter the
repository.

## License

BSD 2-Clause, compatible with RTKLIB-EX. See [LICENSE](LICENSE).
