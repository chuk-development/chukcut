#!/usr/bin/env bash
# Run the engine's library tests twice: on the machine's real GPU, then on
# lavapipe (Mesa's software Vulkan). Blending results differ between the two
# (NVIDIA rounds the source alpha of an 8-bit target to 1/255 in the
# blender, lavapipe does not), so a GPU test is only trusted when both pass.
#
#   scripts/gpu-tests.sh                 every library test, twice
#   scripts/gpu-tests.sh faint_alpha     only tests whose name matches
#   scripts/gpu-tests.sh -- --nocapture  anything after -- goes to the tests
#
# The lavapipe run sets CHUKCUT_TEST_ADAPTER=llvmpipe, so a test that got a
# different adapter fails instead of passing on the wrong one
# (render::test_context).
set -uo pipefail

cd "$(dirname "$0")/.."
LVP=/usr/share/vulkan/icd.d/lvp_icd.json
status=0

run() {
    echo "== $1"
    shift
    if ! env "$@" cargo test -p chukcut-engine -j 4 --lib "${ARGS[@]}"; then
        status=1
    fi
}

ARGS=("$@")
run "real GPU" CHUKCUT_TEST_ADAPTER=
if [ -f "$LVP" ]; then
    run "lavapipe" VK_ICD_FILENAMES="$LVP" CHUKCUT_TEST_ADAPTER=llvmpipe
else
    echo "no lavapipe at $LVP (Debian/Ubuntu: mesa-vulkan-drivers); the second run was skipped"
    status=1
fi
exit $status
