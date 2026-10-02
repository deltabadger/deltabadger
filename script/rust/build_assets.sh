#!/usr/bin/env bash
# Builds the two files the Rust crate embeds, without Ruby: app/assets/builds/application.js (bun,
# the `build` script of package.json) and app/assets/builds/application.css (dart-sass, the command
# dartsass-rails runs). rust/build.rs fingerprints and embeds them; run this before `cargo build`.
#   script/rust/build_assets.sh
# dart-sass comes from npm at the version Gemfile.lock pins for the Rails build, so both builds
# compile the stylesheet with the same compiler.
set -euo pipefail
cd "$(dirname "$0")/../.."
bun install --frozen-lockfile
bun run build
sass_version=$(sed -n 's/^    sass-embedded (\([0-9][0-9.]*\)-.*/\1/p' Gemfile.lock | head -n 1)
[ -n "$sass_version" ] || { echo "sass-embedded is not in Gemfile.lock" >&2; exit 1; }
bunx "sass@$sass_version" --style=compressed --no-source-map \
  --load-path app/assets/stylesheets --load-path node_modules --load-path app/assets/fonts \
  app/assets/stylesheets/application.scss:app/assets/builds/application.css
ls -l app/assets/builds/application.js app/assets/builds/application.css
