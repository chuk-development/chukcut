#!/bin/bash
# Run one test binary N times, one run after another, and record each exit
# code; a failing run's output is kept next to the record. For exit races
# (docs/STATUS.md, "Robustness"): never run two GPU test binaries at once.
#
#   scripts/loop-test.sh target/debug/deps/export-<hash> 40 _scratch/loop.txt
set -u
bin=$(realpath "$1"); n=$2; out=$(realpath "$3")
: > "$out"
for i in $(seq 1 "$n"); do
  (cd crates/engine && timeout 300 "$bin") > "$out.last" 2>&1
  code=$?
  echo "run $i exit $code" >> "$out"
  [ "$code" -ne 0 ] && cp "$out.last" "$out.fail$i"
done
echo DONE >> "$out"
