#!/usr/bin/env bash
# Generate updater.json with SHA-256 checksums for all release assets.
# Expects env vars: TAG, REPO
set -euo pipefail

TAG="${TAG:?TAG is required}"
REPO="${REPO:?REPO is required}"

VERSION="${TAG#v}"
PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"

# Download all release artifacts to compute SHA-256 checksums.
mkdir -p dl
declare -A FILES=(
  ["windows-x86_64"]="gemacast-pc-x86_64-pc-windows-msvc.msi"
  ["darwin-x86_64"]="gemacast-pc-x64-apple-darwin.dmg"
  ["darwin-aarch64"]="gemacast-pc-aarch64-apple-darwin.dmg"
  ["darwin-universal"]="gemacast-pc-universal-apple-darwin.dmg"
  ["linux-x86_64"]="gemacast-pc-${VERSION}-x86_64.AppImage"
  ["linux-aarch64"]="gemacast-pc-${VERSION}-aarch64.AppImage"
  ["android"]="gemacast-mobile.apk"
)

declare -A SIGS=(
  ["windows-x86_64"]="gemacast-pc-x86_64-pc-windows-msvc.msi.sig"
  ["darwin-x86_64"]="gemacast-pc-x64-apple-darwin.dmg.sig"
  ["darwin-aarch64"]="gemacast-pc-aarch64-apple-darwin.dmg.sig"
  ["darwin-universal"]="gemacast-pc-universal-apple-darwin.dmg.sig"
  ["linux-x86_64"]="gemacast-pc-${VERSION}-x86_64.AppImage.sig"
  ["linux-aarch64"]="gemacast-pc-${VERSION}-aarch64.AppImage.sig"
  ["android"]="gemacast-mobile.apk.sig"
)

# Download each artifact and compute its SHA-256 hash.
declare -A HASHES
for platform in "${!FILES[@]}"; do
  file="${FILES[$platform]}"
  if gh release download "$TAG" --pattern "$file" --dir dl --repo "$REPO" 2>/dev/null; then
    HASHES[$platform]=$(sha256sum "dl/$file" | cut -d' ' -f1)
    echo "SHA-256 for $platform ($file): ${HASHES[$platform]}"
  else
    echo "WARNING: Could not download $file for $platform — omitting sha256"
    HASHES[$platform]=""
  fi
done

# Build the JSON using jq for proper escaping
jq -n \
  --arg version "$VERSION" \
  --arg pub_date "$PUB_DATE" \
  --arg base_url "$BASE_URL" \
  --arg win_file "${FILES[windows-x86_64]}" \
  --arg win_sig "${SIGS[windows-x86_64]}" \
  --arg win_hash "${HASHES[windows-x86_64]}" \
  --arg dar_x64_file "${FILES[darwin-x86_64]}" \
  --arg dar_x64_sig "${SIGS[darwin-x86_64]}" \
  --arg dar_x64_hash "${HASHES[darwin-x86_64]}" \
  --arg dar_arm_file "${FILES[darwin-aarch64]}" \
  --arg dar_arm_sig "${SIGS[darwin-aarch64]}" \
  --arg dar_arm_hash "${HASHES[darwin-aarch64]}" \
  --arg dar_uni_file "${FILES[darwin-universal]}" \
  --arg dar_uni_sig "${SIGS[darwin-universal]}" \
  --arg dar_uni_hash "${HASHES[darwin-universal]}" \
  --arg lin_x64_file "${FILES[linux-x86_64]}" \
  --arg lin_x64_sig "${SIGS[linux-x86_64]}" \
  --arg lin_x64_hash "${HASHES[linux-x86_64]}" \
  --arg lin_arm_file "${FILES[linux-aarch64]}" \
  --arg lin_arm_sig "${SIGS[linux-aarch64]}" \
  --arg lin_arm_hash "${HASHES[linux-aarch64]}" \
  --arg android_file "${FILES[android]}" \
  --arg android_sig "${SIGS[android]}" \
  --arg android_hash "${HASHES[android]}" \
  '{
    version: $version,
    pub_date: $pub_date,
    platforms: {
      "windows-x86_64": { url: "\($base_url)/\($win_file)", signature: "\($base_url)/\($win_sig)", sha256: $win_hash },
      "darwin-x86_64":  { url: "\($base_url)/\($dar_x64_file)", signature: "\($base_url)/\($dar_x64_sig)", sha256: $dar_x64_hash },
      "darwin-aarch64": { url: "\($base_url)/\($dar_arm_file)", signature: "\($base_url)/\($dar_arm_sig)", sha256: $dar_arm_hash },
      "darwin-universal": { url: "\($base_url)/\($dar_uni_file)", signature: "\($base_url)/\($dar_uni_sig)", sha256: $dar_uni_hash },
      "linux-x86_64":   { url: "\($base_url)/\($lin_x64_file)", signature: "\($base_url)/\($lin_x64_sig)", sha256: $lin_x64_hash },
      "linux-aarch64":  { url: "\($base_url)/\($lin_arm_file)", signature: "\($base_url)/\($lin_arm_sig)", sha256: $lin_arm_hash },
      "android":        { url: "\($base_url)/\($android_file)", signature: "\($base_url)/\($android_sig)", sha256: $android_hash }
    }
  }' > updater.json

echo "Generated updater.json:"
cat updater.json
