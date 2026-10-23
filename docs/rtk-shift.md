# RTK flights: correcting the base position

How `azimut shift` moves real-time positions to a corrected base position.

When the aircraft flew in RTK mode with its own base (a D-RTK 2 or a GNSS receiver), its
real-time positions are relative to the coordinates that base broadcast. If those were only
approximate, for example a base that averaged its own position for a few minutes, every
photo is off by the same vector. Once the base position is known precisely, for example
from a CSRS-PPP solution of the base's RINEX, `shift` moves every real-time camera position
of the MRK by the difference:

```sh
azimut shift /path/to/campaign \
    --from 46.3490000,-72.5510000,60.000 \
    --to 46.3489901,-72.5510153,59.412 \
    --frame "NAD83(CSRS) v8, epoch 2010.0" \
    --out results
```

`--from` is what the base broadcast and `--to` the corrected coordinates of the same physical
point, both with ellipsoidal heights. `--to-ppp 12345678.sum` takes the corrected coordinates
from a CSRS-PPP summary instead, and prints which point they refer to (`--from` must be that
point). `--enu E,N,U` gives the shift in metres instead. The
outputs are the same files as `process`. The MRK positions are already camera centres, so no
lever arm is added. Only RTK fixed and float positions are relative to the base: single-point
positions are not shifted and are listed in `problems.csv`. The uncertainties are the
aircraft's own, relative to the base; the uncertainty of the corrected base position is not
added. A vertical shift over 5 m is flagged (an orthometric height given by mistake differs
by the geoid height, about -30 m in Quebec) and a shift over 500 m is refused.

Files exposed at another time than the RGB JPEG (Mavic 3M bands) are placed by linear
interpolation between this MRK record and the next one, if both are RTK and less than 10 s
apart; otherwise they get no position and are listed in `problems.csv`.

Images are not modified unless `--write-images` is given (see [Writing positions into the images](images.md)): the positions go to
the CSV and `geo.txt` files that Metashape, Pix4D and ODM import.

---
[Back to the README](../README.md)
