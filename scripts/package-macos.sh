#!/usr/bin/env bash
# Build universal (Apple silicon + Intel) macOS release packages:
#   rocketman-<tag>-macos-universal.dmg     rocketman.app, drag to Applications
#   rocketman-<tag>-macos-universal.tar.gz  rocketman and rocketman-tui binaries
#
# The app is ad-hoc signed only, not notarized, so Gatekeeper asks before the
# first launch of a downloaded copy (right-click > Open, or
# `xattr -dr com.apple.quarantine /Applications/rocketman.app`).
#
# Runs on macOS with Xcode (the Metal compiler is needed for GPUI's shaders).
# Usage: scripts/package-macos.sh <tag> <out-dir>
# Prints the paths of the packages it wrote.
set -euo pipefail

TAG=$1
OUT=$2

cd "$(dirname "$0")/.."
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

# awk, not sed: BSD sed rejects the GNU-style `{s/…/p}` block.
VERSION=$(awk -F'"' '/^\[/ { p = ($0 == "[package]") } p && /^version *=/ { print $2; exit }' Cargo.toml)
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)

rustup target add "${TARGETS[@]}" >&2
for target in "${TARGETS[@]}"; do
  cargo build --release --locked --target "$target" >&2
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
for name in rocketman rocketman-tui; do
  lipo -create -output "$work/bin/$name" \
    "target/${TARGETS[0]}/release/$name" "target/${TARGETS[1]}/release/$name"
  lipo -info "$work/bin/$name" >&2
done

# rocketman.app, with the terminal app inside too so it can be linked onto PATH.
app=$work/dmg/rocketman.app
mkdir -p "$app/Contents/MacOS"
cp "$work/bin/rocketman" "$work/bin/rocketman-tui" "$app/Contents/MacOS/"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@MIN_MACOS@/$MACOSX_DEPLOYMENT_TARGET/g" \
  packaging/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >&2
codesign --force --sign - "$app/Contents/MacOS/rocketman-tui" >&2
codesign --force --sign - "$app" >&2
codesign --verify --strict --verbose=2 "$app" >&2

ln -s /Applications "$work/dmg/Applications"
dmg=$OUT/rocketman-$TAG-macos-universal.dmg
rm -f "$dmg"
hdiutil create -volname rocketman -srcfolder "$work/dmg" -fs HFS+ -format UDZO -ov "$dmg" >&2
echo "$dmg"

tarball=$OUT/rocketman-$TAG-macos-universal.tar.gz
tar -czf "$tarball" -C "$work/bin" rocketman rocketman-tui
echo "$tarball"
