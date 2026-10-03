#!/usr/bin/env bash
# Build chukcut and install it for the current user. No sudo, no system files.
#
#   scripts/install.sh               check dependencies, build release, install
#   scripts/install.sh --check       only check dependencies
#   scripts/install.sh --no-build    install the existing target/release/chukcut
#   scripts/install.sh --cuda        build whisper.cpp with CUDA (needs nvcc)
#   scripts/install.sh --uninstall   remove everything this script installed
#
# What goes where (XDG_DATA_HOME defaults to ~/.local/share):
#
#   ~/.local/bin/chukcut                                   the binary
#   $XDG_DATA_HOME/applications/chukcut.desktop            menu entry
#   $XDG_DATA_HOME/icons/hicolor/<size>/apps/chukcut.png   icons
#   $XDG_DATA_HOME/mime/packages/chukcut.xml               .chukcut file type
#   $XDG_DATA_HOME/metainfo/<id>.metainfo.xml              software-centre data
#
# The script never installs system packages. When something is missing it
# prints the exact apt or dnf line for you to run, and stops.
#
# The same script installs a release tarball (packaging/tarball.sh): there it
# finds bin/chukcut next to itself and skips the build.
set -euo pipefail

root="$(cd -- "$(dirname -- "$0")/.." >/dev/null && pwd)"
linux="$root/packaging/linux"
metainfo_id="io.github.chuk_development.chukcut"

bindir="${CHUKCUT_BINDIR:-$HOME/.local/bin}"
datadir="${XDG_DATA_HOME:-$HOME/.local/share}"

mode=install
build=1
features=()
while [ $# -gt 0 ]; do
    case "$1" in
        --uninstall) mode=uninstall ;;
        --check) mode=check ;;
        --no-build) build=0 ;;
        --cuda) features=(--features cuda) ;;
        --bindir) bindir="$2"; shift ;;
        -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "install.sh: unknown option '$1' (try --help)" >&2; exit 2 ;;
    esac
    shift
done

if [ -t 1 ]; then
    say() { printf '\033[1m%s\033[0m\n' "$*"; }
    warn() { printf '\033[33m%s\033[0m\n' "$*" >&2; }
else
    say() { printf '%s\n' "$*"; }
    warn() { printf '%s\n' "$*" >&2; }
fi

# A tarball ships the binary in bin/ and has no Cargo.toml.
prebuilt=0
if [ ! -f "$root/Cargo.toml" ] && [ -x "$root/bin/chukcut" ]; then
    prebuilt=1
    build=0
fi

# ---------------------------------------------------------------------------
# Uninstall
# ---------------------------------------------------------------------------

refresh_caches() {
    if command -v update-desktop-database >/dev/null; then
        update-desktop-database -q "$datadir/applications" 2>/dev/null || true
    fi
    if command -v update-mime-database >/dev/null && [ -d "$datadir/mime" ]; then
        update-mime-database "$datadir/mime" >/dev/null 2>&1 || true
    fi
    if command -v gtk-update-icon-cache >/dev/null && [ -d "$datadir/icons/hicolor" ]; then
        # -t: the user's hicolor directory has no index.theme of its own.
        gtk-update-icon-cache -q -t -f "$datadir/icons/hicolor" 2>/dev/null || true
    fi
}

if [ "$mode" = uninstall ]; then
    say "Removing chukcut"
    rm -fv "$bindir/chukcut" \
        "$datadir/applications/chukcut.desktop" \
        "$datadir/mime/packages/chukcut.xml" \
        "$datadir/metainfo/$metainfo_id.metainfo.xml"
    rm -fv "$datadir"/icons/hicolor/*/apps/chukcut.png "$datadir"/icons/hicolor/scalable/apps/chukcut.svg
    refresh_caches
    echo "Projects, settings and caches are kept:"
    echo "  ${XDG_CONFIG_HOME:-$HOME/.config}/chukcut  ${XDG_DATA_HOME:-$HOME/.local/share}/chukcut"
    echo "  ${XDG_CACHE_HOME:-$HOME/.cache}/chukcut  ${XDG_STATE_HOME:-$HOME/.local/state}/chukcut"
    echo "Delete those by hand if you want them gone."
    exit 0
fi

# ---------------------------------------------------------------------------
# Dependency check
# ---------------------------------------------------------------------------

missing=()

# pkg-config module -> Debian package -> Fedora package
build_libs=(
    "libavcodec      libavcodec-dev        ffmpeg-devel"
    "libavformat     libavformat-dev       ffmpeg-devel"
    "libavutil       libavutil-dev         ffmpeg-devel"
    "libavfilter     libavfilter-dev       ffmpeg-devel"
    "libavdevice     libavdevice-dev       ffmpeg-devel"
    "libswscale      libswscale-dev        ffmpeg-devel"
    "libswresample   libswresample-dev     ffmpeg-devel"
    "libva           libva-dev             libva-devel"
    "alsa            libasound2-dev        alsa-lib-devel"
    "xkbcommon       libxkbcommon-dev      libxkbcommon-devel"
    "xkbcommon-x11   libxkbcommon-x11-dev  libxkbcommon-x11-devel"
    "wayland-client  libwayland-dev        wayland-devel"
    "x11-xcb         libx11-xcb-dev        libX11-devel"
    "xcb             libxcb1-dev           libxcb-devel"
    "fontconfig      libfontconfig-dev     fontconfig-devel"
    "freetype2       libfreetype-dev       freetype-devel"
    "shaderc         libshaderc-dev        libshaderc-devel"
)
# command -> Debian package -> Fedora package
build_tools=(
    "pkg-config  pkg-config       pkgconf-pkg-config"
    "cmake       cmake            cmake"
    "nasm        nasm             nasm"
    "c++         build-essential  gcc-c++"
)
# Needed to run, not to build: the Vulkan loader. The driver itself is the
# GPU vendor's (Mesa for Intel and AMD, the proprietary driver for NVIDIA).
runtime_libs=(
    "libvulkan.so.1  libvulkan1  vulkan-loader"
)

apt=()
dnf=()
need() { missing+=("$1"); apt+=("$2"); dnf+=("$3"); }

# ldconfig lives in /sbin on Debian, which is not on every user's PATH.
ldconfig_bin="$(command -v ldconfig || echo /sbin/ldconfig)"
have_lib() {
    "$ldconfig_bin" -p 2>/dev/null | grep -q "$1" && return 0
    compgen -G "/usr/lib*/$1*" >/dev/null || compgen -G "/usr/lib/x86_64-linux-gnu/$1*" >/dev/null \
        || compgen -G "/usr/lib/llvm-*/lib/$1*" >/dev/null
}

check_deps() {
    if [ "$build" = 1 ]; then
        if ! command -v cargo >/dev/null; then
            warn "Rust is not installed. Get it from https://rustup.rs, then run this again."
            missing+=(cargo)
        fi
        for row in "${build_tools[@]}"; do
            read -r cmd deb rpm <<<"$row"
            command -v "$cmd" >/dev/null || need "$cmd" "$deb" "$rpm"
        done
        if command -v pkg-config >/dev/null; then
            for row in "${build_libs[@]}"; do
                read -r mod deb rpm <<<"$row"
                pkg-config --exists "$mod" || need "$mod" "$deb" "$rpm"
            done
        fi
        # bindgen (ffmpeg-sys-next) loads libclang at build time.
        have_lib "libclang" || need libclang libclang-dev clang-devel
    fi
    for row in "${runtime_libs[@]}"; do
        read -r lib deb rpm <<<"$row"
        have_lib "$lib" || need "$lib" "$deb" "$rpm"
    done
    if [ "$prebuilt" = 1 ] && command -v ldd >/dev/null; then
        local gone
        gone="$(ldd "$root/bin/chukcut" 2>/dev/null | awk '/not found/ {print $1}')"
        if [ -n "$gone" ]; then
            warn "The binary needs libraries this system lacks:"
            # One library per line; the split is the point.
            # shellcheck disable=SC2086
            printf '  %s\n' $gone >&2
            warn "A prebuilt binary links the FFmpeg version of the system it was built on"
            warn "(see packaging/README.md). Install those libraries, or build from source."
            missing+=(runtime-libs)
        fi
    fi

    if [ ${#missing[@]} -eq 0 ]; then
        say "All dependencies found."
        return 0
    fi

    uniq_words() { printf '%s\n' "$@" | awk '!seen[$0]++' | tr '\n' ' '; }
    warn "Missing: ${missing[*]}"
    echo
    if [ ${#apt[@]} -gt 0 ]; then
        echo "Debian / Ubuntu / Mint:"
        echo "  sudo apt install $(uniq_words "${apt[@]}")"
        echo
        echo "Fedora (FFmpeg with H.264/HEVC comes from RPM Fusion, https://rpmfusion.org/Configuration):"
        echo "  sudo dnf install $(uniq_words "${dnf[@]}")"
        echo
    fi
    [ ${#apt[@]} -gt 0 ] && echo "Run the line for your distribution, then run this script again."
    return 1
}

say "Checking dependencies"
if ! check_deps; then
    exit 1
fi
[ "$mode" = check ] && exit 0

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

if [ "$prebuilt" = 1 ]; then
    binary="$root/bin/chukcut"
else
    binary="$root/target/release/chukcut"
fi

if [ "$build" = 1 ]; then
    # Linking wgpu, GPUI and whisper.cpp in parallel needs about 4 GB per job;
    # full parallelism OOMs on 32 GB. CARGO_BUILD_JOBS overrides.
    mem_gb=$(awk '/MemTotal/ {print int($2 / 1048576)}' /proc/meminfo)
    jobs=$(( mem_gb / 4 ))
    [ "$jobs" -lt 1 ] && jobs=1
    [ "$jobs" -gt "$(nproc)" ] && jobs=$(nproc)
    jobs="${CARGO_BUILD_JOBS:-$jobs}"
    say "Building release with $jobs jobs (the first build takes a while)"
    (cd "$root" && cargo build --release --locked -p chukcut -j "$jobs" "${features[@]}")
fi

if [ ! -x "$binary" ]; then
    warn "No binary at $binary. Run without --no-build."
    exit 1
fi

# ---------------------------------------------------------------------------
# Install
# ---------------------------------------------------------------------------

say "Installing to $bindir and $datadir"
install -Dm755 "$binary" "$bindir/chukcut"

# The menu may not have ~/.local/bin on its PATH, so the installed desktop
# file names the binary by its full path.
mkdir -p "$datadir/applications"
sed -e "s|^Exec=chukcut|Exec=$bindir/chukcut|" -e "s|^TryExec=chukcut|TryExec=$bindir/chukcut|" \
    "$linux/chukcut.desktop" >"$datadir/applications/chukcut.desktop"
chmod 644 "$datadir/applications/chukcut.desktop"

for png in "$linux"/icons/hicolor/*/apps/chukcut.png; do
    size="$(basename "$(dirname "$(dirname "$png")")")"
    install -Dm644 "$png" "$datadir/icons/hicolor/$size/apps/chukcut.png"
done
install -Dm644 "$linux/icons/chukcut.svg" "$datadir/icons/hicolor/scalable/apps/chukcut.svg"

install -Dm644 "$linux/chukcut-mime.xml" "$datadir/mime/packages/chukcut.xml"
install -Dm644 "$linux/$metainfo_id.metainfo.xml" "$datadir/metainfo/$metainfo_id.metainfo.xml"

refresh_caches

say "Installed $bindir/chukcut"
case ":$PATH:" in
    *":$bindir:"*) ;;
    *) warn "$bindir is not on your PATH. The menu entry works; for the terminal add it to PATH." ;;
esac
echo "Start it from the application menu, or run: chukcut [project.chukcut | media files...]"
echo "Remove it with: $0 --uninstall"
