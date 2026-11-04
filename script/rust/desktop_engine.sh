#!/usr/bin/env bash
# Builds the engine the desktop app runs as its sidecar (PLAN-B6 §4.1, r1.13) and copies it where Tauri's externalBin
# looks for it: src-tauri/binaries/deltabadger-engine-<target triple>[.exe]. Fetches the catalogue bundle the app ships
# (db/seed_data/catalogue.json.gz) when it is missing. tauri.conf.json runs it before every `tauri build` and every
# `tauri dev`, so the app always carries a fresh engine of the profile it was built with.
#   script/rust/desktop_engine.sh [--debug] [<target triple>]
# Release unless --debug is given or Tauri builds a debug app (TAURI_ENV_DEBUG=true). The triple is the argument, else
# Tauri's TAURI_ENV_TARGET_TRIPLE, else the host's. CARGO names the cargo to run (default: cargo).
# Bash 3.2 (macOS) safe.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo=${CARGO:-cargo}
profile=release
if [ "${1:-}" = --debug ]; then profile=debug; shift; fi
if [ "${TAURI_ENV_DEBUG:-}" = true ]; then profile=debug; fi
triple=${1:-${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}}
[ -n "$triple" ] || { echo "desktop_engine.sh: no target triple" >&2; exit 1; }

exe=
case "$triple" in
  *-apple-darwin) export MACOSX_DEPLOYMENT_TARGET=11.3 ;; # tauri.conf.json's minimumSystemVersion
  # The C runtime linked in, so the engine needs no Visual C++ redistributable (PLAN-B6 r1.2).
  *-windows-msvc) exe=.exe; export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" ;;
  *-windows-*) exe=.exe ;;
esac
release_flag=
if [ "$profile" = release ]; then release_flag=--release; fi

# The web UI the engine embeds (release builds refuse to build without it, rust/build.rs).
script/rust/build_assets.sh
# Always with --target: the engine's output lands in <target dir>/<triple>/, apart from the desktop shell's.
(cd rust && "$cargo" build --locked $release_flag --target "$triple" --bin deltabadger)
target_dir=$(cd rust && "$cargo" metadata --format-version 1 --no-deps | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
mkdir -p src-tauri/binaries
sidecar="src-tauri/binaries/deltabadger-engine-$triple$exe"
cp "$target_dir/$triple/$profile/deltabadger$exe" "$sidecar"

catalogue=db/seed_data/catalogue.json.gz
if [ ! -s "$catalogue" ]; then
  curl -fsSL --retry 5 --retry-all-errors --max-filesize 33554432 -o "$catalogue.partial" \
    https://data.deltabadger.com/public/v1/catalogue.json.gz
  mv "$catalogue.partial" "$catalogue"
fi
gzip -t "$catalogue"
ls -l "$sidecar" "$catalogue"
