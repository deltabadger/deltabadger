#!/usr/bin/env bash
# The release's rules (PLAN-B8 §1.2, r1.5–r2.1), as one script so the rule that publishes is the rule that is tested
# (script/release/validate_test.sh, run by rust.yml). release.yml calls each subcommand; script/release.sh uses
# `version` and `set-version`.
#
#   validate.sh version                         the release version: src-tauri/Cargo.toml's
#   validate.sh set-version <v>                 write <v> into src-tauri/Cargo.toml, tauri.conf.json and Cargo.lock
#   validate.sh same-version <tag>              fail unless both version files say <tag> without its `v`
#   validate.sh feed <dir> <tag> <critical> <base url>   write <dir>/latest.json from the three updater packages
#   validate.sh check <dir> <tag> <signed|unsigned> <pubkey>   the desktop assets a release publishes, or why not
#   validate.sh highest <series> <tag> <tag>...  `true` when <tag> is the highest stable tag of <series> (e.g. v3, v3.1)
#   validate.sh tags <tag> <latest_major> <tag>...   the image tags to publish, one per line
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() { echo "::error::$*" >&2; exit 1; }

# The eight files a signed release attaches (PLAN-B7 §7, r1.15) and the three updater packages latest.json points at.
INSTALLERS=(Deltabadger_darwin-aarch64.dmg Deltabadger_linux-x86_64.deb)
PACKAGES=(darwin-aarch64:Deltabadger_darwin-aarch64.app.tar.gz
          windows-x86_64:Deltabadger_windows-x86_64-setup.exe
          linux-x86_64-appimage:Deltabadger_linux-x86_64.AppImage)

version() { sed -n 's/^version = "\([^"]*\)"$/\1/p' src-tauri/Cargo.toml | head -1; }

stable() { [[ $1 =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; }

# minisign's files are what Tauri base64-encodes into its pubkey and .sig strings.
verify() { # <file> <sig file> <pubkey>
  local tmp; tmp=$(mktemp -d)
  base64 -d <<< "$3" > "$tmp/key.pub" 2>/dev/null || { rm -rf "$tmp"; return 1; }
  base64 -d < "$2" > "$tmp/file.minisig" 2>/dev/null || { rm -rf "$tmp"; return 1; }
  minisign -V -q -p "$tmp/key.pub" -m "$1" -x "$tmp/file.minisig" > /dev/null 2>&1; local ok=$?
  rm -rf "$tmp"; return $ok
}

highest() { # <series> <tag> <tags...>: the newest stable tag of the series, by version order
  local series=$1 tag=$2; shift 2
  local top; top=$(printf '%s\n' "$tag" "$@" | grep -E "^${series//./\\.}\.[0-9.]+$" | while read -r t; do stable "$t" && echo "$t"; done | sort -V | tail -1 || true)
  if [ "$top" = "$tag" ]; then echo true; else echo false; fi
}

cmd=${1:?usage: validate.sh version|set-version|same-version|feed|check|highest|tags ...}; shift
case $cmd in
  version) version ;;

  set-version)
    v=${1:?set-version <version>}
    [[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || fail "'$v' is not a release version"
    # Only the first `version = ` line ([package]) and the lock entry right after `name = "deltabadger-desktop"`.
    awk -v v="$v" '!done && /^version = "/ { print "version = \"" v "\""; done=1; next } 1' src-tauri/Cargo.toml > src-tauri/Cargo.toml.new
    awk -v v="$v" 'prev == "name = \"deltabadger-desktop\"" && /^version = "/ { $0 = "version = \"" v "\"" } { print; prev = $0 }' \
      src-tauri/Cargo.lock > src-tauri/Cargo.lock.new
    jq --arg v "$v" '.version = $v' src-tauri/tauri.conf.json > src-tauri/tauri.conf.json.new
    mv src-tauri/Cargo.toml.new src-tauri/Cargo.toml
    mv src-tauri/Cargo.lock.new src-tauri/Cargo.lock
    mv src-tauri/tauri.conf.json.new src-tauri/tauri.conf.json
    [ "$(version)" = "$v" ] && grep -A1 '^name = "deltabadger-desktop"$' src-tauri/Cargo.lock | grep -qx "version = \"$v\"" \
      || fail "set-version $v did not take"
    ;;

  same-version)
    tag=${1:?same-version <tag>}
    [ "$tag" = "v$(version)" ] || fail "$tag is not src-tauri/Cargo.toml's version ($(version))"
    [ "$tag" = "v$(jq -r .version src-tauri/tauri.conf.json)" ] || fail "$tag is not tauri.conf.json's version"
    ;;

  feed)
    dir=${1:?} tag=${2:?} critical=${3:?} base=${4:?}
    platforms='{}'
    for entry in "${PACKAGES[@]}"; do
      key=${entry%%:*} file=${entry#*:}
      [ -s "$dir/$file.sig" ] || fail "no signature for $file"
      platforms=$(jq --arg k "$key" --arg url "$base/$file" --rawfile sig "$dir/$file.sig" \
        '.[$k] = {url: $url, signature: ($sig | rtrimstr("\n"))}' <<< "$platforms")
    done
    jq -n --arg version "${tag#v}" --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson platforms "$platforms" \
      --argjson critical "$([ "$critical" = true ] && echo true || echo false)" \
      '{version: $version, pub_date: $date, platforms: $platforms} + (if $critical then {critical: true} else {} end)' \
      > "$dir/latest.json"
    ;;

  check)
    dir=${1:?} tag=${2:?} mode=${3:?} pubkey=${4:-}
    if [ "$mode" != signed ]; then
      stable "$tag" && fail "desktop signing not configured: a stable release needs B13 (updater key and Apple signing)"
      echo "prerelease $tag without desktop assets (signing not configured)"; exit 0
    fi
    [ -n "$pubkey" ] || fail "no updater public key to verify the packages with"
    for file in "${INSTALLERS[@]}"; do [ -s "$dir/$file" ] || fail "missing $file"; done
    for entry in "${PACKAGES[@]}"; do
      file=${entry#*:}
      [ -s "$dir/$file" ] || fail "missing $file"
      [ -s "$dir/$file.sig" ] || fail "missing or empty $file.sig"
      verify "$dir/$file" "$dir/$file.sig" "$pubkey" || fail "$file.sig does not verify against the updater key"
    done
    feed="$dir/latest.json"
    [ -s "$feed" ] || fail "missing latest.json"
    jq -e --arg v "${tag#v}" '
      .version == $v
      and (.platforms | keys == ["darwin-aarch64", "linux-x86_64-appimage", "windows-x86_64"])
      and ([.platforms[] | (.url | length > 0) and (.signature | length > 0)] | all)
      and ((has("critical") | not) or .critical == true)' "$feed" > /dev/null \
      || fail "latest.json is not three platforms of version ${tag#v}: $(cat "$feed")"
    for entry in "${PACKAGES[@]}"; do
      key=${entry%%:*} file=${entry#*:}
      [[ $(jq -r --arg k "$key" '.platforms[$k].url' "$feed") == https://*/releases/download/$tag/$file ]] \
        || fail "latest.json's $key does not point at $file"
      [ "$(jq -r --arg k "$key" '.platforms[$k].signature' "$feed")" = "$(tr -d '\n' < "$dir/$file.sig")" ] \
        || fail "latest.json's $key signature is not $file.sig"
    done
    echo "desktop assets of $tag: complete and signed"
    ;;

  highest) highest "$@" ;;

  tags)
    tag=${1:?} major=${2:-}; shift 2 || true
    stable "$tag" || [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+-[0-9A-Za-z.]+$ ]] || fail "'$tag' is not a release tag"
    latest=$(bash script/rust/latest_tag.sh "$tag" "$major")
    v=${tag#v}
    echo "$v"
    if stable "$tag"; then
      mm=${v%.*} m=${v%%.*}
      if [ "$(highest "v$mm" "$tag" "$@")" = true ]; then echo "$mm"; fi
      if [ "$(highest "v$m" "$tag" "$@")" = true ]; then
        echo "$m"
        if [ "$latest" = true ]; then echo latest; fi
      fi
    fi
    ;;

  *) fail "unknown command $cmd" ;;
esac
