#!/usr/bin/env bash
# Build a Debian package of chukcut for the machine's architecture.
#
#   packaging/deb/build-deb.sh                     build release, then pack
#   packaging/deb/build-deb.sh --no-build          pack the binaries in target/release
#   packaging/deb/build-deb.sh --version 0.2.0     set the package version
#
# Writes target/dist/chukcut_<version>_<arch>.deb (arch from dpkg: amd64,
# arm64). Needs dpkg-deb and dpkg-shlibdeps (package dpkg-dev), and the
# build's FFmpeg, ALSA and X11 libraries installed from dpkg packages, which is
# what dpkg-shlibdeps reads the Depends line from.
#
# The package links the FFmpeg of the build machine, so a .deb made on
# Ubuntu 24.04 installs on Ubuntu 24.04, Mint 22 and their relatives, and
# apt refuses it where those library packages do not exist. What goes where,
# and why: packaging/README.md.
set -euo pipefail

root="$(cd -- "$(dirname -- "$0")/../.." >/dev/null && pwd)"
cd "$root"

build=1
version=""
while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) build=0 ;;
        --version) version="$2"; shift ;;
        -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "build-deb.sh: unknown option '$1'" >&2; exit 2 ;;
    esac
    shift
done

[ -n "$version" ] || version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)"
# A tag name "v0.2.0" is version 0.2.0. A pre-release "0.2.0-rc.1" becomes
# "0.2.0~rc.1": in Debian a "-" starts the revision, and "~" sorts before the
# release, which is what a pre-release means.
version="${version#v}"
version="${version//-/\~}"
case "$version" in
    [0-9]*) ;;
    *) echo "build-deb.sh: version '$version' must start with a digit" >&2; exit 2 ;;
esac

arch="$(dpkg --print-architecture)"
name="chukcut_${version}_${arch}"
dist="$root/target/dist"
work="$root/target/deb"
stage="$work/$name"

binaries=(chukcut chukcut-ml-worker chukcut-cli)
if [ "$build" = 1 ]; then
    cargo build --release --locked -p chukcut -p chukcut-ml-worker -p chukcut-cli
fi
for bin in "${binaries[@]}"; do
    [ -x "target/release/$bin" ] || { echo "build-deb.sh: no binary at target/release/$bin" >&2; exit 1; }
done

rm -rf "$work"
mkdir -p "$stage/DEBIAN" "$dist"

# The editor and the CLI go on PATH. The worker is private to the editor:
# it finds it in <prefix>/libexec/chukcut/ (engine::modules::ml::worker::beside).
install -Dm755 target/release/chukcut "$stage/usr/bin/chukcut"
install -Dm755 target/release/chukcut-cli "$stage/usr/bin/chukcut-cli"
install -Dm755 target/release/chukcut-ml-worker "$stage/usr/libexec/chukcut/chukcut-ml-worker"

linux=packaging/linux
metainfo_id="io.github.chuk_development.chukcut"
install -Dm644 "$linux/chukcut.desktop" "$stage/usr/share/applications/chukcut.desktop"
install -Dm644 "$linux/chukcut-mime.xml" "$stage/usr/share/mime/packages/chukcut.xml"
install -Dm644 "$linux/$metainfo_id.metainfo.xml" "$stage/usr/share/metainfo/$metainfo_id.metainfo.xml"
install -Dm644 "$linux/icons/chukcut.svg" "$stage/usr/share/icons/hicolor/scalable/apps/chukcut.svg"
for png in "$linux"/icons/hicolor/*/apps/chukcut.png; do
    size="$(basename "$(dirname "$(dirname "$png")")")"
    install -Dm644 "$png" "$stage/usr/share/icons/hicolor/$size/apps/chukcut.png"
done

doc="$stage/usr/share/doc/chukcut"
install -Dm644 NOTICE.md "$doc/NOTICE.md"
install -Dm644 README.md "$doc/README.md"
cat >"$doc/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: chukcut
Source: https://github.com/chuk-development/chukcut

Files: *
Copyright: chukcut contributors
License: GPL-3+
 This program is free software: you can redistribute it and/or modify it
 under the terms of the GNU General Public License as published by the Free
 Software Foundation, either version 3 of the License, or (at your option)
 any later version.
 .
 On Debian systems, the complete text of the GNU General Public License
 version 3 can be found in "/usr/share/common-licenses/GPL-3".
 .
 Third-party components and the codec patent notice: NOTICE.md in this
 directory.
EOF
chmod 644 "$doc/copyright"

# The Depends line from the binaries' own NEEDED entries: dpkg-shlibdeps
# maps each library to the package that ships it, with the minimum version
# the build used (libavcodec60 (>= 7:6.1), libasound2t64, ...). It insists on
# a debian/control in the working directory, so it gets an empty one.
mkdir -p "$work/debian"
printf 'Source: chukcut\n\nPackage: chukcut\nArchitecture: any\n' >"$work/debian/control"
shlibs="$(cd "$work" && dpkg-shlibdeps -O \
    "$stage/usr/bin/chukcut" "$stage/usr/bin/chukcut-cli" "$stage/usr/libexec/chukcut/chukcut-ml-worker" \
    | sed -n 's/^shlibs:Depends=//p')"
[ -n "$shlibs" ] || { echo "build-deb.sh: dpkg-shlibdeps found no dependencies" >&2; exit 1; }
rm -rf "$work/debian"

# What the binaries load with dlopen, so dpkg-shlibdeps cannot see it: wgpu
# opens the Vulkan loader, GPUI opens the Wayland client library. The Vulkan
# driver itself is the GPU vendor's (Mesa, or the NVIDIA driver), and the
# VAAPI driver only matters for hardware decode and encode.
depends="$shlibs, libvulkan1"
recommends="mesa-vulkan-drivers | vulkan-icd, libwayland-client0, libwayland-cursor0, va-driver-all | va-driver"

# The umask of the build user must not leak into the package.
find "$stage" -type d -exec chmod 755 {} +

installed_kb="$(du -sk --exclude=DEBIAN "$stage" | cut -f1)"
cat >"$stage/DEBIAN/control" <<EOF
Package: chukcut
Version: $version
Architecture: $arch
Maintainer: chukcut contributors <https://github.com/chuk-development/chukcut/issues>
Installed-Size: $installed_kb
Depends: $depends
Recommends: $recommends
Section: video
Priority: optional
Homepage: https://github.com/chuk-development/chukcut
Description: video editor with a native GPU interface
 chukcut is a video editor in the style of CapCut: open a phone video, cut
 it, add captions and effects, and export. The interface and the compositor
 both run on the GPU (Vulkan). Hardware decode and encode use VAAPI and
 NVDEC/NVENC when the driver supports them.
 .
 The package contains the editor (chukcut), the command line and MCP server
 (chukcut-cli) and the process that runs the AI models (chukcut-ml-worker).
 ONNX Runtime and the models download on first use.
EOF

# The desktop database, the MIME database and the icon cache are refreshed
# by dpkg triggers that desktop-file-utils, shared-mime-info and the icon
# themes register for these directories, so the package needs no scripts.

dpkg-deb --root-owner-group -Zxz --build "$stage" "$dist/$name.deb" >/dev/null
rm -rf "$work"
(cd "$dist" && sha256sum "$name.deb" >"$name.deb.sha256")
echo "$dist/$name.deb"
