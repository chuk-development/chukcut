#!/usr/bin/env bash
# Set the version of every chukcut crate, in the five Cargo.toml files and in
# Cargo.lock, so the binaries report the version the release files are named
# after.
#
#   packaging/set-version.sh 0.2.0        bump before pushing to the release branch
#   packaging/set-version.sh 0.2.0-rc.1   a pre-release (the draft is marked as one)
#   packaging/set-version.sh              print the current version
#
# The release workflow runs it on its own checkout when a version override is
# given, the way storytold/craft-common-actions runs `cargo xtask version set`
# (docs/decisions/0037-release-workflow-from-filmcraft.md). Cargo.lock is
# edited too, because the builds run with --locked.
set -euo pipefail

root="$(cd -- "$(dirname -- "$0")/.." >/dev/null && pwd)"
cd "$root"

crates=(app cli engine ml-worker whisper)
current="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)"
if [ $# -eq 0 ]; then
    echo "$current"
    exit 0
fi
version="${1#v}"

# Semantic versioning, as the release workflow checks it: 1.2.3 or 1.2.3-rc.1.
if ! printf '%s' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$'; then
    echo "set-version.sh: '$version' is not a version like 1.2.3 or 1.2.3-rc.1" >&2
    exit 2
fi

# Only the package's own version line, the first `version =` in the file;
# dependency tables use inline `version = ` inside braces, never at the start.
for crate in "${crates[@]}"; do
    sed -i "0,/^version = \".*\"/s//version = \"$version\"/" "crates/$crate/Cargo.toml"
done

# In Cargo.lock the version line follows the `name = "chukcut…"` line.
awk -v v="$version" '
    /^name = "chukcut(-[a-z-]+)?"$/ { print; ours = 1; next }
    ours && /^version = / { print "version = \"" v "\""; ours = 0; next }
    { ours = 0; print }
' Cargo.lock >Cargo.lock.new
mv Cargo.lock.new Cargo.lock

echo "chukcut $current -> $version"
