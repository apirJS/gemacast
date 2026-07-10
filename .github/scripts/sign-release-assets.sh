#!/usr/bin/env bash
# GPG sign all release assets.
# Expects env vars: GPG_PASSPHRASE
# Must be run from the directory containing the downloaded assets.
set -euo pipefail

GPG_PASSPHRASE="${GPG_PASSPHRASE:?GPG_PASSPHRASE is required}"

cd assets
SIGNED=0
for file in *.msi *.deb *.rpm *.AppImage *.dmg *.app.tar.gz *.tar.xz *.zip *.apk; do
  if [ -f "$file" ]; then
    echo "$GPG_PASSPHRASE" | gpg --batch --yes --pinentry-mode loopback \
      --passphrase-fd 0 \
      --detach-sign --armor --output "${file}.sig" "$file"
    SIGNED=$((SIGNED+1))
  fi
done

if [ "$SIGNED" -eq 0 ]; then
  echo "::error::No files were signed — check that release assets were downloaded"
  exit 1
fi
echo "Signed $SIGNED file(s)"
