# PIX4Dmatic

How to bring Azimut's positions into PIX4Dmatic.

PIX4D documents a geolocation import format for PIX4Dmatic; its project files are not
documented, so Azimut does not write them. `<flight>_pix4dmatic.csv` uses the "geographic
coordinates with accuracy values" layout: no header (one `#` comment line), then image,
latitude, longitude, z, horizontal and vertical accuracy (1-sigma: sqrt(sE² + sN²) and sU).
Without angles, PIX4Dmatic keeps the orientation read from the images. Create a project with
the images (the originals, or the copies written by `--write-images copy`), then File >
Import > Image geolocation and orientation, and choose the geographic system of the frame
with ellipsoidal heights. With `--geoid`, `<flight>_pix4dmatic_orthometric.csv` holds the
orthometric heights instead. Files without an uncertainty get no row, so PIX4Dmatic keeps the
position read from them.

---
[Back to the README](../README.md)
