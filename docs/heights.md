# Orthometric heights (CGVD2013)

How `--geoid` adds orthometric heights, and why the grid must match the frame.

`--geoid` (with `process` or `shift`) adds `orthometric_height` and `vertical_datum` columns
to the CSV, H = h - N, with the geoid undulation N interpolated bilinearly in a grid in the
PROJ GeoTIFF format. For CGVD2013, download NRCan's CGG2013a converted by PROJ (Open
Government Licence - Canada):

| Positions in | Grid |
|---|---|
| NAD83(CSRS), e.g. from CSRS-PPP | [`ca_nrc_CGG2013an83.tif`](https://cdn.proj.org/ca_nrc_CGG2013an83.tif) |
| ITRF or IGS frames | [`ca_nrc_CGG2013ai08.tif`](https://cdn.proj.org/ca_nrc_CGG2013ai08.tif) |

```sh
azimut process /path/to/campaign ... --base-ppp 12345678.sum \
    --geoid ca_nrc_CGG2013an83.tif --out results
```

A grid applies to ellipsoidal heights in one frame, and the two grids differ by 0.1 to 1.1 m
at 12 points across Canada, so Azimut refuses a grid made for another frame family than `--frame`
(NAD83(CSRS) positions with the ITRF grid, say). Within a family (NAD83(CSRS) v6 and v8,
ITRF2008 and IGS20) it notes that the difference between realisations is not applied.
Positions outside the grid get an empty height. `geo.txt` and the images keep ellipsoidal
heights.

---
[Back to the README](../README.md)
