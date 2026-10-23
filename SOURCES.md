# Sources

Every format rule and design choice in Azimut is tied to a public source. Add a line here
with any new rule. Do not use closed-source code, decompiled binaries, or outputs, screenshots
or interface text of closed-source tools as a source.

## MRK exposure log

| Rule | Where in the code | Source |
|---|---|---|
| 13 fields: index, GPS TOW, [GPS week], N/E/V offsets (mm), lat, lon, ellipsoidal height, 3 std devs (m), status | `mrk.rs` | [Matrice 4 Series User Manual](https://dl.djicdn.com/downloads/DJI_Matrice_4_Series/DJI_Matrice_4_Series_User_Manual_en.pdf); [Matrice 400 User Manual v1.02](https://dl.djicdn.com/downloads/Matrice_400/UM/20250908/Matrice_400_User_Manual_v1.02_en.pdf); [Mavic 3 Enterprise Series User Manual](https://dl.djicdn.com/downloads/DJI_Mavic_3_Enterprise/20250916/DJI_Mavic_3_Enterprise_Series_User_Manual_EN.pdf); [Zenmuse P1 User Manual v1.4](https://dl.djicdn.com/downloads/Zenmuse_P1/20230530/Zenmuse_P1%20_User%20Manual_v1.4_EN.pdf) |
| Offsets from antenna phase centre to CMOS centre, V positive downward | `mrk.rs` | [DJI Enterprise Geospatial FAQ](https://enterprise-insights.dji.com/blog/geospatial-solutions-faq) |
| Values carry suffixes (`,N`, `,Lat`, `,Q`...); latitude comes before longitude in real files | `mrk.rs` (read by suffix) | [Agisoft forum, P4RTK MRK](https://www.agisoft.com/forum/index.php?topic=14074.0); [OpenDroneMap forum](https://community.opendronemap.org/t/dji-ppk-file-usage/16603) |
| Lowercase `lat`/`lon` tags and other separators exist | `mrk.rs` | [Skydio X10 RTK/PPK metadata overview](https://support.skydio.com/hc/en-us/articles/32887502774171-Skydio-X10-RTK-PPK-metadata-overview) |
| Status: 50 fixed, 32 to 49 float, 16 single, 0 none | `mrk.rs` `RtkQuality` | [Mavic 3M Image Processing Guide v1.0](https://dl.djicdn.com/downloads/DJI_Mavic_3_Enterprise/20230829/Mavic_3M_Image_Processing_Guide_EN.pdf); Matrice 4 manual (RtkFlag) |

## Image metadata and time

| Rule | Where | Source |
|---|---|---|
| XMP fields `UTCAtExposure` (microsecond precision, `YYYY:MM:DD HH:MM:SS.ffffff`) and `CaptureUUID` | `photo.rs`, `time.rs` | [Mavic 3M User Manual](https://dl.djicdn.com/downloads/DJI_Mavic_3_Enterprise/20240814/DJI_Mavic_3M_User_Manual_EN.pdf); Matrice 4 manual |
| `UTCAtExposure` measured as GPS time (18 s ahead of UTC) on P1 and L2 data, despite its label; pair on time, not order | `matching.rs` (tries both readings) | [dji-geotagger output.md](https://github.com/geo-raypan/dji-geotagger/blob/main/docs/output.md); [dji-geotagger commits](https://github.com/geo-raypan/dji-geotagger/commits/main) |
| `UTCAtExposure` measured as GPS time on Mavic 3M too: equal to the MRK GPS time within 1 µs | `matching.rs` | Own measurement: Mavic 3M, firmware 11.08.01.41, August 2024 flight, 13 captures (maintainer's data, not committed), read independently with ExifTool 12.76 |
| GPS - UTC = 18 s since 2017-01-01 | `time.rs` | IERS Bulletin C (no leap second since 2017) |
| XMP in JPEG: APP1 segment starting `http://ns.adobe.com/xap/1.0/\0` | `headers.rs` | Adobe XMP Specification Part 3 |
| XMP in TIFF: tag 700 (BYTE/UNDEFINED) | `headers.rs` | Adobe XMP Specification Part 3; TIFF 6.0 |

## Mavic 3 Multispectral

| Rule | Where | Source |
|---|---|---|
| One trigger writes 1 RGB JPEG + 4 TIFF bands (Green, Red, RedEdge, NIR), sharing `CaptureUUID` | `capture.rs` | [Mavic 3M Image Processing Guide](https://dl.djicdn.com/downloads/DJI_Mavic_3_Enterprise/20230829/Mavic_3M_Image_Processing_Guide_EN.pdf); [Pix4D support](https://support.pix4d.com/hc/en-us/articles/36000014121991) |
| Band from XMP `BandName`, value `RedEdge` without a space; file names vary (`_MS_G` or `_G`) | `photo.rs` | [m3m_radcal SPEC.md](https://github.com/Eggpl4ntE4ter/m3m_radcal/blob/main/docs/SPEC.md); [uav_pytools](https://github.com/Jeff-Tracey/uav_pytools) |
| Group on `CaptureUUID`, fall back on file names | `capture.rs` | [OpenDroneMap photo.py](https://raw.githubusercontent.com/OpenDroneMap/ODM/master/opendm/photo.py) |
| MRK has one line per RGB photo (captures), not per file | `matching.rs` | [Emlid community](https://community.emlid.com/t/ppk-processing-of-multispectral-drone-data/39286); [DJI Terra accuracy notes](https://terra.dji.com/user-manual/ko/accuracy-whitepaper/photogrammetry-accuracy.html) |
| The MRK times the RGB JPEG; the four band TIFFs are exposed 26.3 ± 0.3 ms later, together (spread under 0.5 ms), so each file is positioned at its own `UTCAtExposure` | `pipeline.rs` (`time_offset_s`, `place_files`), `rtk_shift.rs` | Own measurement, same flight as above: 13 captures, JPEG `UTCAtExposure` equal to the MRK time within 1 µs |
| One XMP packet per JPEG and per TIFF; MRK record number equals the file number | `headers.rs`, `matching.rs` | Own measurement, same flight |
| Radiometric XMP tags must survive any edit; Pillow/ImageMagick/GDAL rewrite or drop metadata | design rule (read-only by default; `--write-images` changes only position values in place) | Mavic 3M Image Processing Guide; [ImageMagick #1674](https://github.com/ImageMagick/ImageMagick/issues/1674) |

## Folder layout and pitfalls

| Rule | Where | Source |
|---|---|---|
| Raw GNSS log is RTCM 3.2: `*_PPKRAW.bin` (Mavic 3E/3M, P1), `*.RTK` (Matrice 4, M400) | `scan.rs` `aux_kind` | Manuals above; [MatricePilots](https://matricepilots.com/threads/what-are-these-file-types.25895) |
| New folder every 999 photos; GNSS log closes a few seconds early | `scan.rs` (`folder_split`) | [dji-geotagger pipeline.md](https://github.com/geo-raypan/dji-geotagger/blob/main/docs/pipeline.md) |
| Folders that do not start at 0001 break order-based pairing | `scan.rs`, `matching.rs` | [dji-geotagger releases](https://github.com/geo-raypan/dji-geotagger/releases) |
| On-board RINEX seen with 1980 dates or missing constellation letters | `scan.rs` (`no_raw_log`) | [Emlid community](https://community.emlid.com/t/ppk-processing-failed/35293) |

## RINEX

| Rule | Where | Source |
|---|---|---|
| Header labels in columns 61-80; version/type, antenna, approximate position, interval, time of first/last obs | `rinex.rs` | [RINEX 3.04](https://files.igs.org/pub/data/format/rinex304.pdf); [RINEX 2.11](https://files.igs.org/pub/data/format/rinex211.txt); [RINEX 4.01](https://files.igs.org/pub/data/format/rinex_4.01.pdf) |
| RINEX 3 epoch lines start with `>`; RINEX 2 epoch lines use two-digit years (80-99 = 19xx) | `rinex.rs` | Same |
| Observation code second character is the frequency band (`C1C`, `L2W`, `L5Q`) | `rinex.rs` (`bands`) | RINEX 3.04, section 5.1 |
| Compact RINEX (Hatanaka) keeps a readable RINEX header after two `CRINEX` lines | `rinex.rs` | [Hatanaka 2008, Compact RINEX format](https://terras.gsi.go.jp/ja/crx2rnx/RINEX_HATANAKA.pdf) |
| File names: `.yyo`/`.yyd`, `_MO.rnx`/`.crx`, `.yyn`/`_MN.rnx` | `rinex.rs` (`is_obs_name`, `is_nav_name`) | RINEX 3.04, section 4 (file naming) |
| A 30 s base is too coarse for a 5 Hz drone log | `scan.rs` (`base_interval`) | [Emlid community](https://community.emlid.com/t/ppk-processing-failed/35293) |
| DJI accuracy specification adds 1 ppm of the baseline | `scan.rs` (`base_far`) | [DJI M350 RTK specs](https://enterprise.dji.com/matrice-350-rtk/specs); [Taddia et al. 2020](https://www.mdpi.com/2504-446X/4/2/9) |
| Record the base before take-off and after landing | `scan.rs` (`base_short_margin`) | [REDcatch support: floats](https://redcatch.freshdesk.com/support/solutions/articles/103000382548-what-to-do-if-you-receive-many-floats) (public support article) |

## RTKLIB-EX (PPK engine)

Read from the RTKLIB-EX source code (BSD-2), [github.com/rtklibexplorer/RTKLIB](https://github.com/rtklibexplorer/RTKLIB).

| Rule | Where | Source |
|---|---|---|
| Options file keys and value names (`pos1-*`, `pos2-*`, `out-*`, `ant2-*`, `misc-timeinterp`) | `config.rs` | `src/options.c` |
| `ant2-anttype=*` takes antenna type and offsets from the RINEX header | `config.rs` | `src/postpos.c` |
| ECEF solution line: x y z Q ns sdx sdy sdz sdxy sdyz sdzx age ratio; covariances written as signed square roots | `pos.rs` | `src/solution.c` (`outecef`, `sqvar`) |
| A negative variance is written as `sdx = 0` (macro `SQRT`), so a covariance that is not positive definite cannot be told apart from a zero one; such covariances were measured in kinematic mode on RTKLIB-EX's own test data (`test/data/rinex`, stations 0759 and 3040, 2005-04-02: 102 of 115 epochs with stock options, none in static mode). The uncertainty is then left empty | `camera.rs`, `azimut-export` | `src/solution.c` (`SQRT`, `outecef`); own measurement with RTKLIB-EX `50886e2` |
| Solution status codes 1 fix, 2 float, 3 SBAS, 4 DGPS, 5 single, 6 PPP, 7 dead reckoning | `pos.rs` | `src/rtklib.h` (`SOLQ_*`) |
| `rnx2rtkp -k conf -o out rover base nav...` | `rtklib.rs` | `app/consapp/rnx2rtkp/rnx2rtkp.c` |
| `convbin -r rtcm3 -tr y/m/d h:m:s -v 3.04 -od -os -o obs -n nav` (RTCM 3 needs an approximate date) | `rtklib.rs` | `app/consapp/convbin/convbin.c` |

## CSRS-PPP

| Rule | Where | Source |
|---|---|---|
| Summary lines start with a keyword; `POS LAT/LON/HGT <system> <yy:ddd:sssss> <a priori> <estimated> <diff> <sigma 95 %> <correlations>`, angles in degrees, minutes, seconds; `ANT` (`NOT FOUND` when absent), `ARP` offsets, `IAR`, `OHT`, `GHT`, `VER`, `RNX`, `MKR`, `MOD` | `azimut-io` `csrs_ppp.rs` | Own measurement: summary files of versions 3.54.2 (2022) and 4.14.11 (2024) for the maintainer's base stations (not committed); latitude, longitude and height convert to the summary's X, Y, Z within 0.1 mm on GRS80, and height minus `GHT` equals `OHT` |
| Version 5.15 writes `IAR` once per constellation (`IAR GPS 89.30%`, `IAR GAL OFF`) and then the total as `IAR ALL 89.30%`; version 4 writes a single `IAR 98.71%` | `csrs_ppp.rs` | Own measurement: first submission from `azimut ppp`, 2026-10-23, version 5.15 (Emlid Reach RS2 base, not committed) |
| Summary files hold the position in more than one system; the `.pos` file only in the one selected | `csrs_ppp.rs` (`--ppp-system`) | [Krcmar, CSRS-PPP: a web tool](https://krcmar.ca/resource-articles/2010_Winter_CSRS-PPP%20-%20A%20web%20tool_0.pdf) |
| CSRS-PPP estimates the antenna phase centre and reduces it to the mark with the antenna calibration and the RINEX `ANTENNA: DELTA H/E/N` offsets, which the user must check | `config.rs` (`BaseSetup::from_ppp`) | [NRCan, Guidelines for RTK/RTN GNSS Surveying in Canada](https://natural-resources.canada.ca/sites/www.nrcan.gc.ca/files/earthsciences/pdf/Canada%20RTK_UserGuide_v1_2-EN.pdf); [Emlid forum, no antenna type in RINEX](https://community.emlid.com/t/rs2-ppp-problem-no-antenna-type-in-rinex-header/21106) |

## CSRS-PPP service

| Rule | Where | Source |
|---|---|---|
| `POST <service>/submit`, multipart form with `return_email`, `user_name`, `cmd_process_type=std`, `ppp_access=nobrowser_status`, `language`, `process_type` (`Static`, `Kinematic`), `sysref` (`ITRF`, `NAD83`), `nad83_epoch=CURR`, `v_datum=cgvd2013`, `output_pdf=lite` and the RINEX in `rfile_upload`, user agent `Mozilla/5.0 (Python CSRS-PPP access)`; the reply is the tracking key as plain text, anything else is a refusal | `azimut-csrs` (`form_fields`, `tracking_key`) | NRCan's `csrs_ppp_auto.py`, distributed from the [CSRS-PPP page](https://webapp.csrs-scrs.nrcan-rncan.gc.ca/geod/tools-outils/ppp.php); own measurement, 2026-10-23 (a base RINEX submitted and its results retrieved) |
| `GET <service>/results/status?id=<key>` answers `processing` or `done` as plain text; an unknown key gets `error` | `azimut-csrs` (`parse_status`) | Same; `error` measured with an invalid key on 2026-10-23 |
| `GET <service>/results/file?id=<key>` returns a zip with, for `X.24O`, `X.sum`, `X.pos`, `X.csv`, `X.pdf`, `X.clk`, `X.tro`, `errors.txt`, `output_descriptions.txt`; a 4 to 8 h session at 30 s is done in about a minute | `azimut-csrs` (`extract`) | Own measurement, 2026-10-23 |

## RTK shift

| Rule | Where | Source |
|---|---|---|
| MRK latitude, longitude and height are the camera (CMOS) centre in real time, antenna position plus lever arm, with ellipsoidal heights | `rtk_shift.rs` (no lever arm added) | [DJI Phantom 4 RTK white paper](https://shop.quadrocopter.com/assets/images/P4RwhitePaperV2.pdf); [DJI Enterprise Geospatial FAQ](https://enterprise-insights.dji.com/blog/geospatial-solutions-faq) |
| An RTK rover inherits the offset between the broadcast base position and the true one, so every position moves by that difference; relative precision is unaffected | `rtk_shift.rs` | [Emlid docs, Placing the base](https://docs.emlid.com/reach/common/tutorials/placing-the-base) |
| Only RTK (fixed, float) positions use the base; single-point positions do not and are not shifted | `rtk_shift.rs` | Same; status codes as in the MRK section |
| Bulk XYZ shift of RTK positions is an established correction for drone imagery | `rtk_shift.rs` | [REDtoolbox product page](https://gnss.ae/software/redtoolbox/) (feature description only) |

## Camera centre

| Rule | Where | Source |
|---|---|---|
| Interpolate the antenna to the exposure time, then add the lever arm rotated from NED to ECEF at that position; no extrapolation | `camera.rs` | [dji-geotagger pipeline.md](https://github.com/geo-raypan/dji-geotagger/blob/main/docs/pipeline.md) |
| Check: camera height = antenna height minus the Down offset | `camera.rs` (`height_check_m`) | Same |
| GRS80 ellipsoid (ITRF, NAD83) | `geodesy.rs` | Moritz (1980), Geodetic Reference System 1980 |

## Geoid grids (CGVD2013)

| Rule | Where | Source |
|---|---|---|
| PROJ GeoTIFF grids: `TYPE=VERTICAL_OFFSET_GEOGRAPHIC_TO_VERTICAL` holds the geoid undulation N (`geoid_undulation`, positive upward from the ellipsoid), so H = h - N; values scaled by `SCALE`/`OFFSET` from the GDAL metadata; nodata in `GDAL_NODATA` (raw value); CDN grids are pixel-is-point, pixel-is-area shifts by half a cell; bilinear interpolation by default | `azimut-geo` `geoid.rs` | [PROJ, Geodetic TIFF grids (GTG) specification](https://proj.org/en/stable/specifications/geodetictiffgrids.html) |
| CGG2013a is published for NAD83(CSRS)v6 (`ca_nrc_CGG2013an83.tif`) and ITRF2008 (`ca_nrc_CGG2013ai08.tif`); each file names its frame in `ImageDescription` | `geoid.rs` (`check_frame`) | [cdn.proj.org](https://cdn.proj.org/), files converted by PROJ from NRCan's CGG2013a (Open Government Licence - Canada) |
| The two CGG2013a grids differ by 0.13 to 1.12 m at 12 points across Canada, so a grid used with positions in the other frame is refused | `geoid.rs` (`check_frame`) | Own measurement with both grids (Azimut and PROJ 9.8.1 agree within 0.05 mm at every point) |

## Writing positions into images

| Rule | Where | Source |
|---|---|---|
| EXIF GPS IFD (pointer tag 34853): `GPSLatitudeRef`/`GPSLongitudeRef` ASCII x 2, `GPSLatitude`/`GPSLongitude` RATIONAL x 3 (degrees, minutes, seconds), `GPSAltitudeRef` BYTE (1 = below the reference), `GPSAltitude` RATIONAL in metres | `azimut-io` `layout.rs`, `azimut-export` `images.rs` | [CIPA DC-008-2012, Exif 2.3, GPS attribute information](https://www.cipa.jp/std/documents/e/DC-008-2012_E.pdf) |
| XMP packets carry whitespace padding before `<?xpacket end` so they can be edited in place (2 to 4 kB recommended) | `images.rs` (`patch_xmp`) | [Adobe XMP Specification Part 3](https://archimedespalimpsest.net/Documents/External/XMP/XMPSpecificationPart3.pdf) |
| Mavic 3M XMP packets hold 2.8 to 5.9 kB of padding (three files measured); DJI writes `drone-dji:` fields as attributes; EXIF seconds to 1e-4, altitude to 1 mm | `images.rs` | Own measurement, Mavic 3M flight above (JPEG and TIFF) |
| DJI writes the ellipsoidal height in EXIF `GPSAltitude` and XMP `AbsoluteAltitude` (both equal the MRK `Ellh`), with `GPSAltitudeRef` 0 and `GPSMapDatum` `WGS-84`; the band TIFFs carry their own position | `images.rs` (heights written ellipsoidal) | Own measurement, same flight, read with ExifTool 12.76 |
| OpenDroneMap reads DJI XMP fields only as attributes (`@drone-dji:SensorGain`, `ExposureTime`, `CaptureUUID`, `FlightYawDegree`...), takes the altitude from `@drone-dji:AbsoluteAltitude` over EXIF and the accuracy from `RtkStdLon/Lat/Hgt` | `images.rs` (attribute form kept; `AbsoluteAltitude` and `RtkStd*` written) | [OpenDroneMap photo.py](https://raw.githubusercontent.com/OpenDroneMap/ODM/master/opendm/photo.py) (`get_xmp_tag`, `parse_exif_values`) |
| ExifTool 12.76, asked to write one DJI XMP field, rewrites every field of the packet as an element, which OpenDroneMap then no longer reads; it also drops empty attributes | `images.rs` (in-place writing instead of ExifTool) | Own measurement on copies of the Mavic 3M files |

## PIX4Dmatic and MCP

| Rule | Where | Source |
|---|---|---|
| PIX4Dmatic image geolocation import: `.csv`/`.txt`, comma-separated, no header (a `#` comment allowed on the first line); "geographic coordinates with accuracy values": image, coordinate 1, coordinate 2 (in the axis order of the chosen system, latitude first for geographic EPSG systems), z, accuracy horizontal, accuracy vertical (m); orientation columns optional | `azimut-export` (`pix4dmatic_csv`) | [PIX4Dmatic: How to import and export image geolocation and orientation files](https://support.pix4d.com/hc/en-us/articles/360056389172) |
| PIX4Dmatic project files (`.p4m`) have no public format description, so Azimut does not write them | design rule | [PIX4Dmatic outputs](https://support.pix4d.com/hc/en-us/articles/360044950312) (lists `.p4m` without a format) |
| Report charts: status colours (good, warning, critical) always paired with an icon or a shape and a word; categorical blue, orange, aqua for sigma E, N, U, checked for colour-vision deficiency in light and dark themes, with direct labels and a table because aqua is under 3:1 on the light surface | `report.rs` | Data-visualisation reference palette and checks (adjacent-pair CVD ΔE ≥ 8, normal-vision ΔE ≥ 15), validated with its script: worst CVD ΔE 9.2 light, 9.4 dark |
| MCP: JSON-RPC 2.0 over stdio, one message per line, logs on stderr; `initialize` (version negotiation, `instructions`), `tools/list`, `tools/call` (`content`, `structuredContent`, `isError`), `prompts/list`/`get`, `ping`, `notifications/progress` with the caller's `progressToken` | `azimut-cli` `mcp.rs` | [Model Context Protocol specification 2025-06-18](https://modelcontextprotocol.io/specification/2025-06-18) |

## Outputs

| Rule | Where | Source |
|---|---|---|
| OpenDroneMap `geo.txt`: projection line, then `image geo_x geo_y geo_z [yaw pitch roll] [horizontal vertical accuracy]`; angles are used for radiometric calibration | `azimut-export` | [ODM documentation, geo.txt](https://docs.opendronemap.org/geo/) |
| Gimbal angles in XMP `drone-dji:GimbalYawDegree`, `GimbalPitchDegree`, `GimbalRollDegree` | `photo.rs` | [Matrice 4 Series User Manual](https://dl.djicdn.com/downloads/DJI_Matrice_4_Series/DJI_Matrice_4_Series_User_Manual_en.pdf); [Mavic 3M Image Processing Guide](https://dl.djicdn.com/downloads/DJI_Mavic_3_Enterprise/20230829/Mavic_3M_Image_Processing_Guide_EN.pdf) |
