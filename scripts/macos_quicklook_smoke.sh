#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:-"$ROOT/target/debug/bundle/macos/Squallz.app"}"
if [[ "$APP" != /* ]]; then
  APP="$ROOT/$APP"
fi

APPEX="$APP/Contents/PlugIns/SquallzQuickLook.appex"
EXECUTABLE="$APPEX/Contents/MacOS/SquallzQuickLook"
WORK="$ROOT/target/macos-quicklook-smoke"
RUNTIME_APP="$WORK/Squallz.app"
RUNTIME_APPEX="$RUNTIME_APP/Contents/PlugIns/SquallzQuickLook.appex"
SAMPLE="$WORK/sample.zip"
PREVIEW_DIR="$WORK/preview"
RUNTIME_LOG="$WORK/qlmanage.log"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
REQUIRE_RUNTIME="${SQUALLZ_QUICKLOOK_REQUIRE_RUNTIME:-0}"
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

fail() {
  echo "macos_quicklook_smoke: $*" >&2
  exit 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  fail "this smoke must run on macOS"
fi
[[ -d "$APP" && ! -L "$APP" ]] || fail "missing app bundle: $APP"
[[ -d "$APPEX" && ! -L "$APPEX" ]] || fail "missing Quick Look extension: $APPEX"
[[ -x "$EXECUTABLE" && ! -L "$EXECUTABLE" ]] || fail "missing Quick Look executable"

python3 - "$APP/Contents/Info.plist" "$APPEX/Contents/Info.plist" <<'PY'
import plistlib
import sys

with open(sys.argv[1], "rb") as handle:
    app = plistlib.load(handle)
with open(sys.argv[2], "rb") as handle:
    extension = plistlib.load(handle)

assert app["CFBundleIdentifier"] == "dev.squallz.desktop"
types = app.get("CFBundleDocumentTypes", [])
quicklook_archive_types = {
    "public.zip-archive",
    "com.sun.java-archive",
    "dev.squallz.archive.apk",
    "dev.squallz.archive.cbz",
    "com.apple.itunes.ipa",
    "org.7-zip.7-zip-archive",
    "public.tar-archive",
    "org.gnu.gnu-zip-tar-archive",
    "public.tar-bzip2-archive",
    "org.tukaani.tar-xz-archive",
    "dev.squallz.archive.tar-zstd",
}
app_archive_types = quicklook_archive_types | {
    "dev.squallz.archive.cbr",
    "dev.squallz.archive.rar",
    "dev.squallz.archive.split-volume",
    "dev.squallz.archive.wim",
    "dev.squallz.archive.split-wim",
}
stream_types = {
    "org.gnu.gnu-zip-archive",
    "public.bzip2-archive",
    "org.tukaani.xz-archive",
    "dev.squallz.stream.zstd",
    "dev.squallz.stream.lz4",
    "dev.squallz.stream.brotli",
}
assert any(
    item.get("LSItemContentTypes") == ["dev.squallz.sqz-archive"]
    and item.get("LSHandlerRank") == "Owner"
    for item in types
)
assert any(set(item.get("LSItemContentTypes", [])) == app_archive_types for item in types)
assert any(set(item.get("LSItemContentTypes", [])) == stream_types for item in types)
imported = app.get("UTImportedTypeDeclarations", [])
imported_ids = {item.get("UTTypeIdentifier") for item in imported}
assert imported_ids == {
    "dev.squallz.archive.apk",
    "dev.squallz.archive.cbz",
    "dev.squallz.archive.cbr",
    "dev.squallz.archive.rar",
    "dev.squallz.archive.tar-zstd",
    "dev.squallz.archive.split-volume",
    "dev.squallz.archive.wim",
    "dev.squallz.archive.split-wim",
    "dev.squallz.stream.zstd",
    "dev.squallz.stream.lz4",
    "dev.squallz.stream.brotli",
}
exported = app.get("UTExportedTypeDeclarations", [])
exported_ids = {item.get("UTTypeIdentifier") for item in exported}
assert exported_ids == {"dev.squallz.sqz-archive"}

assert extension["CFBundleIdentifier"] == "dev.squallz.desktop.quicklook"
assert extension["CFBundlePackageType"] == "XPC!"
assert extension["CFBundleInfoDictionaryVersion"] == "6.0"
assert extension["CFBundleSupportedPlatforms"] == ["MacOSX"]
assert extension["LSMinimumSystemVersion"] == "12.0"
assert extension["CFBundleShortVersionString"] == app["CFBundleShortVersionString"]
assert extension["CFBundleVersion"] == app["CFBundleVersion"]
contract = extension["NSExtension"]
assert contract["NSExtensionPointIdentifier"] == "com.apple.quicklook.preview"
assert contract["NSExtensionPrincipalClass"] == "SquallzQuickLook.PreviewProvider"
attributes = contract["NSExtensionAttributes"]
assert attributes["QLIsDataBasedPreview"] is True
assert attributes["QLSupportsSearchableItems"] is False
assert set(attributes["QLSupportedContentTypes"]) == (
    quicklook_archive_types | stream_types | {"dev.squallz.sqz-archive"}
)
PY

codesign --verify --strict --verbose=2 "$APPEX"

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
  printf 'Forbidden process imports:\n' >&2
  printf '  %s\n' "${FORBIDDEN_SYMBOLS[@]}" >&2
  exit 1
fi

APP_ARCHS="$(lipo -archs "$APP/Contents/MacOS/squallz-gui")"
EXTENSION_ARCHS="$(lipo -archs "$EXECUTABLE")"
[[ "$APP_ARCHS" == "$EXTENSION_ARCHS" ]] || {
  fail "app and Quick Look architectures differ: $APP_ARCHS vs $EXTENSION_ARCHS"
}

if [[ -z "$IDENTITY" ]]; then
  IDENTITY="$(
    security find-identity -v -p codesigning 2>/dev/null \
      | sed -nE 's/^[[:space:]]*[0-9]+\) [0-9A-F]+ "(Apple Development:|Developer ID Application:)([^"]+)"/\1\2/p' \
      | head -n 1
  )"
fi

if [[ -z "$IDENTITY" ]]; then
  if [[ "$REQUIRE_RUNTIME" == "1" ]]; then
    fail "runtime preview requires a valid Apple Development or Developer ID signing identity"
  fi
  echo "Quick Look structural smoke passed; runtime preview was not run because no valid Apple signing identity is installed."
  exit 0
fi

rm -rf "$WORK"
mkdir -p "$WORK" "$PREVIEW_DIR"
ditto "$APP" "$RUNTIME_APP"
python3 - "$SAMPLE" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1], "w", compression=zipfile.ZIP_DEFLATED) as archive:
    archive.writestr("hello.txt", "Finder preview generated by Squallz\n")
    archive.writestr("docs/readme.md", "# Squallz Quick Look\n")
PY

codesign --force --options runtime \
  --entitlements "$ROOT/native/macos/SquallzQuickLook/SquallzQuickLook.entitlements" \
  --sign "$IDENTITY" \
  "$RUNTIME_APPEX"
codesign --force --options runtime --sign "$IDENTITY" \
  "$RUNTIME_APP/Contents/MacOS/sqz"
codesign --force --options runtime --sign "$IDENTITY" \
  "$RUNTIME_APP/Contents/MacOS/squallz-gui"
codesign --force --options runtime --sign "$IDENTITY" "$RUNTIME_APP"
codesign --verify --deep --strict --verbose=2 "$RUNTIME_APP"

cleanup() {
  pluginkit -r "$RUNTIME_APPEX" >/dev/null 2>&1 || true
  "$LSREGISTER" -u "$RUNTIME_APP" >/dev/null 2>&1 || true
}
trap cleanup EXIT

"$LSREGISTER" -f "$RUNTIME_APP"
pluginkit -a "$RUNTIME_APPEX"
pluginkit -m -i dev.squallz.desktop.quicklook \
  | grep -Fq "dev.squallz.desktop.quicklook" \
  || fail "Quick Look extension did not register"
qlmanage -r cache >/dev/null 2>&1
if ! qlmanage -p -x -c public.zip-archive -o "$PREVIEW_DIR" "$SAMPLE" >"$RUNTIME_LOG" 2>&1; then
  fail "Quick Look preview generation failed; see $RUNTIME_LOG"
fi

python3 - "$PREVIEW_DIR" <<'PY'
import sys
from pathlib import Path

root = Path(sys.argv[1])
files = [path for path in root.rglob("*") if path.is_file() and path.stat().st_size]
assert files, "Quick Look generated no preview files"
payload = b"\n".join(path.read_bytes()[:4_000_000] for path in files)
assert b"Squallz" in payload, "preview output does not contain Squallz branding"
assert b"hello.txt" in payload, "preview output does not contain the archive entry"
PY

echo "Quick Look structural and runtime smoke passed."
