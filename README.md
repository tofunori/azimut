<div align="center">

# Azimut

**Open-source PPK geotagging for DJI survey drones**

Precise camera positions for photogrammetry, from the SD card to PIX4D, Metashape and
OpenDroneMap, with the multispectral Mavic 3M handled file by file.

[![CI](https://github.com/tofunori/azimut/actions/workflows/ci.yml/badge.svg)](https://github.com/tofunori/azimut/actions/workflows/ci.yml)
[![License: BSD-2-Clause](https://img.shields.io/badge/license-BSD--2--Clause-blue.svg)](LICENSE)
![Rust 1.82+](https://img.shields.io/badge/rust-1.82%2B-orange.svg)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-lightgrey.svg)

[Français](README.fr.md) · [Install](docs/install.md) · [Documentation](#documentation) · [Validation](docs/validation.md)

</div>

---

Azimut reads a DJI flight folder, checks it, runs a post-processed kinematic (PPK) solution
against a base station with [RTKLIB-EX](https://github.com/rtklibexplorer/RTKLIB), and writes
the camera position of every image, with its uncertainty, its reference frame and, on
request, its CGVD2013 height. It is an open alternative to the closed, often Windows-only
tools used for this step, built for research: every rule can be traced to a public source,
and nothing is approximated silently. Developed at the GlacioLab, Université du Québec à
Trois-Rivières (UQTR).

<p align="center">
  <img src="docs/img/report-en.png" alt="Azimut quality report: captures fixed, uncertainties, camera map and uncertainty per capture" width="820">
  <br><sub>The quality report written for every flight (synthetic demo flight; light and dark themes, English or French).</sub>
</p>

## What it does

| | |
|---|---|
| **Diagnose before processing** | Inventory of a whole SD card or campaign in seconds, from image headers only: incomplete Mavic 3M captures, MRK/photo pairing, real-time fix rate, base-station coverage, interval and distance. |
| **PPK** | Raw drone GNSS log + base RINEX → RTKLIB-EX combined solution → antenna interpolated to each file's exposure time → camera centre with the DJI lever arm. |
| **Mavic 3M, file by file** | The four band TIFFs are exposed 26 ms after the RGB JPEG; each file gets its own position (up to 26 cm difference at 10 m/s). |
| **RTK correction** | For RTK flights, shift every position by the correction of the base, e.g. from an NRCan **CSRS-PPP** solution. |
| **Heights** | Ellipsoidal heights in the frame you state, and CGVD2013 orthometric heights from NRCan's CGG2013a geoid, with a check that the grid matches the frame. |
| **Outputs** | CSV for Metashape and PIX4D, PIX4Dmatic geolocation file, OpenDroneMap `geo.txt`, a problems list, and an HTML/JSON **quality report**. |
| **Into the images** | Optionally write the positions into the EXIF and DJI XMP, in place (undoable) or into copies, changing nothing else, not even the Mavic 3M radiometric metadata. |
| **Agents** | `azimut mcp` lets Claude Code or Codex run the whole workflow, asking the user at each decision instead of guessing. |

## Validation

Checked end to end on a full **Mavic 3M** flight (450 captures, 2248 files, no RTK) against
an IGS station 7.6 km away: **450 of 450 captures fixed**, 95.6 % of trajectory epochs
fixed, median 1-sigma 0.4 cm horizontal and 0.5 cm vertical (RTKLIB's estimate), about two minutes of processing
from 295 MB of headers for a 22 GB flight. CGVD2013 heights match PROJ within 0.05 mm.
Writing into images changes only the recorded bytes and undoes to identical files.

**Not yet shown:** absolute accuracy against ground checkpoints and against another tool on
the same flights. See [docs/validation.md](docs/validation.md) for the details and what
remains.

## Quick start

```sh
# 1. Install (Rust 1.82+), or download a release binary
cargo install --path crates/azimut-cli

# 2. Try it on a synthetic flight
azimut demo demo/DJI_Demo --captures 96 --survey
azimut shift demo --enu 0.42,-0.18,0.65 --frame "NAD83(CSRS) v8, epoch 2010.0" --out demo/out

# 3. On real data: diagnose, then process against a base station
azimut scan /path/to/campaign --base /path/to/base.24o
azimut process /path/to/campaign \
    --base /path/to/base.24o --base-ppp /path/to/12345678.sum --antex igs20.atx \
    --geoid ca_nrc_CGG2013an83.tif --rtklib /path/to/rtklib/bin \
    --report-lang fr --out results
```

`results/` then holds, for each flight:

| File | Content |
|---|---|
| `<flight>_report.html` | Quality report: points to check, fix rate, uncertainties, camera map, base, findings, every capture. |
| `<flight>_positions.csv` | One row per image file: position, 1-sigma E/N/U, fix status, frame, CGVD2013 height. |
| `<flight>_pix4dmatic.csv` | PIX4Dmatic geolocation import, with `<flight>_pix4dmatic.txt` explaining the import. |
| `<flight>_geo.txt` | OpenDroneMap image positions, angles and accuracies. |
| `<flight>_problems.csv` | Captures without a position or an uncertainty, and why. |
| `<flight>_solution.json` | Everything, for programs; `azimut write-images` uses it. |

## Documentation

| Topic | Page |
|---|---|
| Installing Azimut, RTKLIB-EX and the reference files | [docs/install.md](docs/install.md) |
| What `scan` checks | [docs/scan.md](docs/scan.md) |
| PPK processing, base position from CSRS-PPP | [docs/ppk.md](docs/ppk.md) |
| RTK flights: correcting the base position | [docs/rtk-shift.md](docs/rtk-shift.md) |
| CGVD2013 heights | [docs/heights.md](docs/heights.md) |
| PIX4Dmatic | [docs/pix4dmatic.md](docs/pix4dmatic.md) |
| Writing positions into the images | [docs/images.md](docs/images.md) |
| Running Azimut from an agent (MCP) | [docs/mcp.md](docs/mcp.md) |
| Validation on real data | [docs/validation.md](docs/validation.md) |
| Architecture and design rules | [docs/architecture.md](docs/architecture.md) |
| Public sources of every format rule | [SOURCES.md](SOURCES.md) |
| Changes | [CHANGELOG.md](CHANGELOG.md) |

## Roadmap

- [x] Header-only diagnosis of DJI folders, Mavic 3M grouping, time pairing
- [x] PPK with RTKLIB-EX, per-file exposure times, MRK lever arm
- [x] CSRS-PPP base positions, RTK shift
- [x] CGVD2013 heights, PIX4Dmatic / Metashape / ODM outputs, quality report (EN/FR)
- [x] Writing into images with undo, MCP server for agents
- [ ] Validation against ground checkpoints and an established tool
- [ ] ITRF ↔ NAD83(CSRS) transformation with epoch; projected coordinates (MTM, UTM)
- [ ] More drones checked on real data (Mavic 3E, Matrice 4, Matrice 400)
- [ ] Desktop application

## Citing

If you use Azimut in research, please cite it with the metadata in
[CITATION.cff](CITATION.cff) (GitHub's "Cite this repository" button), and cite RTKLIB-EX for
the GNSS engine.

## Contributing

Issues and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). In short: every
format rule must cite a public source, no code, output, screenshot or text from closed-source
tools may enter the repository, and real survey data are committed only with their owner's
agreement.

## Acknowledgements

Azimut builds on [RTKLIB-EX](https://github.com/rtklibexplorer/RTKLIB) (T. Takasu and
contributors), the [PROJ](https://proj.org) geoid grids converted from Natural Resources
Canada's CGG2013a, the [IGS](https://igs.org) antenna models and station data, and the
[Nevada Geodetic Laboratory](https://geodesy.unr.edu) station positions.

## License

BSD 2-Clause, compatible with RTKLIB-EX. See [LICENSE](LICENSE).
