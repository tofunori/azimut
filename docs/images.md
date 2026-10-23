# Writing positions into the images

How `--write-images` and `azimut write-images` change only the position fields, and how to undo it.

`--write-images in-place` or `--write-images copy` (with `process` or `shift`) also writes
each file's position into the image: in the original file, or in a copy in `--images-dir`
(default `<out>/images/<flight>`), leaving the original untouched. Copies take as much disk
space as the flight; images without a position are not copied unless `--copy-unpositioned`
is given. `--dry-run` shows what would be written. The same can be done later from the
solution file, after looking at the report:

```sh
azimut write-images results/<flight>_solution.json --mode copy --dry-run
azimut write-images results/<flight>_solution.json --mode copy
```

The fields written are:

- EXIF GPS: `GPSLatitude`, `GPSLongitude` (to 1e-6 arc-second), `GPSAltitude` (to 0.1 mm)
  and their reference letters;
- DJI XMP: `drone-dji:GpsLatitude`, `GpsLongitude`, `AbsoluteAltitude`, and the 1-sigma
  `RtkStdLon` (East), `RtkStdLat` (North), `RtkStdHgt` (Up).

Heights are ellipsoidal in the frame of the positions, as DJI writes them: on the Mavic 3M,
EXIF `GPSAltitude`, XMP `AbsoluteAltitude` and the MRK `Ellh` hold the same value, although
EXIF defines the altitude above sea level. `GPSMapDatum` (`WGS-84`), `RtkFlag` and all other
fields are left as they are: the CSV records the frame.

Only the bytes of those values change. Each value is written in the existing field, at the
same size: EXIF rationals keep their 8 bytes, and an XMP value that gets longer or shorter
takes from or gives back to the packet's whitespace padding. The file keeps its length, its
other bytes and the attribute form of the DJI XMP, which matters because OpenDroneMap reads
DJI XMP fields (sensor gain, exposure time, `CaptureUUID`, angles, altitude, accuracy) only
as attributes. General tools rewrite the whole packet: ExifTool 12.76, asked to change one
DJI XMP field, rewrites every field as an element. Before writing, Azimut checks that the new
XMP parses to the same fields apart from the written ones; a file without these fields, with
its XMP already rewritten, or without an uncertainty for the position is skipped and listed.

In place, the original bytes of every change are saved first to `<flight>_image_edits.json`, and

```sh
azimut restore-images results/<flight>_image_edits.json
```

puts them back (a file that changed since is left alone). Keep a copy of the SD card anyway.

---
[Back to the README](../README.md)
