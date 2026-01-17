#!/bin/bash

# Version bump script for Deltabadger
# Updates version in: umbrel-app.yml, Cargo.toml, tauri.conf.json
# Creates a signed git tag and pushes it

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Files to update
UMBREL_APP="$ROOT_DIR/deltabadger/umbrel-app.yml"
CARGO_TOML="$ROOT_DIR/src-tauri/Cargo.toml"
TAURI_CONF="$ROOT_DIR/src-tauri/tauri.conf.json"

# Get current version from Cargo.toml
CURRENT_VERSION=$(grep -m1 '^version = ' "$CARGO_TOML" | sed 's/version = "\(.*\)"/\1/')

echo "Current version: $CURRENT_VERSION"

# Parse version components
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

# Determine new version
if [ -n "$1" ]; then
    NEW_VERSION="$1"
else
    # Auto-increment patch version
    NEW_PATCH=$((PATCH + 1))
    NEW_VERSION="$MAJOR.$MINOR.$NEW_PATCH"
fi

echo "New version: $NEW_VERSION"
echo ""

# Confirm with user
read -p "Proceed with version bump? (y/n) " -n 1 -r
echo ""
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 1
fi

# Update umbrel-app.yml
echo "Updating $UMBREL_APP..."
sed -i '' "s/^version: \"$CURRENT_VERSION\"/version: \"$NEW_VERSION\"/" "$UMBREL_APP"

# Update Cargo.toml (only the package version, not dependencies)
echo "Updating $CARGO_TOML..."
sed -i '' "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" "$CARGO_TOML"

# Update tauri.conf.json
echo "Updating $TAURI_CONF..."
sed -i '' "s/\"version\": \"$CURRENT_VERSION\"/\"version\": \"$NEW_VERSION\"/" "$TAURI_CONF"

echo ""
echo "Files updated successfully!"
echo ""

# Show git diff
echo "Changes:"
git diff --color "$UMBREL_APP" "$CARGO_TOML" "$TAURI_CONF"
echo ""

# Ask about git operations
read -p "Commit, tag v$NEW_VERSION, and push to main? (y/n) " -n 1 -r
echo ""
if [[ $REPLY =~ ^[Yy]$ ]]; then
    git add "$UMBREL_APP" "$CARGO_TOML" "$TAURI_CONF"
    git commit -m "Bump version to $NEW_VERSION"
    git push origin main
    git tag -s "v$NEW_VERSION" -m "v$NEW_VERSION"
    git push origin "v$NEW_VERSION"
    echo ""
    echo "Done! Version $NEW_VERSION committed, tagged, and pushed."
else
    echo ""
    echo "Files updated locally. Git operations skipped."
    echo "To manually commit, tag, and push:"
    echo "  git add -A && git commit -m \"Bump version to $NEW_VERSION\""
    echo "  git push origin main"
    echo "  git tag -s v$NEW_VERSION -m \"v$NEW_VERSION\""
    echo "  git push origin v$NEW_VERSION"
fi
