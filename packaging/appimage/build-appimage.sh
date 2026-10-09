#!/usr/bin/env bash
# Build an AppImage of chukcut for the machine's architecture (x86_64 or
# aarch64), with FFmpeg and the other libraries it links bundled.
#
#   packaging/appimage/build-appimage.sh                    build release, then pack
#   packaging/appimage/build-appimage.sh --no-build         pack the binaries in target/release
#   packaging/appimage/build-appimage.sh --version 0.2.0    name the file after this version
#
# Writes target/dist/chukcut-<version>-<arch>.AppImage. linuxdeploy copies
# every library the three binaries need into the AppDir, except the ones in
# its own exclude list and in EXCLUDE below; appimagetool packs the AppDir.
# Both are AppImages themselves, downloaded at pinned versions into
# target/appimage-tools/ unless LINUXDEPLOY, APPIMAGETOOL and
# APPIMAGE_RUNTIME point at local copies. No FUSE is needed to build.
#
# What is bundled and what is not, and why: packaging/README.md.
set -euo pipefail

root="$(cd -- "$(dirname -- "$0")/../.." >/dev/null && pwd)"
cd "$root"

build=1
version=""
while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) build=0 ;;
        --version) version="$2"; shift ;;
        -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "build-appimage.sh: unknown option '$1'" >&2; exit 2 ;;
    esac
    shift
done
[ -n "$version" ] || version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)"
version="${version#v}"

arch="$(uname -m)"
case "$arch" in
    x86_64 | aarch64) ;;
    *) echo "build-appimage.sh: no AppImage tools for $arch" >&2; exit 1 ;;
esac

# Pinned tool versions. linuxdeploy has no numbered releases, only dated
# alpha tags; appimagetool and the type 2 runtime have proper ones.
linuxdeploy_tag="1-alpha-20251107-1"
appimagetool_tag="1.9.1"
runtime_tag="20251108"

tools="$root/target/appimage-tools/$arch"
fetch() {
    local url="$1" out="$2"
    if [ ! -s "$out" ]; then
        mkdir -p "$(dirname "$out")"
        curl -sSfL --retry 3 -o "$out.part" "$url"
        mv "$out.part" "$out"
    fi
    chmod +x "$out"
}
linuxdeploy="${LINUXDEPLOY:-$tools/linuxdeploy-$linuxdeploy_tag.AppImage}"
appimagetool="${APPIMAGETOOL:-$tools/appimagetool-$appimagetool_tag.AppImage}"
runtime="${APPIMAGE_RUNTIME:-$tools/runtime-$runtime_tag}"
[ -n "${LINUXDEPLOY:-}" ] || fetch "https://github.com/linuxdeploy/linuxdeploy/releases/download/$linuxdeploy_tag/linuxdeploy-$arch.AppImage" "$linuxdeploy"
[ -n "${APPIMAGETOOL:-}" ] || fetch "https://github.com/AppImage/appimagetool/releases/download/$appimagetool_tag/appimagetool-$arch.AppImage" "$appimagetool"
[ -n "${APPIMAGE_RUNTIME:-}" ] || fetch "https://github.com/AppImage/type2-runtime/releases/download/$runtime_tag/runtime-$arch" "$runtime"

# The tools are AppImages: unpack-and-run instead of mounting, so neither
# this machine nor a CI runner needs FUSE.
export APPIMAGE_EXTRACT_AND_RUN=1

binaries=(chukcut chukcut-ml-worker chukcut-cli)
if [ "$build" = 1 ]; then
    cargo build --release --locked -p chukcut -p chukcut-ml-worker -p chukcut-cli
fi
for bin in "${binaries[@]}"; do
    [ -x "target/release/$bin" ] || { echo "build-appimage.sh: no binary at target/release/$bin" >&2; exit 1; }
done

work="$root/target/appimage"
appdir="$work/AppDir"
dist="$root/target/dist"
rm -rf "$work"
mkdir -p "$appdir/usr/bin" "$dist"

# All three binaries side by side: the editor finds the worker next to
# itself (engine::modules::ml::worker::beside).
for bin in "${binaries[@]}"; do
    install -m755 "target/release/$bin" "$appdir/usr/bin/$bin"
done
# The CUDA transcription helper, when the build machine had nvcc
# (crates/engine/build.rs). It is not handed to linuxdeploy: its CUDA
# libraries (cudart, cuBLAS, ~700 MB) come from the user's system or never,
# and without them the editor transcribes on the CPU.
if [ -x target/release/chukcut-whisper-cuda ]; then
    install -m755 target/release/chukcut-whisper-cuda "$appdir/usr/bin/chukcut-whisper-cuda"
fi

linux=packaging/linux
metainfo_id="io.github.chuk_development.chukcut"
install -Dm644 "$linux/chukcut-mime.xml" "$appdir/usr/share/mime/packages/chukcut.xml"
install -Dm644 "$linux/$metainfo_id.metainfo.xml" "$appdir/usr/share/metainfo/$metainfo_id.metainfo.xml"
install -Dm644 "$linux/icons/chukcut.svg" "$appdir/usr/share/icons/hicolor/scalable/apps/chukcut.svg"
for png in "$linux"/icons/hicolor/*/apps/chukcut.png; do
    size="$(basename "$(dirname "$(dirname "$png")")")"
    install -Dm644 "$png" "$appdir/usr/share/icons/hicolor/$size/apps/chukcut.png"
done
install -Dm644 NOTICE.md "$appdir/usr/share/doc/chukcut/NOTICE.md"
install -Dm644 LICENSE "$appdir/usr/share/doc/chukcut/LICENSE"

# Libraries that must come from the system, on top of linuxdeploy's own
# exclude list (glibc, libGL, X11, fontconfig, ALSA, ...):
#
# - libva and libdrm: a VAAPI driver is loaded into the process and binds to
#   whatever libva and libdrm are already there. An older bundled libva does
#   not find the init symbol of a newer system driver, and an older libdrm
#   lacks symbols the system's Mesa needs; hardware decode would fail.
# - Vulkan, GL, EGL, GBM: the loader must match the installed drivers, and
#   so must the Wayland libraries Mesa's EGL and Vulkan WSI load.
# - libstdc++ and libgcc_s: the system's Mesa (LLVM inside) needs the
#   system's newer C++ runtime, and only one can be loaded per process.
# - NVIDIA and CUDA: never redistributed; they must match the driver.
exclude=(
    'libva.so*' 'libva-drm.so*' 'libva-x11.so*' 'libva-wayland.so*'
    'libdrm.so*' 'libdrm_*.so*'
    'libvulkan.so*' 'libGL.so*' 'libGLX.so*' 'libGLdispatch.so*' 'libEGL.so*' 'libgbm.so*'
    'libwayland-*.so*'
    'libstdc++.so*' 'libgcc_s.so*'
    'libcuda.so*' 'libnvidia-*.so*' 'libnvcuvid.so*'
    'libcudart.so*' 'libcublas.so*' 'libcublasLt.so*'
)
exclude_args=()
for pattern in "${exclude[@]}"; do
    exclude_args+=(--exclude-library "$pattern")
done

# linuxdeploy's own exclude list assumes JACK on every desktop, and it is
# not: Debian and Fedora installs often lack libjack.so.0. libavdevice links
# it, so without it nothing starts. chukcut never talks to a JACK server, so
# a bundled client library is harmless; --library deploys it regardless.
force_bundle=(libjack.so.0)
force_args=()
for lib in "${force_bundle[@]}"; do
    path="$(/sbin/ldconfig -p | awk -v l="$lib" '$1 == l && !found { print $NF; found = 1 }')"
    [ -n "$path" ] && force_args+=(--library "$path")
done

# NO_STRIP: our binaries keep their symbol table on purpose (a panic
# backtrace from a user's machine names functions; Cargo.toml, release
# profile), and the distribution's libraries are stripped already.
NO_STRIP=1 "$linuxdeploy" \
    --appdir "$appdir" \
    --executable "$appdir/usr/bin/chukcut" \
    --executable "$appdir/usr/bin/chukcut-cli" \
    --executable "$appdir/usr/bin/chukcut-ml-worker" \
    --desktop-file "$linux/chukcut.desktop" \
    --icon-file "$linux/icons/hicolor/256x256/apps/chukcut.png" \
    --custom-apprun packaging/appimage/AppRun \
    "${exclude_args[@]}" \
    "${force_args[@]}"

# A check that the exclusions held: none of these may be inside.
leaked="$(find "$appdir/usr/lib" -maxdepth 1 \( -name 'libva*.so*' -o -name 'libdrm*.so*' -o -name 'libvulkan.so*' -o -name 'libstdc++.so*' -o -name 'libcuda.so*' -o -name 'libGL*.so*' -o -name 'libwayland-*.so*' \) -printf '%f\n' 2>/dev/null || true)"
if [ -n "$leaked" ]; then
    echo "build-appimage.sh: libraries that must come from the system were bundled:" >&2
    echo "$leaked" >&2
    exit 1
fi

# Record what was bundled, so a "does not start" report can be answered from
# the AppImage alone (chukcut-<v>.AppImage --appimage-extract BUILD-INFO.txt).
{
    echo "chukcut $version, built $(date -u +%Y-%m-%d) from $(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
    # shellcheck source=/dev/null
    echo "on $(. /etc/os-release && echo "$PRETTY_NAME") $arch, $(pkg-config --modversion libavcodec 2>/dev/null | sed 's/^/libavcodec /')"
    echo
    echo "Bundled libraries (usr/lib):"
    find "$appdir/usr/lib" -maxdepth 1 -name '*.so*' -printf '  %f\n' | sort
    # The distribution source packages they came from, at the exact
    # versions: FFmpeg, x264 and x265 are GPL, and this is where their
    # corresponding source is (apt-get source <package>=<version>).
    # Only matches in the multiarch directory count: another package may
    # ship a library of the same name elsewhere (a private FFmpeg build).
    if command -v dpkg-query >/dev/null; then
        multiarch="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || gcc -dumpmachine)"
        echo
        echo "Source packages of the bundled libraries:"
        find "$appdir/usr/lib" -maxdepth 1 -name '*.so*' -printf '%f\n' | while read -r lib; do
            dpkg -S "*/$lib" 2>/dev/null | grep "/$multiarch/" | head -1 | cut -d: -f1 || true
        done | sort -u | while read -r pkg; do
            if [ -n "$pkg" ]; then
                dpkg-query -W -f '  ${source:Package} ${source:Version}\n' "$pkg" || true
            fi
        done | sort -u
    fi
} >"$appdir/BUILD-INFO.txt"

out="$dist/chukcut-$version-$arch.AppImage"
rm -f "$out"
ARCH="$arch" VERSION="$version" "$appimagetool" --no-appstream --runtime-file "$runtime" "$appdir" "$out" >/dev/null
chmod +x "$out"
(cd "$dist" && sha256sum "$(basename "$out")" >"$(basename "$out").sha256")
echo "$out"
