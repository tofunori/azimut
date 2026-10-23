#!/bin/sh
# Install the latest Azimut release (with RTKLIB-EX) on Linux or macOS:
#   curl -fsSL https://raw.githubusercontent.com/tofunori/azimut/main/scripts/install.sh | sh
# Installs into ~/.local/share/azimut and links ~/.local/bin/azimut.
set -eu
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  *) echo "No Azimut build for $(uname -s) $(uname -m); build from source (docs/install.md)." >&2; exit 1 ;;
esac
url="https://github.com/tofunori/azimut/releases/latest/download/azimut-$target.tar.gz"
dest="$HOME/.local/share/azimut"
tmp="$(mktemp -d)"
echo "Downloading $url"
curl -fsSL "$url" -o "$tmp/azimut.tar.gz"
rm -rf "$dest" && mkdir -p "$dest" "$HOME/.local/bin"
tar xzf "$tmp/azimut.tar.gz" -C "$dest" --strip-components 1
ln -sf "$dest/azimut" "$HOME/.local/bin/azimut"
rm -rf "$tmp"
echo "Installed: $("$dest/azimut" --version)"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "Add ~/.local/bin to your PATH to run 'azimut'." ;; esac
