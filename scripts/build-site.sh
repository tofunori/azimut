#!/usr/bin/env bash
# Assemble the GitHub Pages site into <out>: the pages in site/, the images they use from
# docs/img, and the example quality reports under exemples/. Used by the Pages workflow;
# open <out>/index.html to preview it locally.
#
#   scripts/build-site.sh <output folder>
set -euo pipefail
out="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
rm -rf "$out"
mkdir -p "$out/img" "$out/exemples"
cp -R "$root/site/." "$out/"
for f in logo.svg pipeline.svg pipeline-fr.svg report-en.png report-fr.png social-preview.png; do
  cp "$root/docs/img/$f" "$out/img/"
done
cp "$root/examples/demo-flight/report-en.html" "$root/examples/demo-flight/rapport-fr.html" "$out/exemples/"
touch "$out/.nojekyll"
echo "site built in $out"
