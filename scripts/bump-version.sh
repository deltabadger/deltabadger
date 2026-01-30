#!/bin/bash

# Version bump script for Deltabadger
# Updates version in: umbrel-app.yml, Cargo.toml, tauri.conf.json
# Source of truth: Cargo.toml (Rails reads from there via config/initializers/version.rb)
#
# Usage:
#   ./bump-version.sh           # Bump patch (1.0.0 → 1.0.1)
#   ./bump-version.sh patch     # Bump patch (1.0.0 → 1.0.1)
#   ./bump-version.sh minor     # Bump minor (1.0.5 → 1.1.0)
#   ./bump-version.sh major     # Bump major (1.2.5 → 2.0.0)
#   ./bump-version.sh 1.2.3     # Set explicit version

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"

# Files to update
UMBREL_APP="$ROOT_DIR/deltabadger/umbrel-app.yml"
CARGO_TOML="$ROOT_DIR/src-tauri/Cargo.toml"
TAURI_CONF="$ROOT_DIR/src-tauri/tauri.conf.json"

# Get current version from Cargo.toml (source of truth)
CURRENT_VERSION=$(grep -m1 '^version = ' "$CARGO_TOML" | sed 's/version = "\(.*\)"/\1/')

echo "Current version: $CURRENT_VERSION"

# Parse version components
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

# Determine new version based on argument
case "${1:-patch}" in
    major)
        NEW_VERSION="$((MAJOR + 1)).0.0"
        ;;
    minor)
        NEW_VERSION="$MAJOR.$((MINOR + 1)).0"
        ;;
    patch)
        NEW_VERSION="$MAJOR.$MINOR.$((PATCH + 1))"
        ;;
    *)
        # Explicit version provided (e.g., 1.2.3)
        if [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            NEW_VERSION="$1"
        else
            echo "Error: Invalid version format '$1'"
            echo "Usage: $0 [major|minor|patch|X.Y.Z]"
            exit 1
        fi
        ;;
esac

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
