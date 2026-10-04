#!/usr/bin/env bash
# EXPERIMENTAL. Merge an arm64 and an x86_64 chukcut.app into one universal
# app and pack it as a .dmg.
#
#   packaging/macos/make-universal.sh <version> <arm64.app.tar.gz> <x86_64.app.tar.gz>
#
# Writes target/dist/chukcut-<version>-universal-macos.dmg. Every Mach-O file
# (the three binaries and each bundled dylib) is joined with lipo, so both
# builds must have bundled the same set of dylibs: they do when both ran
# against the same Homebrew state, which is why the two build jobs run in the
# same workflow. Anything else in the bundle is taken from the arm64 build.
set -euo pipefail

[ $# -eq 3 ] || { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
version="${1#v}"
arm="$2"
intel="$3"

root="$(cd -- "$(dirname -- "$0")/../.." >/dev/null && pwd)"
work="$root/target/macos/universal"
dist="$root/target/dist"
rm -rf "$work"
mkdir -p "$work/arm64" "$work/x86_64" "$work/dmg" "$dist"
tar -C "$work/arm64" -xzf "$arm"
tar -C "$work/x86_64" -xzf "$intel"

a="$work/arm64/chukcut.app"
x="$work/x86_64/chukcut.app"
u="$work/dmg/chukcut.app"
cp -R "$a" "$u"

missing=0
while IFS= read -r -d '' file; do
    rel="${file#"$a"/}"
    file "$file" | grep -q 'Mach-O' || continue
    if [ ! -f "$x/$rel" ]; then
        echo "make-universal.sh: $rel is in the arm64 build only" >&2
        missing=1
        continue
    fi
    lipo -create "$file" "$x/$rel" -output "$u/$rel"
done < <(find "$a/Contents" -type f -print0)
while IFS= read -r -d '' file; do
    rel="${file#"$x"/}"
    [ -e "$a/$rel" ] || { echo "make-universal.sh: $rel is in the x86_64 build only" >&2; missing=1; }
done < <(find "$x/Contents" -type f -print0)
[ "$missing" = 0 ] || { echo "make-universal.sh: the two builds bundled different files" >&2; exit 1; }

# lipo keeps each slice's signature, but sign the whole again so the bundle
# seal matches the merged files.
codesign --force --sign - "$u/Contents/Frameworks/"*.dylib "$u/Contents/MacOS/"*
codesign --force --sign - "$u"
lipo -info "$u/Contents/MacOS/chukcut"

ln -s /Applications "$work/dmg/Applications"
out="$dist/chukcut-$version-universal-macos.dmg"
rm -f "$out"
hdiutil create -volname "chukcut $version" -srcfolder "$work/dmg" -ov -format UDZO "$out" >/dev/null
(cd "$dist" && shasum -a 256 "$(basename "$out")" >"$(basename "$out").sha256")
echo "$out"
