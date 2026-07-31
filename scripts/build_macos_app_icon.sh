#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICON_DIR="$ROOT/crates/squallz-gui/icons"
SOURCE="$ICON_DIR/AppIcon.icon"
CANONICAL_SOURCE="$ICON_DIR/squallz-icon-source.png"
COMPOSER_SOURCE="$SOURCE/Assets/Squallz.png"
OUTPUT="$ICON_DIR/AppIcon-compiled.car"
MANIFEST="$ICON_DIR/macos-icon-build.json"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS app icon compilation requires macOS and Xcode 26 or newer." >&2
  exit 2
fi

if ! cmp -s "$CANONICAL_SOURCE" "$COMPOSER_SOURCE"; then
  echo "AppIcon.icon must use the canonical Squallz icon source without visual changes." >&2
  exit 1
fi

XCODE_DETAILS="$(xcodebuild -version)"
XCODE_VERSION="$(awk 'NR == 1 { print $2 }' <<<"$XCODE_DETAILS")"
XCODE_BUILD="$(awk 'NR == 2 { print $3 }' <<<"$XCODE_DETAILS")"
XCODE_MAJOR="${XCODE_VERSION%%.*}"
if [[ -z "$XCODE_MAJOR" || "$XCODE_MAJOR" -lt 26 ]]; then
  echo "macOS app icon compilation requires Xcode 26 or newer." >&2
  exit 2
fi

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/squallz-app-icon.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

xcrun actool "$SOURCE" \
  --compile "$WORK_DIR" \
  --output-format human-readable-text \
  --notices \
  --warnings \
  --output-partial-info-plist "$WORK_DIR/asset-info.plist" \
  --app-icon AppIcon \
  --enable-on-demand-resources NO \
  --development-region en \
  --target-device mac \
  --minimum-deployment-target 11.0 \
  --platform macosx

test -s "$WORK_DIR/Assets.car"
test -s "$WORK_DIR/AppIcon.icns"
test "$(plutil -extract CFBundleIconName raw -o - "$WORK_DIR/asset-info.plist")" = "AppIcon"
test "$(plutil -extract CFBundleIconFile raw -o - "$WORK_DIR/asset-info.plist")" = "AppIcon"

cp "$WORK_DIR/Assets.car" "$OUTPUT"
SOURCE_SHA256="$(shasum -a 256 "$CANONICAL_SOURCE" | awk '{ print $1 }')"
OUTPUT_SHA256="$(shasum -a 256 "$OUTPUT" | awk '{ print $1 }')"
printf '%s\n' \
  '{' \
  '  "schema": 1,' \
  "  \"source_sha256\": \"$SOURCE_SHA256\"," \
  "  \"assets_car_sha256\": \"$OUTPUT_SHA256\"," \
  "  \"xcode_version\": \"$XCODE_VERSION\"," \
  "  \"xcode_build\": \"$XCODE_BUILD\"," \
  '  "minimum_macos": "11.0"' \
  '}' >"$MANIFEST"
echo "Updated $OUTPUT"
echo "Updated $MANIFEST"
shasum -a 256 "$CANONICAL_SOURCE" "$OUTPUT"
