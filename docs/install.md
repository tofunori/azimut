# Installation

Azimut is one command-line program, `azimut`, for Linux, macOS and Windows. PPK also needs
the two console programs of RTKLIB-EX, which Azimut drives but does not ship.

## 1. Azimut

**Prebuilt binaries.** Each [release](https://github.com/tofunori/azimut/releases) carries
`azimut` for Linux (x86-64), macOS (Apple silicon and Intel) and Windows (x86-64). Unpack it
and put it on the `PATH`.

**From source.** With [Rust](https://rustup.rs) 1.82 or newer:

```sh
git clone https://github.com/tofunori/azimut
cd azimut
cargo install --path crates/azimut-cli     # installs `azimut` into ~/.cargo/bin
azimut --help
```

Try it without data:

```sh
azimut demo demo/DJI_Demo --captures 96 --survey
azimut scan demo
azimut shift demo --enu 0.42,-0.18,0.65 --frame "NAD83(CSRS) v8, epoch 2010.0" --out demo/out
```

`demo/out/DJI_Demo_report.html` is the quality report of this synthetic flight.

## 2. RTKLIB-EX (for PPK)

Azimut uses `convbin` (raw log to RINEX) and `rnx2rtkp` (PPK) from
[RTKLIB-EX](https://github.com/rtklibexplorer/RTKLIB) (BSD-2).

- **Windows**: download a release of RTKLIB-EX and keep its `bin` folder.
- **Linux, macOS**: build the two programs:

  ```sh
  git clone https://github.com/rtklibexplorer/RTKLIB
  make -C RTKLIB/app/consapp/convbin/gcc
  make -C RTKLIB/app/consapp/rnx2rtkp/gcc LDLIBS=-lm   # without LAPACK, libgfortran is not needed
  ```

Then either give the folder to each run (`--rtklib /path/to/bin`), set
`AZIMUT_RTKLIB_DIR`, or put both programs on the `PATH`.

## 3. Reference files (optional)

| File | Needed for | Where |
|---|---|---|
| `igs20.atx` | Antenna phase-centre models of the base (`--antex`) | [files.igs.org](https://files.igs.org/pub/station/general/igs20.atx) |
| `ca_nrc_CGG2013an83.tif` | CGVD2013 heights for NAD83(CSRS) positions (`--geoid`) | [cdn.proj.org](https://cdn.proj.org/ca_nrc_CGG2013an83.tif) |
| `ca_nrc_CGG2013ai08.tif` | CGVD2013 heights for ITRF or IGS positions (`--geoid`) | [cdn.proj.org](https://cdn.proj.org/ca_nrc_CGG2013ai08.tif) |

## 4. Agents (optional)

To let Claude Code or Codex run Azimut for you, see [Running Azimut from an agent](mcp.md).

---
[Back to the README](../README.md)
