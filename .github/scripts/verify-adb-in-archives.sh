#!/usr/bin/env bash
# Verify ADB is bundled inside every cargo-dist archive.
# Expects: MANIFEST_PATH (path to dist-manifest.json)
set -euo pipefail

MANIFEST_PATH="${MANIFEST_PATH:?MANIFEST_PATH is required}"

for archive in $(dist print-upload-files-from-manifest --manifest "$MANIFEST_PATH"); do
  if [[ "$archive" == *.zip ]]; then
    # Buffer listing to avoid broken pipe with pipefail
    listing=$(unzip -l "$archive" 2>/dev/null || true)
    if ! echo "$listing" | grep -q 'adb'; then
      echo "::error::ADB not found in $archive"
      exit 1
    fi
  elif [[ "$archive" == *.tar.gz ]] || [[ "$archive" == *.tar.xz ]]; then
    listing=$(tar -tf "$archive" 2>/dev/null || true)
    if ! echo "$listing" | grep -q 'adb'; then
      echo "::error::ADB not found in $archive"
      exit 1
    fi
  fi
done

echo "ADB successfully verified in all archives!"
