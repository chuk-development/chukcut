#!/usr/bin/env bash
# Debug run of chukcut with its own config, data, cache and state directories,
# so a test session never touches your real settings, recent projects,
# autosaves, proxies or downloaded models.
#
#   scripts/run-dev.sh [options] [-- app arguments]
#
#   --fresh          delete the isolated directories first (first-run state)
#   --release        build and run the release profile instead of debug
#   --display :NN    run on another X display, e.g. a private Xvfb
#   --lavapipe       force Mesa's software Vulkan driver (no GPU needed)
#
# The isolated directories live in _scratch/dev-xdg (ignored by git);
# CHUKCUT_DEV_DIR moves them. Everything after "--" goes to the app:
#
#   scripts/run-dev.sh -- _scratch/media/vertical.mp4
#   scripts/run-dev.sh --fresh --display :94 --lavapipe
set -euo pipefail

root="$(cd -- "$(dirname -- "$0")/.." >/dev/null && pwd)"
dev="${CHUKCUT_DEV_DIR:-$root/_scratch/dev-xdg}"

profile=dev
fresh=0
args=()
while [ $# -gt 0 ]; do
    case "$1" in
        --fresh) fresh=1 ;;
        --release) profile=release ;;
        --display) export DISPLAY="$2"; unset WAYLAND_DISPLAY; shift ;;
        --lavapipe) export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json ;;
        -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        --) shift; args=("$@"); break ;;
        *) args=("$@"); break ;;
    esac
    shift
done

if [ "$fresh" = 1 ]; then
    rm -rf "$dev"
fi
mkdir -p "$dev"/{config,data,cache,state}

export XDG_CONFIG_HOME="$dev/config"
export XDG_DATA_HOME="$dev/data"
export XDG_CACHE_HOME="$dev/cache"
export XDG_STATE_HOME="$dev/state"
export RUST_LOG="${RUST_LOG:-info}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

# Same rule as install.sh: about 4 GB of memory per parallel rustc job.
mem_gb=$(awk '/MemTotal/ {print int($2 / 1048576)}' /proc/meminfo)
jobs=$(( mem_gb / 4 ))
[ "$jobs" -lt 1 ] && jobs=1
[ "$jobs" -gt "$(nproc)" ] && jobs=$(nproc)
jobs="${CARGO_BUILD_JOBS:-$jobs}"

cd "$root"
if [ "$profile" = release ]; then
    cargo build --release -p chukcut -j "$jobs"
    bin="$root/target/release/chukcut"
else
    cargo build -p chukcut -j "$jobs"
    bin="$root/target/debug/chukcut"
fi

echo "chukcut: isolated directories in $dev (logs in $dev/state/chukcut/logs)" >&2
exec "$bin" "${args[@]}"
