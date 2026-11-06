#!/usr/bin/env bash
set -euo pipefail

profile_dir="target/release"
profile_suffix=""
if [[ "$PROFILE" == "debug" ]]; then
  profile_dir="target/debug"
  profile_suffix="-debug"
fi

out="target/release-assets"
tmp="target/release-packaging"
rm -rf "$out" "$tmp"
mkdir -p "$out" "$tmp"

copy_artifact() {
  local source="$1"
  local name="$2"
  local destination="$out/$name"
  if [[ -e "$destination" ]]; then
    echo "Artifact name collision: $name"
    exit 1
  fi
  cp "$source" "$destination"
}

copy_executable_archive() {
  local source="$1"
  local binary_name="$2"
  local archive_name="$3"
  local dir="$tmp/${archive_name%.tar.gz}"
  mkdir -p "$dir"
  cp "$source" "$dir/$binary_name"
  chmod +x "$dir/$binary_name"
  tar -C "$dir" -czf "$out/$archive_name" "$binary_name"
}

if [[ "${RUNNER_OS}" == "Windows" ]]; then
  shopt -s nullglob
  nsis_candidates=("$profile_dir"/bundle/nsis/*.exe)
  if (( ${#nsis_candidates[@]} != 1 )); then
    echo "Expected exactly one Windows NSIS installer, found ${#nsis_candidates[@]}" >&2
    exit 1
  fi
  copy_artifact \
    "$profile_dir/sqz.exe" \
    "sqz-${VERSION}-${PLATFORM}${profile_suffix}.exe"
  copy_artifact \
    "${nsis_candidates[0]}" \
    "Squallz-${VERSION}-${PLATFORM}${profile_suffix}.exe"
elif [[ "${RUNNER_OS}" != "macOS" || "$PUBLIC_RELEASE" != "true" ]]; then
  copy_executable_archive \
    "$profile_dir/sqz" \
    "sqz" \
    "sqz-${VERSION}-${PLATFORM}${profile_suffix}.tar.gz"
fi

if [[ "${RUNNER_OS}" == "macOS" ]]; then
  if [[ "$PUBLIC_RELEASE" == "true" ]]; then
    shopt -s nullglob
    dmg_candidates=("$profile_dir"/bundle/dmg/*.dmg)
    if (( ${#dmg_candidates[@]} != 1 )); then
      echo "Expected exactly one macOS DMG, found ${#dmg_candidates[@]}" >&2
      exit 1
    fi
    copy_artifact \
      "${dmg_candidates[0]}" \
      "Squallz-${VERSION}-${PLATFORM}.dmg"
  else
    app_path="$profile_dir/bundle/macos/Squallz.app"
    if [[ ! -d "$app_path" ]]; then
      echo "Expected macOS app bundle is missing: $app_path"
      exit 1
    fi
    ditto -c -k --sequesterRsrc --keepParent \
      "$app_path" \
      "$out/Squallz-${VERSION}-${PLATFORM}${profile_suffix}.app.zip"
  fi
elif [[ "${RUNNER_OS}" == "Linux" ]]; then
  shopt -s nullglob
  appimage_candidates=("$profile_dir"/bundle/appimage/*.AppImage)
  if (( ${#appimage_candidates[@]} != 1 )); then
    echo "Expected exactly one Linux AppImage, found ${#appimage_candidates[@]}" >&2
    exit 1
  fi
  copy_executable_archive \
    "${appimage_candidates[0]}" \
    "Squallz.AppImage" \
    "Squallz-${VERSION}-${PLATFORM}${profile_suffix}.tar.gz"
fi

bundle_dir="$profile_dir/bundle"
if [[ -d "$bundle_dir" ]] \
  && [[ "${RUNNER_OS}" != "macOS" || "$PUBLIC_RELEASE" != "true" ]]; then
  while IFS= read -r -d '' file; do
    if [[ "${RUNNER_OS}" == "Windows" && "$file" == "${nsis_candidates[0]}" ]]; then
      continue
    fi
    case "$file" in
      *.app/*) continue ;;
    esac
    case "$file" in
      *.AppImage|*.deb|*.dmg|*.exe|*.msi|*.rpm|*.tar.gz|*.zip)
        relative="${file#$bundle_dir/}"
        artifact_name="bundle-${PLATFORM}-${relative//\//-}"
        copy_artifact "$file" "$artifact_name"
        ;;
    esac
  done < <(find "$bundle_dir" -type f -print0)
fi
