#!/usr/bin/env bash
# Build the two RTKLIB-EX console programs Azimut uses, convbin and rnx2rtkp, with the
# options of RTKLIB-EX's own gcc makefiles (no LAPACK). Used by the release workflow.
#
#   scripts/build-rtklib.sh <RTKLIB-EX source folder> <output folder>
#
# Needs a C compiler (CC, default cc; gcc from MinGW on Windows). EXTRA_CFLAGS is added to the
# compile line (e.g. "-arch arm64 -arch x86_64" for a universal macOS build).
set -euo pipefail
src="$1/src"
app="$1/app/consapp"
out="$2"
cc="${CC:-cc}"
mkdir -p "$out"

opts="-DTRACE -DENAGLO -DENAQZS -DENAGAL -DENACMP -DENAIRN -DNFREQ=4 -DNEXOBS=3"
cflags="-std=c99 -O3 -w -I$src $opts ${EXTRA_CFLAGS:-}"
exe=""
libs="-lm"
# MinGW defines _WIN32 but RTKLIB-EX tests WIN32 (its own Windows makefiles pass -DWIN32).
case "$("$cc" -dumpmachine)" in
  *mingw*|*cygwin*|*windows*) exe=".exe"; cflags="$cflags -DWIN32"; libs="-static -lws2_32 -lwinmm -lm" ;;
esac

common="rtkcmn trace rinex sbas preceph pntpos ephemeris ionex rtcm rtcm2 rtcm3 rtcm3e sofa"
rnx2rtkp_only="rtkpos postpos solution lambda geoid options ppp ppp_ar tides"
convbin_only="rcvraw convrnx"
receivers="novatel ublox crescent skytraq javad nvs binex rt17 septentrio swiftnav unicore"

files() { for f in "$@"; do printf '%s/%s.c ' "$src" "$f"; done; }

# shellcheck disable=SC2046,SC2086
"$cc" $cflags -o "$out/rnx2rtkp$exe" "$app/rnx2rtkp/rnx2rtkp.c" $(files $common $rnx2rtkp_only) $libs
# shellcheck disable=SC2046,SC2086
"$cc" $cflags -o "$out/convbin$exe" "$app/convbin/convbin.c" $(files $common $convbin_only) \
  $(for r in $receivers; do printf '%s/rcv/%s.c ' "$src" "$r"; done) $libs
cp "$1/license.txt" "$out/RTKLIB-LICENSE.txt"
echo "built $out/rnx2rtkp$exe and $out/convbin$exe"
