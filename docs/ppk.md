# PPK processing

How `azimut process` turns a flight's raw GNSS log and a base station into camera positions.

Azimut drives [RTKLIB-EX](https://github.com/rtklibexplorer/RTKLIB) (BSD-2), which it does
not ship. Build its console programs `convbin` and `rnx2rtkp` (for example with `make` in
`app/consapp/convbin/gcc` and `app/consapp/rnx2rtkp/gcc`, or use the Windows release
binaries), then point Azimut at them. The `rnx2rtkp` makefile links `libgfortran`, which it
does not need without LAPACK: if it is not installed, build with `make LDLIBS=-lm`.

```sh
azimut process /path/to/campaign \
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
| `<flight>_pix4dmatic.csv` | PIX4Dmatic image geolocation file: latitude, longitude, ellipsoidal height and 1-sigma horizontal and vertical accuracy per file, no angles (see [PIX4Dmatic](pix4dmatic.md)). `<flight>_pix4dmatic.txt` says how to import it. |
| `<flight>_problems.csv` | Exposures without a position, or without an uncertainty, and why. |
| `<flight>_solution.json` | Everything above plus the method (PPK file paths, or the RTK shift), statistics and file paths. |
| `<flight>_report.html`, `_report.json` | Quality report: points to check, captures fixed, trajectory fix rate, horizontal and vertical uncertainty (median and 95th percentile), base, a map of the cameras (status by colour and shape, flight order, scale, north, direction of the base), the 1-sigma E/N/U of each capture, scan findings, the capture table and the files written. A single HTML file without scripts, light and dark, printable; `--report-lang fr` writes it in French. |

When RTKLIB writes a covariance that is not positive definite (it prints 0 for a negative
variance, seen on kinematic solutions), the uncertainty columns are left empty rather than
written as 0 m, which photogrammetry software would read as a perfect position. The position
is kept and the exposure is listed in `problems.csv`.

## Base position from CSRS-PPP

Submit the base RINEX to [CSRS-PPP](https://webapp.csrs-scrs.nrcan-rncan.gc.ca/geod/tools-outils/ppp.php)
(NRCan), then give its summary file instead of `--base-llh`:

```sh
azimut process /path/to/campaign --rtklib /path/to/rtklib/bin \
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
orthometric heights (see [CGVD2013 heights](heights.md)).

Defaults for DJI aircraft: L1+L2, GPS + GLONASS + Galileo + BeiDou, 15° mask, continuous
ambiguity resolution with ratio 3, GLONASS ambiguities left float (base and drone receivers
come from different makers), base observations interpolated to the 5 Hz drone epochs.
`--no-beidou` drops BeiDou for logs with reported decoding problems (Matrice 4).

---
[Back to the README](../README.md)
