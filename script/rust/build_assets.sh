#!/usr/bin/env bash
# Builds the two files the Rust crate embeds, without Ruby: rust/web/builds/application.js (bun,
# the `build` script of package.json) and rust/web/builds/application.css (dart-sass, the command
# dartsass-rails runs). rust/build.rs fingerprints and embeds them; run this before `cargo build`.
#   script/rust/build_assets.sh
# dart-sass is package.json's exact devDependency (the version the Rails build's sass-embedded gem
# pinned, so the CSS is the same bytes), installed by the bun install below.
set -euo pipefail
cd "$(dirname "$0")/../.."
bun install --frozen-lockfile
bun run build
bun x --no-install sass --style=compressed --no-source-map \
  --load-path rust/web/stylesheets --load-path node_modules --load-path rust/web/fonts \
  rust/web/stylesheets/application.scss:rust/web/builds/application.css
ls -l rust/web/builds/application.js rust/web/builds/application.css
