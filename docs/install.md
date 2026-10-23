# Installation

Azimut is one command-line program, `azimut`, for Linux, macOS and Windows. PPK also needs
two console programs of RTKLIB-EX; the release archives and install scripts include them.

## 1. Azimut

**One line** (installs the latest release, RTKLIB-EX included):

```powershell
# Windows, in PowerShell
irm https://raw.githubusercontent.com/tofunori/azimut/main/scripts/install.ps1 | iex
```

```sh
# macOS or Linux, in a terminal
curl -fsSL https://raw.githubusercontent.com/tofunori/azimut/main/scripts/install.sh | sh
```

The scripts install into `%LOCALAPPDATA%\azimut` (Windows, added to the user `PATH`) or
`~/.local/share/azimut` with a link in `~/.local/bin` (macOS, Linux).

**Archives.** Each [release](https://github.com/tofunori/azimut/releases) has an archive per
system: Linux x86-64, macOS (Apple silicon and Intel), Windows x86-64. Each holds `azimut`
and an `rtklib` folder with `convbin` and `rnx2rtkp`; Azimut finds them there by itself.
The programs are not signed: on Windows, SmartScreen may ask to confirm ("More info" → "Run
anyway"); on macOS, an archive downloaded with a browser may need
`xattr -dr com.apple.quarantine <folder>`.

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

## 2. RTKLIB-EX (only when building from source)

The release archives include RTKLIB-EX. From source, build its two console programs with the
same options as the archives:

```sh
git clone https://github.com/rtklibexplorer/RTKLIB
scripts/build-rtklib.sh RTKLIB rtklib-bin      # convbin and rnx2rtkp, no LAPACK needed
```

Then give the folder to each run (`--rtklib rtklib-bin`), set `AZIMUT_RTKLIB_DIR`, put the
programs in an `rtklib` folder next to `azimut`, or add them to the `PATH`.

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
