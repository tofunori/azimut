# NGS CORS stations in Michigan, 29 June 2025, 15:00 to 15:15 GPS time

Fixture for `tests/ppk_accuracy.rs`: two pairs of continuously operating reference stations
of the U.S. National Geodetic Survey (NGS) network, all with the same antenna model
(`LEIAR20         LEIM`) and Leica GR50 receivers: MIAL and NOR3, 11.0 km apart near
Alpena, and MILS and MISR, 4.8 km apart near Sault Ste. Marie.

| File | Content |
|---|---|
| `mial180p.25o.gz` | MIAL observations (base), RINEX 2.11, 1 s, GPS + GLONASS, L1/L2/L5 |
| `nor3180p.25o.gz` | NOR3 observations (rover), same format |
| `mils180p.25o.gz` | MILS observations (base), same format |
| `misr180p.25o.gz` | MISR observations (rover), same format |
| `brdc1800.25n.gz` | GPS broadcast ephemerides merged by NGS, records from 13:00 to 17:00 |
| `brdc1800.25g.gz` | GLONASS broadcast ephemerides merged by NGS, records from 13:00 to 17:00 |

Source: the NOAA CORS Network open data on AWS, `s3://noaa-cors-pds/rinex/2025/180/`
for the navigation files and `s3://noaa-cors-pds/raw/2025/180/` for the hourly 1 s
observation files (<https://noaa-cors-pds.s3.amazonaws.com/raw/2025/180/mils/mils180p.25d.gz>,
likewise for the other stations). NOAA data are public domain in the United
States and distributed without restriction under the NOAA Open Data Dissemination program.

The hourly observation files were expanded from Hatanaka compression (RNXCMP through the
`hatanaka` Python package), cut to 15 minutes (900 epochs) by keeping whole epoch records, and the
`TIME OF FIRST OBS` / `TIME OF LAST OBS` header lines rewritten; a `COMMENT` line says so.
No observation was changed. The navigation files keep the records whose reference time is
within two hours before the window and one hour after it.

The reference coordinates in the test are the published ITRF2020 positions (epoch 2020.0)
and velocities of each station's antenna reference point (ARP), from the NGS coordinate
files `s3://noaa-cors-pds/coord/coord_20/<station>_20.coord.txt`, all computed with data
through GPS week 2237 (November 2022). The velocity of MISR, installed in March 2021, is
predicted with HTDP 3.2.7 rather than computed from its data; the others are computed.
