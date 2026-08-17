#!/usr/bin/env bash

# Creates an empty _bundle/ so `tauri dev` and `cargo check` can run.
#
# tauri.conf.json declares ../_bundle/app and ../_bundle/ruby as bundle resources, and
# tauri-build refuses to configure at all when a declared resource path is missing — for dev
# builds too, not just releases. Without this, every developer would have to sit through a
# full Ruby compile (scripts/bundle_desktop.sh) before they could run the app at all.
#
# The directories are deliberately EMPTY. lib.rs looks for <resource_dir>/ruby/bin/ruby and
# falls back to the system `bundle exec` when it is absent, which is exactly what a dev build
# should do. Putting a real Ruby here is bundle_desktop.sh's job.

set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
bundle_dir="$repo_dir/_bundle"

# Never clobber a real bundle: bundle_desktop.sh puts a Ruby at ruby/bin/ruby, and silently
# replacing it with an empty tree would turn a self-contained build into one that quietly
# falls back to whatever Ruby the launching shell happened to have.
if [ -x "$bundle_dir/ruby/bin/ruby" ]; then
  echo "==> _bundle already holds a built Ruby; leaving it alone"
  exit 0
fi

mkdir -p "$bundle_dir/app" "$bundle_dir/ruby"
echo "==> Created empty _bundle/ placeholders for dev builds"
