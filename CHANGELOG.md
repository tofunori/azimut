# Changelog

All notable changes. Versions follow [Semantic Versioning](https://semver.org).

## [Unreleased]

### Added
- The CSRS-PPP e-mail is asked in the terminal when it is missing, and remembered in Azimut's
  settings (`config.json`, folder overridable with `AZIMUT_CONFIG_DIR`).
- MCP tool `submit_ppp`: sends a base RINEX to CSRS-PPP after the user's confirmation, asks
  for the account e-mail once, and returns the summary for `process_ppk` (`resume` for a
  submission that outlasts the wait).
- The igs20.atx models of the Emlid Reach RS2, RS2+ and RS3 are embedded: a base on one of
  these antennas positioned by CSRS-PPP no longer needs `--antex`.
- Other base antennas get their model from igs20.atx, downloaded from files.igs.org into
  Azimut's cache on first use (folder overridable with `AZIMUT_CACHE_DIR`) and reused
  offline: `--antex` is no longer needed. `azimut antex` fetches the file ahead of an offline
  session and checks an antenna. `--antenna-type` uses the same models.

### Changed
- `process` refuses to run without a base position source: `--base-llh`, `--base-ppp` or
  `--submit-ppp`. The approximate RINEX header position, used silently before, now needs
  `--approximate-base`.

### Fixed
- Mavic 3M captures that also hold the multispectral camera's JPEG (`_F.JPG`) were reported
  incomplete (2 JPEG) and that file was placed at the MRK time. It now has its own role
  (`ms_jpeg`) and is positioned at the bands' exposure time.

## [0.2.0] - 2026-10-24

### Added
- CSRS-PPP submission of the base RINEX from Azimut (`ppp`, `process --submit-ppp`): waits
  for the result, keeps the archive, resumes with the tracking key (`ppp --resume`), and
  prints the service's warnings (`errors.txt`) under the position.
- Project website on GitHub Pages (French and English), with the example quality reports
  viewable online (`site/`, `scripts/build-site.sh`).

### Fixed
- The ambiguity fixing rate of CSRS-PPP 5.15 summaries (per-constellation `IAR` lines, then
  `IAR ALL`) was reported as missing.
- Release builds: the RTKLIB-EX commit is quoted so YAML keeps it a string, and MinGW
  builds define `WIN32`.

## [0.1.0] - 2026-10-23

### Added
- Header-only diagnosis of DJI flight folders (`scan`): Mavic 3M capture grouping, MRK and
  photo pairing on time, base-station coverage, interval and distance.
- PPK with RTKLIB-EX (`process`): combined solution with full covariance, antenna
  interpolated to each file's own exposure time, MRK lever arm.
- Base position from an NRCan CSRS-PPP summary (`--base-ppp`); RTK flights corrected by a
  base shift (`shift`, `--to-ppp`).
- CGVD2013 orthometric heights from PROJ geoid grids (`--geoid`), with a frame check.
- Outputs: positions CSV, OpenDroneMap `geo.txt`, PIX4Dmatic geolocation file, problems list.
- Quality report in HTML and JSON, in English or French (`--report-lang fr`).
- Positions written into the images, in place with undo (`restore-images`) or into copies
  (`--write-images`, `write-images`).
- MCP server for agents (`azimut mcp`).
- Synthetic DJI flights for tests and demonstrations (`demo`, `demo --survey`).
- Release archives for Linux, macOS and Windows with RTKLIB-EX included, found next to
  `azimut`; one-line install scripts; a French lab guide; example outputs.

### Checked on real data
- A full Mavic 3M flight (450 captures) processed against an IGS station; see
  [docs/validation.md](docs/validation.md).
