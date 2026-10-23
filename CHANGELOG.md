# Changelog

All notable changes. Versions follow [Semantic Versioning](https://semver.org).

## [Unreleased]

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

### Checked on real data
- A full Mavic 3M flight (450 captures) processed against an IGS station; see
  [docs/validation.md](docs/validation.md).
