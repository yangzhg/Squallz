#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE_ROOT="$ROOT/native/macos/SquallzQuickLook"
TAURI_CONFIG="$ROOT/crates/squallz-gui/tauri.conf.json"
OUTPUT_ROOT="$ROOT/target/macos-quicklook"
BUILD_ROOT="$OUTPUT_ROOT/build"
MODULE_CACHE="$OUTPUT_ROOT/module-cache"
APPEX="$OUTPUT_ROOT/SquallzQuickLook.appex"
EXECUTABLE="$APPEX/Contents/MacOS/SquallzQuickLook"
TARGET="${TAURI_ENV_TARGET_TRIPLE:-}"

if [[ -z "$TARGET" ]]; then
  TARGET="$(rustc -vV | awk '/^host: / { print $2 }')"
fi

VERSION="$(plutil -extract version raw -o - "$TAURI_CONFIG")"
EXTENSION_MINIMUM_VERSION="$(
  plutil -extract LSMinimumSystemVersion raw -o - "$SOURCE_ROOT/Info.plist"
)"

case "$TARGET" in
  aarch64-apple-darwin)
    COMPONENTS=("aarch64-apple-darwin")
    ;;
  x86_64-apple-darwin)
    COMPONENTS=("x86_64-apple-darwin")
    ;;
  universal-apple-darwin)
    COMPONENTS=("aarch64-apple-darwin" "x86_64-apple-darwin")
    ;;
  *)
    echo "Quick Look extension target is not a supported macOS target: $TARGET" >&2
    exit 2
    ;;
esac

rm -rf "$BUILD_ROOT" "$MODULE_CACHE" "$APPEX"
mkdir -p "$BUILD_ROOT" "$MODULE_CACHE"

component_executable() {
  local component="$1"
  local swift_target
  case "$component" in
    aarch64-apple-darwin)
      swift_target="arm64-apple-macosx$EXTENSION_MINIMUM_VERSION"
      ;;
    x86_64-apple-darwin)
      swift_target="x86_64-apple-macosx$EXTENSION_MINIMUM_VERSION"
      ;;
    *)
      echo "Unsupported Quick Look component target: $component" >&2
      return 2
      ;;
  esac

  local output="$BUILD_ROOT/SquallzQuickLook-$component"
  local rust_library="$ROOT/target/$component/release/libsquallz_quicklook.a"

  if ! MACOSX_DEPLOYMENT_TARGET="$EXTENSION_MINIMUM_VERSION" cargo build \
    --manifest-path "$ROOT/Cargo.toml" \
    --package squallz-quicklook \
    --release \
    --target "$component"; then
    echo "Quick Look Rust build failed for $component" >&2
    return 1
  fi

  if [[ ! -f "$rust_library" ]]; then
    echo "Quick Look Rust library is missing: $rust_library" >&2
    return 2
  fi

  # The Rust static archive uses the SDK-provided liblzma implementation.
  if ! CLANG_MODULE_CACHE_PATH="$MODULE_CACHE" \
    SWIFT_MODULE_CACHE_PATH="$MODULE_CACHE" \
    MACOSX_DEPLOYMENT_TARGET="$EXTENSION_MINIMUM_VERSION" \
    xcrun swiftc \
      -application-extension \
      -emit-executable \
      -module-name SquallzQuickLook \
      -Osize \
      -parse-as-library \
      -target "$swift_target" \
      -whole-module-optimization \
      "$SOURCE_ROOT/PreviewProvider.swift" \
      "$rust_library" \
      -llzma \
      -framework CoreGraphics \
      -framework Foundation \
      -framework QuickLookUI \
      -framework UniformTypeIdentifiers \
      -Xlinker -dead_strip \
      -Xlinker -e \
      -Xlinker _NSExtensionMain \
      -o "$output"; then
    echo "Quick Look Swift link failed for $component" >&2
    return 1
  fi

  printf '%s\n' "$output"
}

COMPONENT_EXECUTABLES=()
for component in "${COMPONENTS[@]}"; do
  if ! built_component="$(component_executable "$component")"; then
    exit 1
  fi
  COMPONENT_EXECUTABLES+=("$built_component")
done

mkdir -p "$APPEX/Contents/MacOS" "$APPEX/Contents/Resources"
cp "$SOURCE_ROOT/Info.plist" "$APPEX/Contents/Info.plist"
cp -R "$SOURCE_ROOT/en.lproj" "$APPEX/Contents/Resources/en.lproj"
cp -R "$SOURCE_ROOT/zh-Hans.lproj" "$APPEX/Contents/Resources/zh-Hans.lproj"

plutil -replace CFBundleShortVersionString -string "$VERSION" "$APPEX/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$APPEX/Contents/Info.plist"
plutil -replace LSMinimumSystemVersion \
  -string "$EXTENSION_MINIMUM_VERSION" \
  "$APPEX/Contents/Info.plist"
plutil -lint "$APPEX/Contents/Info.plist"

if [[ "${#COMPONENT_EXECUTABLES[@]}" -eq 1 ]]; then
  cp "${COMPONENT_EXECUTABLES[0]}" "$EXECUTABLE"
else
  lipo -create "${COMPONENT_EXECUTABLES[@]}" -output "$EXECUTABLE"
fi
chmod 0755 "$EXECUTABLE"

FORBIDDEN_SYMBOLS=()
while IFS= read -r symbol_line; do
  symbol="${symbol_line##* }"
  case "$symbol" in
    _fork|_vfork|_exec*|_posix_spawn*|_wait|_waitpid|_popen|_system|_pipe|_pipe2|'_OBJC_CLASS_$_NSTask')
      FORBIDDEN_SYMBOLS+=("$symbol")
      ;;
  esac
done < <(nm -u "$EXECUTABLE")
if [[ "${#FORBIDDEN_SYMBOLS[@]}" -ne 0 ]]; then
  printf 'Quick Look executable imports forbidden process symbols:\n' >&2
  printf '  %s\n' "${FORBIDDEN_SYMBOLS[@]}" >&2
  exit 1
fi

SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
SIGNING_ARGS=(
  --force
  --options runtime
  --entitlements "$SOURCE_ROOT/SquallzQuickLook.entitlements"
  --sign "$SIGNING_IDENTITY"
)
if [[ "$SIGNING_IDENTITY" != "-" ]]; then
  SIGNING_ARGS+=(--timestamp)
fi

codesign "${SIGNING_ARGS[@]}" "$APPEX"
codesign --verify --strict --verbose=2 "$APPEX"
