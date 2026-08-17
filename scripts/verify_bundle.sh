#!/bin/bash

set -euo pipefail

repo_dir=$(cd "$(dirname "$0")/.." && pwd)
bundle_dir=${1:-"$repo_dir/_bundle"}
app_dir="$bundle_dir/app"
ruby_dir="$bundle_dir/ruby"
ruby_bin="$ruby_dir/bin/ruby"

fail() {
  echo "bundle verification failed: $*" >&2
  exit 1
}

require_file() {
  [ -f "$1" ] || fail "missing $1"
}

require_file "$ruby_bin"
require_file "$app_dir/Gemfile"
require_file "$app_dir/Gemfile.lock"
require_file "$app_dir/bin/rails"
require_file "$app_dir/src-tauri/Cargo.toml"

if find "$app_dir" -name '.env*' -print -quit | grep -q .; then
  fail "an environment file was copied into the app bundle"
fi

host_arch=$(uname -m)
ruby_arch=$(file "$ruby_bin")
case "$ruby_arch" in
  *"$host_arch"*) ;;
  *) fail "bundled Ruby is not for this host ($host_arch): $ruby_arch" ;;
esac

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/deltabadger-bundle-verify.XXXXXX")
server_pid=
cleanup() {
  if [ -n "$server_pid" ] && kill -0 "$server_pid" 2>/dev/null; then
    kill -TERM "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

# Run from a new location so Ruby cannot accidentally rely on the path it was built at.
relocated_bundle="$tmp_dir/relocated bundle"
cp -R "$bundle_dir" "$relocated_bundle"
app_dir="$relocated_bundle/app"
ruby_dir="$relocated_bundle/ruby"
ruby_bin="$ruby_dir/bin/ruby"
app_data="$tmp_dir/Application Support/Deltabadger"
mkdir -p "$app_data/db" "$app_data/tmp/cache"
mkdir -p "$tmp_dir/home"

expected_ruby_version=$(tr -d '[:space:]' < "$app_dir/.ruby-version")
actual_ruby_version=$(env -i PATH=/usr/bin:/bin HOME="$tmp_dir/home" \
  "$ruby_bin" -e 'print RUBY_VERSION')
[ "$actual_ruby_version" = "$expected_ruby_version" ] || \
  fail "expected Ruby $expected_ruby_version, got $actual_ruby_version"

ruby_prefix=$(env -i PATH=/usr/bin:/bin HOME="$tmp_dir/home" \
  "$ruby_bin" -rrbconfig -e 'print RbConfig::CONFIG.fetch("prefix")')
[ "$ruby_prefix" = "$ruby_dir" ] || \
  fail "Ruby prefix is not relocatable: $ruby_prefix (expected $ruby_dir)"

common_env=(
  PATH=/usr/bin:/bin
  HOME="$tmp_dir/home"
  RAILS_ENV=production
  RAILS_LOG_TO_STDOUT=true
  RAILS_SERVE_STATIC_FILES=1
  RAILS_MAX_THREADS=1
  SOLID_QUEUE_IN_PUMA=true
  SECRET_KEY_BASE=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789
  BUNDLE_GEMFILE="$app_dir/Gemfile"
  BUNDLE_PATH="$app_dir/vendor/bundle"
  GEM_HOME="$app_dir/vendor/bundle"
  DYLD_FALLBACK_LIBRARY_PATH="$ruby_dir/lib"
  DATABASE_PATH="$app_data/db/production.sqlite3"
  QUEUE_DATABASE_PATH="$app_data/db/production_queue.sqlite3"
  CACHE_DATABASE_PATH="$app_data/db/production_cache.sqlite3"
  CABLE_DATABASE_PATH="$app_data/db/production_cable.sqlite3"
  PIDFILE="$app_data/tmp/server.pid"
  BOOTSNAP_CACHE_DIR="$app_data/tmp/cache"
  APP_TMP_DIR="$app_data/tmp"
)

verified_features="runner, databases, and /health-check"
if [ -f "$ruby_dir/lib/libsodium.dylib" ]; then
  env -i "${common_env[@]}" "$ruby_bin" -rrbnacl -e \
    'abort "libsodium unavailable" unless RbNaCl::Sodium::Version::STRING' \
    || fail "rbnacl could not load the bundled libsodium"
  verified_features="runner, libsodium, databases, and /health-check"
else
  echo "bundle verification note: libsodium was not vendored; skipping the optional RbNaCl probe"
fi

runner_output=$(env -i "${common_env[@]}" APP_ROOT_URL=http://127.0.0.1:3099 \
  "$ruby_bin" "$app_dir/bin/rails" runner \
  'puts "bundle-runner-ok Rails=#{Rails.version} App=#{Rails.application.config.version}"')
echo "$runner_output"
case "$runner_output" in
  *"bundle-runner-ok Rails="*" App=0.0.0"*) fail "bundled version fell back to 0.0.0" ;;
  *"bundle-runner-ok Rails="*) ;;
  *) fail "production Rails runner did not boot" ;;
esac

env -i "${common_env[@]}" APP_ROOT_URL=http://127.0.0.1:3099 \
  "$ruby_bin" "$app_dir/bin/rails" db:prepare

for database in production.sqlite3 production_queue.sqlite3 production_cache.sqlite3 production_cable.sqlite3; do
  require_file "$app_data/db/$database"
done

env -i "${common_env[@]}" APP_ROOT_URL=http://127.0.0.1:3099 PORT=3099 \
  "$ruby_bin" "$app_dir/bin/rails" server -p 3099 -b 127.0.0.1 \
  >"$tmp_dir/rails.log" 2>&1 &
server_pid=$!

healthy=false
for _ in $(seq 1 120); do
  if /usr/bin/curl --fail --silent --output /dev/null http://127.0.0.1:3099/health-check; then
    healthy=true
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    sed -n '1,240p' "$tmp_dir/rails.log" >&2
    fail "bundled Rails server exited before becoming healthy"
  fi
  sleep 0.5
done
[ "$healthy" = true ] || {
  sed -n '1,240p' "$tmp_dir/rails.log" >&2
  fail "health check did not return 200"
}

open_files=$(/usr/sbin/lsof -a -p "$server_pid" -Fn 2>/dev/null || true)
case "$open_files" in
  *"$app_data/db/production.sqlite3"*) ;;
  *) fail "Puma does not have the primary SQLite database open under app data" ;;
esac
case "$open_files" in
  *"$relocated_bundle/app/storage/"*) fail "Puma opened a SQLite file in the read-only app tree" ;;
esac

echo "bundle verification passed: $verified_features"
