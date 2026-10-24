# Satellite background of the report map

`--basemap sentinel2` (with `process` or `shift`) draws the report's camera map over
satellite imagery. The tiles are fetched once, when the report is written, and embedded in
the HTML file: the report still opens offline and has no external file. Without network, or
if the server fails, the report is written without background and the run says why
(`basemap left out: …`).

```sh
azimut process flights/ --base base.24o --base-ppp base.sum --basemap sentinel2 --out results
export AZIMUT_BASEMAP=sentinel2   # or set it once for every run
```

The background is off by default: fetching it sends the area of the flight to the tile
server.

## Sources

| `--basemap` | Imagery | Licence |
|---|---|---|
| `none` (default) | No background | |
| `sentinel2` | [EOxCloudless](https://cloudless.eox.at) 2016 by EOX: cloud-free Sentinel-2 mosaic, 10 m pixels (fields, forest, water and roads show; single trees and small buildings do not) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/); EOX's tile service is free for non-commercial use |
| an XYZ URL with `{z}`, `{x}`, `{y}` | Any tile server, e.g. a lab or government orthophoto service | Yours to check; `--basemap-attribution` gives the credit shown under the map (required) |

The credit is printed under the map, as the licences require. Up to 48 tiles of 256 px are
embedded (about 1 MB); the zoom is chosen from the size of the flight.

## Why not Google Maps or Esri World Imagery

Both are sharper, but their terms forbid what a report that opens offline does:

- Google Maps Platform: no pre-fetching, caching, storing or offline use of map tiles, and
  an API key ([Map Tiles API policies](https://developers.google.com/maps/documentation/tile/policies)).
- Esri World Imagery: use with Esri software or an ArcGIS subscription; offline only through
  Esri content packages used in Esri software; no harvesting or redistribution of basemap
  tiles ([terms summary](https://www.esri.com/content/dam/arcgisonline/docs/tou_summary.pdf)).

EOxCloudless 2018 to 2025 are sharper in time but non-commercial only (CC BY-NC-SA 4.0), so
Azimut uses the 2016 layer, whose CC BY 4.0 licence lets the tiles travel with the report.
If you have the right to use such a source, give its tile URL and credit yourself.
