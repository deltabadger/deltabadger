#!/bin/bash

set -euo pipefail

# Relocatable-Ruby choice: build the repository's exact Ruby with ruby-build and
# --enable-load-relative, then vendor and rewrite every non-system Mach-O dylib.
# Verify the result (including native gems) with:
#   find _bundle/ruby _bundle/app/vendor/bundle -type f -print0 | while IFS= read -r -d '' f; do file "$f" | grep -q 'Mach-O' && otool -L "$f"; done | grep -E '/(opt/homebrew|usr/local|\.rbenv|\.asdf)/'
# The command must print nothing; any match falsifies the relocatability claim.

RUBY_VERSION=3.4.8

repo_dir=$(cd "$(dirname "$0")/.." && pwd)
bundle_dir="$repo_dir/_bundle"
app_dir="$bundle_dir/app"
ruby_dir="$bundle_dir/ruby"

fail() {
  echo "desktop bundle failed: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

[ "$(uname -s)" = Darwin ] || fail "macOS is required"

host_arch=$(uname -m)
requested_arch=${1:-${TAURI_ENV_ARCH:-$host_arch}}
case "$requested_arch" in
  arm64|aarch64) target_arch=arm64 ;;
  x86_64|x64) target_arch=x86_64 ;;
  *) fail "unsupported target architecture: $requested_arch" ;;
esac
[ "$host_arch" = "$target_arch" ] || fail \
  "cannot build $target_arch on $host_arch: rbsecp256k1 is compiled for the build host"

for command_name in file install_name_tool lipo npm otool python3 ruby ruby-build; do
  require_command "$command_name"
done

[ "$(tr -d '[:space:]' < "$repo_dir/.ruby-version")" = "$RUBY_VERSION" ] || \
  fail ".ruby-version does not contain Ruby $RUBY_VERSION"
grep -Eq "^ruby ['\"]$RUBY_VERSION['\"]$" "$repo_dir/Gemfile" || \
  fail "Gemfile does not require Ruby $RUBY_VERSION"
grep -Eq "^  ruby $RUBY_VERSION$" "$repo_dir/Gemfile.lock" || \
  fail "Gemfile.lock does not lock Ruby $RUBY_VERSION"

case "$bundle_dir" in
  "$repo_dir/_bundle") ;;
  *) fail "refusing to replace unexpected bundle path: $bundle_dir" ;;
esac

echo "Building frontend assets..."
(cd "$repo_dir" && npm run build)

# _bundle is generated output. Rebuild it from scratch so stale gems or assets
# cannot leak from an earlier architecture/build.
rm -rf "$bundle_dir"

echo "Copying the Rails application..."
ruby "$repo_dir/scripts/copy_desktop_app.rb" "$repo_dir" "$app_dir"

echo "Building relocatable Ruby $RUBY_VERSION for $target_arch..."
RUBY_CONFIGURE_OPTS="${RUBY_CONFIGURE_OPTS:-} --enable-load-relative" \
  ruby-build "$RUBY_VERSION" "$ruby_dir"

ruby_archs=$(lipo -archs "$ruby_dir/bin/ruby")
case " $ruby_archs " in
  *" $target_arch "*) ;;
  *) fail "Ruby executable does not contain $target_arch: $ruby_archs" ;;
esac

bundler_version=$(awk '/^BUNDLED WITH$/ { getline; gsub(/^[[:space:]]+/, ""); print; exit }' \
  "$repo_dir/Gemfile.lock")
[ -n "$bundler_version" ] || fail "could not read the Bundler version from Gemfile.lock"
"$ruby_dir/bin/gem" install bundler --version "$bundler_version" --no-document

echo "Installing production gems..."
(
  cd "$app_dir"
  "$ruby_dir/bin/bundle" config set --local deployment true
  "$ruby_dir/bin/bundle" config set --local path vendor/bundle
  "$ruby_dir/bin/bundle" config set --local without 'development test'
  "$ruby_dir/bin/bundle" install
)

echo "Precompiling production assets..."
(
  cd "$app_dir"
  SECRET_KEY_BASE_DUMMY=1 \
  RAILS_ENV=production \
  APP_ROOT_URL=http://localhost:3000 \
  HOME_PAGE_URL=http://localhost:3000 \
  SMTP_ADDRESS=localhost \
  SMTP_DOMAIN=localhost \
  SMTP_PORT=25 \
  SMTP_USER_NAME=placeholder \
  SMTP_PASSWORD=placeholder \
  NOTIFICATIONS_SENDER=placeholder@example.com \
  COINGECKO_API_KEY=placeholder \
  ORDERS_FREQUENCY_LIMIT=60 \
  "$ruby_dir/bin/bundle" exec rails assets:precompile
)

# RbNaCl loads libsodium lazily, so it is not a Rails boot dependency. Vendor a
# Homebrew copy when available as inexpensive insurance for the JWT EdDSA path.
if command -v brew >/dev/null 2>&1 && sodium_prefix=$(brew --prefix libsodium 2>/dev/null); then
  if [ -f "$sodium_prefix/lib/libsodium.dylib" ]; then
    cp -L "$sodium_prefix/lib/libsodium.dylib" "$ruby_dir/lib/libsodium.dylib"
    install_name_tool -id '@rpath/libsodium.dylib' "$ruby_dir/lib/libsodium.dylib"
  fi
fi

is_macho() {
  local macho_description
  macho_description=$(file -b "$1")
  case "$macho_description" in
    *Mach-O*object*) return 1 ;;
    *Mach-O*) return 0 ;;
    *) return 1 ;;
  esac
}

is_system_dependency() {
  case "$1" in
    /System/Library/*|/usr/lib/*|@executable_path/*|@loader_path/*|@rpath/*) return 0 ;;
    *) return 1 ;;
  esac
}

normalize_dylib_id() {
  case "$(file -b "$1")" in
    *Mach-O*dynamically\ linked\ shared\ library*)
      install_name_tool -id "@rpath/$(basename "$1")" "$1"
      ;;
  esac
}

echo "Vendoring non-system dynamic libraries..."
scan_list=$(mktemp "${TMPDIR:-/tmp}/deltabadger-mach-o.XXXXXX")
trap 'rm -f "$scan_list"' EXIT INT TERM

# Repeat because each copied dylib can introduce another dependency.
while :; do
  find "$ruby_dir" "$app_dir/vendor/bundle" -type f -print0 > "$scan_list"
  copied=false
  while IFS= read -r -d '' macho; do
    is_macho "$macho" || continue
    normalize_dylib_id "$macho"
    while IFS= read -r dependency; do
      [ -n "$dependency" ] || continue
      is_system_dependency "$dependency" && continue
      [ "${dependency#/}" != "$dependency" ] || continue
      [ -f "$dependency" ] || fail "missing dynamic library $dependency (required by $macho)"

      dylib_name=$(basename "$dependency")
      destination="$ruby_dir/lib/$dylib_name"
      if [ ! -f "$destination" ]; then
        cp -L "$dependency" "$destination"
        install_name_tool -id "@rpath/$dylib_name" "$destination"
        copied=true
      fi
      install_name_tool -change "$dependency" "@rpath/$dylib_name" "$macho"
    done < <(otool -L "$macho" | tail -n +2 | awk '{print $1}')
  done < "$scan_list"
  [ "$copied" = false ] && break
done

# Give each Mach-O an rpath to the bundle's common dylib directory. This works
# for Ruby itself, its standard extensions, and native extensions in gems.
find "$ruby_dir" "$app_dir/vendor/bundle" -type f -print0 > "$scan_list"
while IFS= read -r -d '' macho; do
  is_macho "$macho" || continue
  while IFS= read -r old_rpath; do
    case "$old_rpath" in
      /*) install_name_tool -delete_rpath "$old_rpath" "$macho" ;;
    esac
  done < <(otool -l "$macho" | awk '$1 == "cmd" && $2 == "LC_RPATH" { getline; getline; print $2 }')
  relative_lib=$(python3 - "$macho" "$ruby_dir/lib" <<'PY'
import os
import sys
print(os.path.relpath(sys.argv[2], os.path.dirname(sys.argv[1])))
PY
)
  rpath_error=$(mktemp "${TMPDIR:-/tmp}/deltabadger-rpath.XXXXXX")
  if ! install_name_tool -add_rpath "@loader_path/$relative_lib" "$macho" 2>"$rpath_error"; then
    grep -q 'would duplicate path' "$rpath_error" || {
      sed -n '1,20p' "$rpath_error" >&2
      rm -f "$rpath_error"
      fail "could not add the bundled-library rpath to $macho"
    }
  fi
  rm -f "$rpath_error"
done < "$scan_list"

echo "Auditing bundled Mach-O paths and architectures..."
while IFS= read -r -d '' macho; do
  is_macho "$macho" || continue
  macho_archs=$(lipo -archs "$macho")
  case " $macho_archs " in
    *" $target_arch "*) ;;
    *) fail "$macho does not contain $target_arch: $macho_archs" ;;
  esac
  while IFS= read -r dependency; do
    case "$dependency" in
      /System/Library/*|/usr/lib/*|@executable_path/*|@loader_path/*|@rpath/*) ;;
      /*) fail "non-relocatable dependency in $macho: $dependency" ;;
    esac
  done < <(otool -L "$macho" | tail -n +2 | awk '{print $1}')
  while IFS= read -r bundled_rpath; do
    case "$bundled_rpath" in
      /System/Library/*|/usr/lib/*|@executable_path/*|@loader_path/*|@rpath/*) ;;
      /*) fail "non-relocatable rpath in $macho: $bundled_rpath" ;;
    esac
  done < <(otool -l "$macho" | awk '$1 == "cmd" && $2 == "LC_RPATH" { getline; getline; print $2 }')
done < "$scan_list"

rm -f "$scan_list"
trap - EXIT INT TERM

find "$app_dir" -name '.env*' -print -quit | grep -q . && \
  fail "an .env file was copied into the app bundle"
[ -x "$ruby_dir/bin/ruby" ] || fail "bundled Ruby is missing"
[ -f "$app_dir/bin/rails" ] || fail "bundled Rails launcher is missing"
[ -f "$app_dir/src-tauri/Cargo.toml" ] || fail "desktop version source is missing"

echo "Desktop resources are ready in $bundle_dir"
echo "Run scripts/verify_bundle.sh before building the Tauri application."
