# Diagnosis before processing

What `azimut scan` reads, checks and reports, before any processing.

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

---
[Back to the README](../README.md)
