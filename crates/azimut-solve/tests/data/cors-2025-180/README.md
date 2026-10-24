# NGS CORS MILS and MISR, 29 June 2025, 15:00 to 16:00 GPS time

Fixture for `tests/ppk_accuracy.rs`: two continuously operating reference stations of the
U.S. National Geodetic Survey (NGS) network, 4.8 km apart near Sault Ste. Marie, Michigan,
both with the same antenna model (`LEIAR20         LEIM`, Leica GR50 receivers).

| File | Content |
|---|---|
| `mils1800.25o.gz` | MILS observations (base), RINEX 2.11, 30 s, GPS + GLONASS, L1/L2/L5 |
| `misr1800.25o.gz` | MISR observations (rover), same format |
| `brdc1800.25n.gz` | GPS broadcast ephemerides merged by NGS, records from 13:00 to 17:00 |
| `brdc1800.25g.gz` | GLONASS broadcast ephemerides merged by NGS, records from 13:00 to 17:00 |

Source: the NOAA CORS Network open data on AWS, `s3://noaa-cors-pds/rinex/2025/180/`
(<https://noaa-cors-pds.s3.amazonaws.com/rinex/2025/180/mils/mils1800.25o.gz>, likewise for
`misr` and `brdc1800.25n.gz`, `brdc1800.25g.gz`). NOAA data are public domain in the United
States and distributed without restriction under the NOAA Open Data Dissemination program.

The daily files were cut to one hour (120 epochs) by keeping whole epoch records, and the
`TIME OF FIRST OBS` / `TIME OF LAST OBS` header lines rewritten; a `COMMENT` line says so.
No observation was changed. The navigation files keep the records whose reference time is
within two hours before the window and one hour after it.

The reference coordinates in the test are the published ITRF2020 positions (epoch 2020.0)
and velocities of each station's antenna reference point (ARP), from the NGS coordinate
files `s3://noaa-cors-pds/coord/coord_20/mils_20.coord.txt` (computed April 2025 from data
through GPS week 2237) and `misr_20.coord.txt` (computed June 2025; velocity predicted with
HTDP 3.2.7).
