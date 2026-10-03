#!/usr/bin/env bash
# Render the hicolor PNG sizes from the master SVG.
#
# The PNGs are committed so that installing needs no SVG renderer. Run this
# after editing packaging/linux/icons/chukcut.svg, and commit the result.
#
# Needs inkscape (preferred) or rsvg-convert.
set -euo pipefail

here="$(cd -- "$(dirname -- "$0")" >/dev/null && pwd)"
svg="$here/icons/chukcut.svg"
out="$here/icons/hicolor"

render() {
    local size="$1" dest="$2"
    if command -v inkscape >/dev/null; then
        inkscape "$svg" -w "$size" -h "$size" -o "$dest" 2>/dev/null
    elif command -v rsvg-convert >/dev/null; then
        rsvg-convert -w "$size" -h "$size" -o "$dest" "$svg"
    else
        echo "make-icons: need inkscape or rsvg-convert" >&2
        exit 1
    fi
}

for size in 16 22 24 32 48 64 128 256 512; do
    dir="$out/${size}x${size}/apps"
    mkdir -p "$dir"
    render "$size" "$dir/chukcut.png"
    echo "  ${size}x${size}"
done
mkdir -p "$out/scalable/apps"
cp "$svg" "$out/scalable/apps/chukcut.svg"
