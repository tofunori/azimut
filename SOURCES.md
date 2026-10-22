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
| Radiometric XMP tags must survive any edit; Pillow/ImageMagick/GDAL rewrite or drop metadata | design rule (read-only by default) | Mavic 3M Image Processing Guide; [ImageMagick #1674](https://github.com/ImageMagick/ImageMagick/issues/1674) |

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

## Outputs

| Rule | Where | Source |
|---|---|---|
| OpenDroneMap `geo.txt`: projection line, then `image geo_x geo_y geo_z [yaw pitch roll] [horizontal vertical accuracy]`; angles are used for radiometric calibration | `azimut-export` | [ODM documentation, geo.txt](https://docs.opendronemap.org/geo/) |
| Gimbal angles in XMP `drone-dji:GimbalYawDegree`, `GimbalPitchDegree`, `GimbalRollDegree` | `photo.rs` | [Matrice 4 Series User Manual](https://dl.djicdn.com/downloads/DJI_Matrice_4_Series/DJI_Matrice_4_Series_User_Manual_en.pdf); [Mavic 3M Image Processing Guide](https://dl.djicdn.com/downloads/DJI_Mavic_3_Enterprise/20230829/Mavic_3M_Image_Processing_Guide_EN.pdf) |
