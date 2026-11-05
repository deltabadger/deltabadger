#!/usr/bin/env bash
# script/release/validate.sh's cases (PLAN-B8 r1.6, r1.9, r2.1, r2.5; PLAN-B7 r1.14, r1.15). Needs jq and minisign. rust.yml runs it.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
failures=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; failures=$((failures + 1)); }
passes() { local name=$1; shift; if out=$("$@" 2>&1); then ok "$name"; else bad "$name: $out"; fi; }
refuses() { local name=$1 want=$2; shift 2; if out=$("$@" 2>&1); then bad "$name: passed: $out"; elif [[ $out == *"$want"* ]]; then ok "$name"; else bad "$name: said $out"; fi; }
says() { # the output, and a zero exit
  local name=$1 want=$2; shift 2; local out rc=0; out=$("$@" 2>&1) || rc=$?; out=$(xargs <<< "$out")
  if [ "$rc" = 0 ] && [ "$out" = "$want" ]; then ok "$name"; else bad "$name: '$out' (exit $rc), not '$want'"; fi
}

# A copy of the files the script reads, so set-version writes nothing in the checkout.
tree=$work/tree
mkdir -p "$tree/script/release" "$tree/script/rust" "$tree/src-tauri"
cp "$root/script/release/validate.sh" "$tree/script/release/"
cp "$root/script/rust/latest_tag.sh" "$tree/script/rust/"
cp "$root/src-tauri/Cargo.toml" "$root/src-tauri/Cargo.lock" "$root/src-tauri/tauri.conf.json" "$tree/src-tauri/"
v() { bash "$tree/script/release/validate.sh" "$@"; }

# ---- versions
before=$(v version)
passes "the version is src-tauri/Cargo.toml's" test -n "$before"
passes "set-version to a beta" v set-version 3.0.0-beta.3
says "version reads it back" 3.0.0-beta.3 v version
passes "tauri.conf.json moved" test "$(jq -r .version "$tree/src-tauri/tauri.conf.json")" = 3.0.0-beta.3
passes "the lock's deltabadger-desktop entry moved" grep -A1 -x 'name = "deltabadger-desktop"' "$tree/src-tauri/Cargo.lock" -q
says "only one lock line changed" 1 bash -c "diff '$root/src-tauri/Cargo.lock' '$tree/src-tauri/Cargo.lock' | grep -c '^>'"
says "only one Cargo.toml line changed" 1 bash -c "diff '$root/src-tauri/Cargo.toml' '$tree/src-tauri/Cargo.toml' | grep -c '^>'"
passes "same-version agrees" v same-version v3.0.0-beta.3
refuses "same-version refuses another tag" "is not src-tauri/Cargo.toml's version" v same-version v3.0.0
refuses "set-version refuses a non-version" "is not a release version" v set-version 3.0
jq '.version = "9.9.9"' "$tree/src-tauri/tauri.conf.json" > "$work/c" && cp "$tree/src-tauri/tauri.conf.json" "$work/keep" && mv "$work/c" "$tree/src-tauri/tauri.conf.json"
refuses "same-version refuses a tauri.conf.json that differs" "tauri.conf.json" v same-version v3.0.0-beta.3
mv "$work/keep" "$tree/src-tauri/tauri.conf.json"

# ---- desktop assets
minisign -G -W -p "$work/test.pub" -s "$work/test.key" > /dev/null
minisign -G -W -p "$work/other.pub" -s "$work/other.key" > /dev/null
pubkey=$(base64 < "$work/test.pub" | tr -d '\n')
sign() { minisign -S -s "$2" -m "$1" -x "$work/raw.sig" > /dev/null; base64 < "$work/raw.sig" | tr -d '\n' > "$1.sig"; }
FILES=(Deltabadger_darwin-aarch64.dmg Deltabadger_linux-x86_64.deb Deltabadger_darwin-aarch64.app.tar.gz
       Deltabadger_windows-x86_64-setup.exe Deltabadger_linux-x86_64.AppImage)
TAG=v3.1.0
BASE=https://github.com/deltabadger/deltabadger/releases/download/$TAG
assets() { # a complete signed release in $work/a
  rm -rf "$work/a"; mkdir "$work/a"
  for f in "${FILES[@]}"; do echo "$f bytes" > "$work/a/$f"; done
  for f in "${FILES[@]:2}"; do sign "$work/a/$f" "$work/test.key"; done
  v feed "$work/a" "$TAG" "${1:-false}" "$BASE" > /dev/null
}
check() { v check "$work/a" "$TAG" signed "$pubkey"; }

assets; passes "a complete signed release passes" check
passes "latest.json has the three platforms" jq -e '.platforms | keys == ["darwin-aarch64","linux-x86_64-appimage","windows-x86_64"]' "$work/a/latest.json"
passes "not critical without the trailer" jq -e 'has("critical") | not' "$work/a/latest.json"
assets true; passes "critical with the trailer" jq -e '.critical == true' "$work/a/latest.json"
passes "a critical feed passes" check
for f in "${FILES[@]}"; do assets; rm "$work/a/$f"; refuses "missing $f fails" "missing" check; done
for f in "${FILES[@]:2}"; do assets; : > "$work/a/$f.sig"; refuses "empty $f.sig fails" "$f.sig" check; done
assets; echo tampered >> "$work/a/Deltabadger_linux-x86_64.AppImage"; refuses "a tampered package fails" "does not verify" check
assets; sign "$work/a/Deltabadger_windows-x86_64-setup.exe" "$work/other.key"; refuses "a wrong-key signature fails" "does not verify" check
assets; jq 'del(.platforms["linux-x86_64-appimage"])' "$work/a/latest.json" > "$work/j" && mv "$work/j" "$work/a/latest.json"
refuses "latest.json with two platforms fails" "three platforms" check
assets; jq '.platforms["linux-x86_64"] = .platforms["linux-x86_64-appimage"]' "$work/a/latest.json" > "$work/j" && mv "$work/j" "$work/a/latest.json"
refuses "latest.json with four platforms fails" "three platforms" check
assets; jq '.version = "3.0.9"' "$work/a/latest.json" > "$work/j" && mv "$work/j" "$work/a/latest.json"
refuses "latest.json of another version fails" "three platforms of version 3.1.0" check
assets; jq '.platforms["windows-x86_64"].url |= sub("setup.exe"; "x.exe")' "$work/a/latest.json" > "$work/j" && mv "$work/j" "$work/a/latest.json"
refuses "a url to another file fails" "does not point at" check
assets; rm "$work/a/latest.json"; refuses "no latest.json fails" "missing latest.json" check
refuses "a stable tag without signing fails" "a stable release needs B13" v check "$work/a" v3.1.0 unsigned ""
passes "a prerelease without signing passes, image-only" v check "$work/a" v3.1.0-beta.1 unsigned ""

# ---- image tags (rust.yml's former publish-tags cases, then the never-backward rule)
says "beta, LATEST_MAJOR=2" "3.0.0-beta.1" v tags v3.0.0-beta.1 2 v3.0.0-beta.1
says "beta, LATEST_MAJOR=3" "3.0.0-beta.1" v tags v3.0.0-beta.1 3 v3.0.0-beta.1
says "3.0.0, LATEST_MAJOR=2" "3.0.0 3.0 3" v tags v3.0.0 2 v3.0.0
says "3.0.0, LATEST_MAJOR=3" "3.0.0 3.0 3 latest" v tags v3.0.0 3 v3.0.0
says "2.70.22, LATEST_MAJOR=2" "2.70.22 2.70 2 latest" v tags v2.70.22 2 v2.70.22
says "2.70.22, LATEST_MAJOR=3" "2.70.22 2.70 2" v tags v2.70.22 3 v2.70.22
says "an older patch moves only its own minor" "3.0.5 3.0" v tags v3.0.5 3 v3.0.4 v3.0.5 v3.1.0
says "an older minor's patch behind a newer patch moves nothing shared" "3.0.4" v tags v3.0.4 3 v3.0.4 v3.0.5 v3.1.0
says "betas of a newer version don't hold a stable back" "3.1.0 3.1 3 latest" v tags v3.1.0 3 v3.1.0 v3.2.0-beta.1
says "version order, not text order" "3.10.0 3.10 3 latest" v tags v3.10.0 3 v3.9.0 v3.10.0
says "highest: a newer tag only in the remote list holds an older release back" false v highest v3 v3.0.1 v3.0.1 v3.1.0
refuses "LATEST_MAJOR unset fails" "LATEST_MAJOR" v tags v3.0.0 "" v3.0.0

# ---- the seed bundle (r1.9, r2.5): the committed one passes; each broken copy of it fails
good=$root/rust/seed/seed_data.json.gz
bundle() { gzip -dc "$good" | jq "$1" | gzip > "$work/seed.gz"; v seed "$work/seed.gz"; }
passes "the committed seed bundle passes" v seed "$good"
refuses "a seed bundle missing a ticker file fails" "tickers/kraken.json: no data rows" bundle 'del(.["tickers/kraken.json"])'
refuses "an exchange with no tickers fails" "tickers/hyperliquid.json: no data rows" bundle '.["tickers/hyperliquid.json"].data = []'
refuses "a row that is not an object fails" "indices.json: no data rows" bundle '.["indices.json"].data += [1]'
refuses "a logo_url fails" "logo_url" bundle '.["assets.json"].data[5].logo_url = "/logos/x.png"'
refuses "a ticker's base outside assets.json fails" "is not in assets.json" bundle '.["tickers/mexc.json"].data[0].base_external_id = "gone"'
refuses "a ticker without a quote id fails" "null is not in assets.json" bundle '.["tickers/gemini.json"].data[0].quote_external_id = null'
refuses "an index coin outside assets.json fails" "coin gone is not in assets.json" bundle '.["indices.json"].data[0].top_coins += ["gone"]'
refuses "an index's per-exchange coin outside assets.json fails" "coin gone is not in assets.json" \
  bundle '.["indices.json"].data[0].top_coins_by_exchange["Exchanges::Kraken"] += ["gone"]'
passes "an extra file (Alpaca) is not checked" bundle '.["tickers/alpaca.json"] = {data: []}'
echo nope > "$work/seed.gz"; refuses "not a gzip fails" "not a valid gzip" v seed "$work/seed.gz"
refuses "an exchange whose tickers all lack decimals fails" "tickers/kraken.json: no ticker with base, quote and price decimals" \
  bundle '.["tickers/kraken.json"].data |= map(del(.base_decimals))'
passes "one ticker with decimals is enough" bundle '.["tickers/kraken.json"].data |= (.[1:] |= map(.price_decimals = null))'
for doc in '[]' '' ' ' '{"assets.json": {}} {"assets.json": {}}'; do
  printf '%s' "$doc" | gzip > "$work/seed.gz"; refuses "'$doc' is not one JSON object" "not one JSON object" v seed "$work/seed.gz"
done
gzip -dc "$good" | { cat; echo; gzip -dc "$good"; } | gzip > "$work/seed.gz"
refuses "two good bundles in one file fail" "not one JSON object" v seed "$work/seed.gz"

[ "$failures" = 0 ] && echo "all passed" || { echo "$failures failed"; exit 1; }
