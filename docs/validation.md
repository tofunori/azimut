# Validation

What has been checked on real data, what remains to be shown, and which drones are covered.
The real data belong to the maintainer and are not in the repository; the measurements are
cited in [SOURCES.md](../SOURCES.md).

## Status at a glance

| Question | Answer so far |
|---|---|
| Does it run end to end on a real flight? | **Yes.** A full Mavic 3M flight: 450 captures, 2248 files, PPK against an IGS station. |
| Are the results internally consistent? | **Yes.** All 450 captures fixed; the offset to the drone's own single-point positions is nearly constant over the flight. |
| Are the heights right? | **Yes for the geoid step.** CGVD2013 undulations match PROJ within 0.05 mm. |
| Are images left intact when positions are written? | **Yes.** Only the recorded position bytes change; undo gives byte-identical files. |
| How accurate are the positions in absolute terms? | **For the PPK step, 2 to 5 cm on reference stations**, with a bias that grows with base distance; the camera step still needs ground checkpoints. See [PPK accuracy on reference stations](#ppk-accuracy-on-reference-stations). |

## Full flight: Mavic 3M, August 2024

Flight `DJI_202408090833_005`: firmware 11.08.01.41, 450 captures (1 RGB JPEG and 4 band
TIFFs each), 454 MRK records, no RTK (all 454 real-time positions are single-point). Base:
IGS station KUJ2 (Septentrio PolarX5, antenna `AOAD/M_T_RFI_T NONE` from `igs20.atx`),
7.6 km away, logging every 30 s, held at its daily IGS20 position from the Nevada Geodetic
Laboratory.

| Result | Value |
|---|---|
| Trajectory epochs fixed | 95.6 % of 9278 (5 Hz) |
| Captures fixed | 450 of 450 |
| Files positioned at their own exposure time | 2248 of 2248 |
| 1-sigma uncertainty, median (E / N / U) | 0.23 / 0.33 / 0.48 cm |
| 1-sigma uncertainty, 95th percentile (E / N / U) | 0.36 / 0.47 / 0.71 cm |
| Offset to the MRK single-point positions (N / E / U), median and spread | +0.97 m ± 0.08, +0.53 m ± 0.08, −2.76 m ± 0.16 |
| Processing time | about 2 minutes, from image headers only (295 MB read for a 22 GB flight) |

The uncertainties are RTKLIB's own estimates, not a comparison with an independent reference.
The steady offset to the single-point positions is what a single-point error looks like: a
nearly constant bias over a 30-minute flight.

**The 30 s base shows in the uncertainty.** The 1-sigma Up doubles between base epochs: 0.32 cm
within 3 s of a base epoch, 0.68 cm 12 to 15 s away. RTKLIB interpolates the base between
its epochs; a base logging every second removes this pattern.

## PPK accuracy on reference stations

`crates/azimut-solve/tests/ppk_accuracy.rs`, run in CI (job `PPK accuracy (RTKLIB-EX)`, RTKLIB-EX
`50886e2`). A U.S. NGS CORS station is processed as a moving rover, one position per second,
against another station, with the options file Azimut writes for DJI aircraft. Each position
is compared with the rover's published ITRF2020 coordinates, which NGS computed from years of
data with its own software; they do not depend on RTKLIB. Comparing Azimut with `rnx2rtkp` on
the same data would compare it with itself, since Azimut runs `rnx2rtkp`. Both stations of a
pair carry the same antenna model (`LEIAR20 LEIM`), so phase-centre offsets cancel. Data:
29 June 2025, 15:00 to 15:15 GPS time, 1 s, GPS + GLONASS, L1/L2 (public NOAA data, extract in
`crates/azimut-solve/tests/data/cors-2025-180`).

Error of the fixed positions against the published coordinates (cm):

| Baseline | Options | Fixed | Mean E / N / U | Scatter (std) E / N / U | RMS H / U | RTKLIB 1-sigma (median) E / N / U |
|---|---|---|---|---|---|---|
| MISR from MILS, 4.8 km | DJI defaults | 900 of 900 | −2.2 / +1.0 / +2.4 | 0.9 / 0.4 / 0.7 | 2.6 / 2.5 | 0.2 / 0.3 / 0.6 |
| MISR from MILS, 4.8 km | ionosphere estimated | 766 of 900 | −1.2 / +0.9 / +0.6 | 0.5 / 0.5 / 0.9 | 1.7 / 1.1 | 0.4 / 0.5 / 1.0 |
| NOR3 from MIAL, 11.0 km | DJI defaults | 900 of 900 | +0.6 / −2.1 / +5.2 | 0.8 / 0.7 / 1.4 | 2.4 / 5.4 | 0.2 / 0.3 / 0.6 |
| NOR3 from MIAL, 11.0 km | ionosphere estimated | 900 of 900 | −0.1 / −0.2 / +4.1 | 0.3 / 0.5 / 0.6 | 0.6 / 4.1 | 0.3 / 0.4 / 0.9 |

What this shows:

- **Ambiguities fix reliably at 1 s**: every epoch fixed with the DJI defaults on both
  baselines. At 30 s (the same stations' daily files, one hour) only 39 of 120 epochs fixed,
  so the test uses 1 s data, closer to the drone's 5 Hz.
- **The error is a bias, not noise**: a nearly constant 2 to 5 cm offset over the 15
  minutes, with only 0.4 to 1.4 cm of scatter around it. It is larger than the typical RTK
  specification (8 mm + 1 ppm horizontal, 15 mm + 1 ppm vertical, 1-sigma: 1.3 / 2.0 cm at
  4.8 km, 1.9 / 2.6 cm at 11 km).
- **Most of the horizontal bias is the ionosphere**. The DJI defaults use the broadcast
  ionosphere model only (`pos1-ionoopt=brdc`); June 2025 is near the solar maximum. Estimating
  the ionosphere (`est-stec`) brings the horizontal bias from 2.2 to 0.2 cm on the 11 km
  baseline and from 2.4 to 1.5 cm on the 4.8 km one, at the cost of 15 % of the fixes on the
  shorter one. The iono-free combination (`dual-freq`) fixed 11 % and 0 % of the epochs.
- **A vertical bias of 4 cm remains on NOR3** with the ionosphere estimated. Its cause is not
  established: residual troposphere (NOR3 is 30 m above MIAL), the stations' published
  coordinates, or individual antenna differences.
- **RTKLIB's 1-sigma is too small**: 0.2 to 0.6 cm against actual errors of 2 to 5 cm, so the
  accuracies Azimut writes into its outputs (and that Metashape, Pix4D and ODM use as weights)
  are 4 to 10 times too optimistic on these baselines.
- MISR's published velocity is predicted (HTDP), not computed from its data, which makes its
  reference weaker than NOR3's.

Limits: one day, two 15-minute windows, static antennas on good sites, same antenna model at
both ends. A drone adds motion, multipath from the airframe, a different antenna and the
camera step (interpolation, lever arm), none of which this test covers.

The test fails when the fix rate drops below 90 % or the bias, RMS or 95th percentile exceed
the values above by about 1 cm: it guards against a setup error or a regression, and prints the
comparison with the RTK specification without asserting it.

## Metadata facts measured on this flight

- `UTCAtExposure` holds GPS time: it matches the MRK GPS time within 1 µs. The camera
  clock (file names, `ModifyDate`) is therefore 18 s ahead of UTC.
- The MRK times the RGB JPEG. **The four band TIFFs are exposed 26.3 ± 0.3 ms later**, all
  at the same time. At 3.6 m/s that is 9.5 cm, at 10 m/s 26 cm, so Azimut positions every
  file at its own exposure time. The offset between the RGB lens and the band lenses is not
  documented and not applied.
- One XMP packet per JPEG and per TIFF; `CaptureUUID` shared by the five files, `BandName`
  `Green`, `Red`, `RedEdge`, `NIR`; MRK record number equals the file number.
- The MRK can hold more records than images (454 for 450 captures; the last capture has only
  the JPEG and Green): unpaired records and incomplete captures are reported.
- EXIF `GPSAltitude`, XMP `AbsoluteAltitude` and the MRK `Ellh` hold the same ellipsoidal
  height.

## Other checks on real files

- **Writing into images** (copies of 63 files): 2998 bytes changed in all, every one in a
  recorded position field; ExifTool reads the new EXIF and XMP positions without warning;
  the DJI XMP keeps its 98 attributes when read as OpenDroneMap does (`xmltodict`), with the
  new altitude and accuracies; `restore-images` gives files identical to the originals.
- **Agent (MCP)**: a PPK run through `azimut mcp` (86 s) sends progress notifications and
  returns the report summary; `write_images` stops at a dry run until confirmed.
- **CGVD2013**: the geoid undulations Azimut interpolates in both CGG2013a grids match PROJ
  9.8.1 within 0.05 mm at 12 points across Canada.

## Still to show

1. **Absolute accuracy of the camera positions**: ground checkpoints measured independently,
   compared with the camera positions or the photogrammetric products (ASPRS Positional
   Accuracy Standards, Ed. 2). The PPK step alone is measured above, on reference stations.
2. **The ionosphere option**: whether estimating the ionosphere (`pos1-ionoopt=est-stec`)
   should be the default, measured on drone flights with bases a few kilometres away, and
   how to report realistic uncertainties instead of RTKLIB's 1-sigma.
3. **Agreement with an established tool** on the same flights (kept outside the repository,
   as the contribution rules require).
4. **A PIX4Dmatic import** of the geolocation files on a real project.
5. **The base coordinates of KUJ2**: they are taken as the ground mark; if they referred to
   the antenna, every height would be 5 cm too high.
6. **Other drones and firmware**: the same measurements on the Mavic 3E, Matrice 4 and
   Matrice 400 (`*_D.MRK` naming), and their position fields and XMP padding before
   writing into their images (Azimut skips any file without them).
7. The meaning of the `*_F.JPG` file found once in a Mavic 3M flight (2592 × 1944, no exposure
   time, same `CaptureUUID` as the first capture).

## Supported hardware

The MRK layout is the same in the Matrice 4, Matrice 400, Mavic 3E/3M and Zenmuse P1 manuals.

| Platform | Status |
|---|---|
| Mavic 3 Multispectral | **Checked on a real flight** (firmware 11.08.01.41) |
| Mavic 3 Enterprise | Implemented (same files as 3M, no bands), needs real data |
| Matrice 4T / Matrice 400 (`*.RTK`, `*_D.MRK`) | File types recognised, needs real data |
| Matrice 300 RTK + Zenmuse P1 | Implemented, needs real data |
| Matrice 300 RTK + H20 series | No PPK files documented by DJI |

---
[Back to the README](../README.md)
