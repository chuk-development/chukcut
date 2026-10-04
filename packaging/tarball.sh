#!/usr/bin/env bash
# Build a release tarball: our three binaries (the editor, the ML worker and
# the CLI) plus the desktop files and the installer.
#
#   packaging/tarball.sh              build release, then pack
#   packaging/tarball.sh --no-build   pack the binaries already in target/release
#
# Writes target/dist/chukcut-<version>-x86_64-linux.tar.xz. What is inside,
# and why FFmpeg and the NVIDIA libraries are not: packaging/README.md.
set -euo pipefail

root="$(cd -- "$(dirname -- "$0")/.." >/dev/null && pwd)"
cd "$root"

build=1
case "${1:-}" in
    --no-build) build=0 ;;
    "") ;;
    *) echo "tarball.sh: unknown option '$1'" >&2; exit 2 ;;
esac

version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)"
arch="$(uname -m)"
name="chukcut-$version-$arch-linux"
dist="$root/target/dist"
stage="$dist/$name"

# The editor finds the worker next to its own binary, so all three ship in
# bin/ together. A tarball without the worker would have no AI tools, and
# one without the CLI no MCP server: refuse rather than ship half.
binaries=(chukcut chukcut-ml-worker chukcut-cli)
if [ "$build" = 1 ]; then
    cargo build --release --locked -p chukcut -p chukcut-ml-worker -p chukcut-cli
fi
for bin in "${binaries[@]}"; do
    [ -x "$root/target/release/$bin" ] || { echo "tarball.sh: no binary at target/release/$bin" >&2; exit 1; }
done

rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/scripts" "$stage/packaging"
for bin in "${binaries[@]}"; do
    install -m755 "$root/target/release/$bin" "$stage/bin/$bin"
done
install -m755 scripts/install.sh "$stage/scripts/install.sh"
cp -r packaging/linux "$stage/packaging/linux"
rm -f "$stage/packaging/linux/make-icons.sh"
cp LICENSE NOTICE.md README.md "$stage/"
[ -f CHANGELOG.md ] && cp CHANGELOG.md "$stage/"

# Record what the binary links, so a "does not start" report can be answered
# from the tarball alone.
{
    echo "chukcut $version, built $(date -u +%Y-%m-%d) from $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    # shellcheck source=/dev/null
    echo "on $(. /etc/os-release && echo "$PRETTY_NAME"), $(pkg-config --modversion libavcodec 2>/dev/null | sed 's/^/libavcodec /')"
    echo
    for bin in "${binaries[@]}"; do
        echo "Direct library dependencies of bin/$bin (from the system, not bundled):"
        readelf -d "$root/target/release/$bin" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/  \1/p'
    done
} >"$stage/BUILD-INFO.txt"

tar -C "$dist" -cJf "$dist/$name.tar.xz" "$name"
rm -rf "$stage"
(cd "$dist" && sha256sum "$name.tar.xz" >"$name.tar.xz.sha256")
echo "$dist/$name.tar.xz"
