#!/usr/bin/env bash
# Rollback a failed release: delete GitHub release, delete tag, revert version bump.
# Version-only revert (CHANGELOGs are left untouched — release-please will regenerate).
# Expects env vars: TAG, REPO, GH_TOKEN (or GH_PAT for pushing to main)
set -euo pipefail

TAG="${TAG:?TAG is required}"
REPO="${REPO:?REPO is required}"

CURRENT_VERSION="${TAG#v}"

echo "=== ROLLBACK: Release pipeline failed ==="
echo "Tag: $TAG"
echo "Version: $CURRENT_VERSION"

# ── Step 1: Delete the GitHub Release ─────────────────────────────────────
echo ""
echo "Step 1: Deleting GitHub release for $TAG..."
gh release delete "$TAG" --yes --repo "$REPO" 2>/dev/null && echo "Release deleted." || echo "No release found for $TAG (already deleted or never created)."

# ── Step 2: Delete the remote tag ─────────────────────────────────────────
echo ""
echo "Step 2: Deleting remote tag $TAG..."
git push --delete origin "$TAG" 2>/dev/null && echo "Tag deleted from remote." || echo "Tag $TAG not found on remote."

# ── Step 3: Determine previous version ────────────────────────────────────
echo ""
echo "Step 3: Determining previous version..."
git fetch --tags origin

# Find the most recent tag that isn't the one we're rolling back
PREV_TAG=$(git tag --sort=-version:refname | grep -v "^${TAG}$" | head -1)
if [ -z "$PREV_TAG" ]; then
  echo "::warning::Could not determine previous version tag. Manual version revert required."
  echo "Rollback partially complete (release and tag deleted, versions NOT reverted)."
  exit 0
fi

PREV_VERSION="${PREV_TAG#v}"
echo "Previous version: $PREV_VERSION (from tag $PREV_TAG)"

# ── Step 4: Revert version files on main ──────────────────────────────────
echo ""
echo "Step 4: Reverting version files from $CURRENT_VERSION → $PREV_VERSION..."

git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"

git fetch origin main
git checkout main

# Update Cargo.toml workspace version
sed -i "s/version = \"$CURRENT_VERSION\"/version = \"$PREV_VERSION\"/" Cargo.toml

# Update version.txt
echo "$PREV_VERSION" > version.txt

# Update package.json
if [ -f gemacast-mobile/package.json ]; then
  jq --arg v "$PREV_VERSION" '.version = $v' gemacast-mobile/package.json > tmp.json && mv tmp.json gemacast-mobile/package.json
fi

# Update tauri.conf.json
if [ -f gemacast-mobile/src-tauri/tauri.conf.json ]; then
  jq --arg v "$PREV_VERSION" '.version = $v' gemacast-mobile/src-tauri/tauri.conf.json > tmp.json && mv tmp.json gemacast-mobile/src-tauri/tauri.conf.json
fi

# Update release-please manifest
if [ -f .release-please-manifest.json ]; then
  jq --arg v "$PREV_VERSION" '.["."] = $v' .release-please-manifest.json > tmp.json && mv tmp.json .release-please-manifest.json
fi

# ── Step 5: Commit and push ───────────────────────────────────────────────
echo ""
echo "Step 5: Committing and pushing revert..."
git add -A
git commit -m "chore: revert version ${CURRENT_VERSION} → ${PREV_VERSION} (release pipeline failed)"
git push origin main

echo ""
echo "=== ROLLBACK COMPLETE ==="
echo "  - GitHub release for $TAG: DELETED"
echo "  - Remote tag $TAG: DELETED"
echo "  - Version reverted: $CURRENT_VERSION → $PREV_VERSION"
echo ""
echo "The next release-please run will re-create the version bump PR."
