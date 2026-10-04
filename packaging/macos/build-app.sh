#!/usr/bin/env bash
# EXPERIMENTAL. Build chukcut.app for the Mac's own architecture (arm64 or
# x86_64), with Homebrew's FFmpeg and every other non-system dylib copied in.
#
#   packaging/macos/build-app.sh                   build release, then pack
#   packaging/macos/build-app.sh --no-build        pack the binaries in target/release
#   packaging/macos/build-app.sh --version 0.2.0   set the bundle version
#
# Writes target/dist/chukcut-<version>-<arch>-macos.app.tar.gz. Two of them,
# one per architecture, become one universal .dmg in make-universal.sh.
# Needs Homebrew's dylibbundler. chukcut is a Linux program: today the
# engine does not compile for macOS (docs/decisions/0033-release-builds.md
# says what blocks it), so this script is only reached once that is fixed.
set -euo pipefail

root="$(cd -- "$(dirname -- "$0")/../.." >/dev/null && pwd)"
cd "$root"

build=1
version=""
while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) build=0 ;;
        --version) version="$2"; shift ;;
        -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "build-app.sh: unknown option '$1'" >&2; exit 2 ;;
    esac
    shift
done
[ -n "$version" ] || version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)"
version="${version#v}"
arch="$(uname -m)"

binaries=(chukcut chukcut-ml-worker chukcut-cli)
if [ "$build" = 1 ]; then
    cargo build --release --locked -p chukcut -p chukcut-ml-worker -p chukcut-cli
fi
for bin in "${binaries[@]}"; do
    [ -x "target/release/$bin" ] || { echo "build-app.sh: no binary at target/release/$bin" >&2; exit 1; }
done

work="$root/target/macos/$arch"
app="$work/chukcut.app"
dist="$root/target/dist"
rm -rf "$work"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks" "$dist"

# The worker sits next to the editor, where the editor looks for it
# (engine::modules::ml::worker::beside).
for bin in "${binaries[@]}"; do
    install -m755 "target/release/$bin" "$app/Contents/MacOS/$bin"
done

# The icon: an .icns from the PNG renders of packaging/linux/icons.
icons=packaging/linux/icons/hicolor
iconset="$work/chukcut.iconset"
mkdir -p "$iconset"
for pair in 16:16 32:16@2x 32:32 64:32@2x 128:128 256:128@2x 256:256 512:256@2x 512:512; do
    src="${pair%%:*}"
    dst="${pair#*:}"
    case "$dst" in
        *@2x) name="icon_${dst%@2x}x${dst%@2x}@2x.png" ;;
        *) name="icon_${dst}x${dst}.png" ;;
    esac
    cp "$icons/${src}x${src}/apps/chukcut.png" "$iconset/$name"
done
iconutil -c icns -o "$app/Contents/Resources/chukcut.icns" "$iconset"
rm -rf "$iconset"

cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>chukcut</string>
  <key>CFBundleDisplayName</key><string>chukcut</string>
  <key>CFBundleIdentifier</key><string>io.github.chuk-development.chukcut</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleExecutable</key><string>chukcut</string>
  <key>CFBundleIconFile</key><string>chukcut</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>chukcut records voiceovers from the microphone.</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>chukcut project</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>CFBundleTypeExtensions</key><array><string>chukcut</string></array>
      <key>LSHandlerRank</key><string>Owner</string>
    </dict>
  </array>
</dict>
</plist>
EOF

# Copy every Homebrew dylib the binaries reach (FFmpeg and its codecs,
# shaderc, ...) into Contents/Frameworks and point the load commands there.
# The system's own libraries and frameworks stay where they are.
dylibbundler -cd -of -b \
    -x "$app/Contents/MacOS/chukcut" \
    -x "$app/Contents/MacOS/chukcut-cli" \
    -x "$app/Contents/MacOS/chukcut-ml-worker" \
    -d "$app/Contents/Frameworks" \
    -p @executable_path/../Frameworks/

cp LICENSE NOTICE.md "$app/Contents/Resources/"

# install_name_tool broke the signatures; an ad-hoc one is enough to run on
# Apple silicon. A real signature and notarisation need a Developer ID.
codesign --force --sign - "$app/Contents/Frameworks/"*.dylib "$app/Contents/MacOS/"*
codesign --force --sign - "$app"

out="$dist/chukcut-$version-$arch-macos.app.tar.gz"
tar -C "$work" -czf "$out" chukcut.app
echo "$out"
