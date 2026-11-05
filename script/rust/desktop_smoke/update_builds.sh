#!/usr/bin/env bash
# What desktop.yml's update proofs need on a Windows or Linux leg (PLAN-B7 §9 B7-2), after N was built and smoked:
#   script/rust/desktop_smoke/update_builds.sh <triple> <out dir> <updater key file>
# In <out dir>: N's packages; the debug engine (P6's in-tick barrier); N+1 = this tree with its version bumped and one
# proof migration, built in place and then put back (never committed), signed with the job's throwaway key; another
# platform's correctly signed package (r5.1); N+1's signature by another key (P3); versions.json. Windows also checks
# that the test hooks are in the test installer and not in an installer for the released identifier.
set -euo pipefail
cd "$(dirname "$0")/../../.."
triple=${1:?triple} out=${2:?out dir} key=${3:?key file}
mkdir -p "$out"
b=src-tauri/target/release/bundle
exe=; case "$triple" in *windows*) exe=.exe ;; esac
# Bound to N+1's version, as `tauri build` signs (the app is built with requireSignedVersion).
sign() { TAURI_SIGNING_PRIVATE_KEY=$(cat "$2") TAURI_SIGNING_PRIVATE_KEY_PASSWORD= bunx tauri signer sign --app-version "$3" "$1" > /dev/null; }

# ---- N, as built by the job's build step ----
if [ -n "$exe" ]; then
  cp $b/nsis/*-setup.exe "$out/N-setup.exe"
else
  cp $b/appimage/*.AppImage "$out/N.AppImage"
  cp $b/deb/*.deb "$out/N.deb"
fi

# ---- r5.4: the test hooks are compiled for the test identifier only (Windows) ----
if [ -n "$exe" ]; then
  nsi=$(ls src-tauri/target/release/nsis/x64/installer.nsi)
  makensis=$(ls "$LOCALAPPDATA"/tauri/NSIS/makensis.exe)
  grep -q '^!define BUNDLEID "com.deltabadger.app.test"$' "$nsi" || { echo "::error::$nsi is not the test identifier's"; exit 1; }
  # The plugins the bundler's own run found (makensis checks plugin calls while preprocessing too).
  grep -n '^!addplugindir' "$nsi" || true
  plugins=$(cygpath -w "$(dirname "$(find "$LOCALAPPDATA/tauri" -iname nsis_tauri_utils.dll -path '*x86-unicode*' | head -1)")")
  echo "nsis_tauri_utils in $plugins"
  sed 's/^!define BUNDLEID "com.deltabadger.app.test"$/!define BUNDLEID "com.deltabadger.app"/' "$nsi" > "$(dirname "$nsi")/release-id.nsi"
  # The script an installer is compiled from under each identifier, its macros expanded (makensis /PPO): the test
  # build's is the one this job's installer was made from.
  for which in test release; do
    src=installer.nsi; [ $which = release ] && src=release-id.nsi
    (cd "$(dirname "$nsi")" && "$makensis" -INPUTCHARSET UTF8 "-X!addplugindir /x86-unicode \"$plugins\"" -PPO "$src") > "$out/$which.ppo.nsi"
    hooks=$(grep -c 'deltabadger-test-hooks' "$out/$which.ppo.nsi" || true)
    rm_calls=$(grep -c 'RmGetList' "$out/$which.ppo.nsi" || true)
    echo "$which installer: $hooks test-hook lines, $rm_calls RmGetList calls"
    [ "$rm_calls" -ge 1 ] || { echo "::error::the $which installer has no Restart Manager query"; exit 1; }
    # /PPO may echo the macros' definitions as written: those copies are not compiled. Only the test build expands them.
    eval "${which}_hooks=$hooks"
  done
  # A per-account install needs no elevation: no UAC prompt (installer.nsi `RequestExecutionLevel user`).
  grep -aq 'level="asInvoker"' "$out/N-setup.exe" || { echo "::error::the installer asks for elevation"; exit 1; }
  defined=$(grep -c 'deltabadger-test-hooks' src-tauri/windows/hooks.nsh)
  { [ "$release_hooks" = 0 ] || [ "$release_hooks" = "$defined" ]; } || { echo "::error::the release installer carries test hooks"; exit 1; }
  [ $((test_hooks - release_hooks)) -ge 3 ] || { echo "::error::the test installer lacks its test hooks"; exit 1; }
  echo "test hooks expanded in the test installer only ($test_hooks vs $release_hooks lines, $defined in their definitions)"
fi

# ---- the debug engine (DELTABADGER_TEST_PAUSE works only there) ----
bash script/rust/desktop_engine.sh --debug "$triple"
cp "src-tauri/binaries/deltabadger-engine-$triple$exe" "$out/engine-debug$exe"
if [ -z "$exe" ]; then
  # N with the debug engine: the AppImage's squashfs repacked (P6 on Linux).
  rm -rf "$out/squashfs-root"
  (cd "$out" && ./N.AppImage --appimage-extract > /dev/null)
  cp "$out/engine-debug" "$out/squashfs-root/usr/bin/deltabadger-engine"
  offset=$("$out/N.AppImage" --appimage-offset)
  head -c "$offset" "$out/N.AppImage" > "$out/N-debug.AppImage"
  comp=$(unsquashfs -s -o "$offset" "$out/N.AppImage" | sed -n 's/^Compression //p')
  mksquashfs "$out/squashfs-root" "$out/debug.squashfs" -root-owned -noappend -comp "$comp" -quiet
  cat "$out/debug.squashfs" >> "$out/N-debug.AppImage"
  chmod 755 "$out/N-debug.AppImage"
  rm -rf "$out/squashfs-root" "$out/debug.squashfs"
fi

# ---- N+1 ----
n=$(bash script/release/validate.sh version)
next=$(awk -F. '{ printf "%d.%d.%d", $1, $2, $3 + 1 }' <<< "$n")
migration=rust/migrations/29991231000001_b7_proof.sql
bash script/release/validate.sh set-version "$next"
echo 'CREATE TABLE b7_proof (id INTEGER PRIMARY KEY);' > "$migration"
put_back() { git checkout -- src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json; rm -f "$migration"; }
trap put_back EXIT
jq --argjson c "$(cat "$RUNNER_TEMP/update.conf.json")" '. * $c | .bundle.createUpdaterArtifacts = true' <<< '{}' > "$RUNNER_TEMP/next.conf.json"
TAURI_SIGNING_PRIVATE_KEY=$(cat "$key") TAURI_SIGNING_PRIVATE_KEY_PASSWORD= \
  bunx tauri build --bundles "$([ -n "$exe" ] && echo nsis || echo appimage)" --config "$RUNNER_TEMP/next.conf.json" -- --locked
if [ -n "$exe" ]; then
  package=next-setup.exe
  cp $b/nsis/*_"$next"_*-setup.exe "$out/$package"
  cp $b/nsis/*_"$next"_*-setup.exe.sig "$out/$package.sig"
  cp src-tauri/target/release/Deltabadger.exe "$out/next-Deltabadger.exe"
  cp "src-tauri/binaries/deltabadger-engine-$triple.exe" "$out/next-deltabadger-engine.exe"
else
  package=next.AppImage
  cp $b/appimage/*_"$next"_*.AppImage "$out/$package"
  cp $b/appimage/*_"$next"_*.AppImage.sig "$out/$package.sig"
fi
put_back
trap - EXIT

# ---- r5.1: another platform's package, correctly signed under this platform's key ----
python3 - "$out/foreign.bin" "$([ -n "$exe" ] && echo elf || echo pe)" <<'EOF'
import sys
body = bytearray(1 << 20)
if sys.argv[2] == "elf":  # an x86-64 Linux ELF header
    body[:6] = b"\x7fELF\x02\x01"; body[18:20] = b"\x3e\x00"
else:  # an x86-64 Windows PE header
    body[:2] = b"MZ"; body[0x3c] = 0x80; body[0x80:0x86] = b"PE\0\0\x64\x86"
open(sys.argv[1], "wb").write(body)
EOF
sign "$out/foreign.bin" "$key" "$next"

# ---- P3: N+1 signed by another key ----
bunx tauri signer generate --ci -w "$out/other.key" > /dev/null
cp "$out/$package" "$out/other-signed"
sign "$out/other-signed" "$out/other.key" "$next"
mv "$out/other-signed.sig" "$out/$package.other.sig"
rm -f "$out/other-signed" "$out/other.key" "$out/other.key.pub"

printf '{"n": "%s", "next": "%s", "package": "%s"}\n' "$n" "$next" "$package" > "$out/versions.json"

# ---- r4.8: N+1's feed, ready to serve by hand (python3 -m http.server 8765 --bind 127.0.0.1 -d feed) ----
feed=$out/feed
rm -rf "$feed"; mkdir -p "$feed"
name=$([ -n "$exe" ] && echo Deltabadger_windows-x86_64-setup.exe || echo Deltabadger_linux-x86_64.AppImage)
cp "$out/$package" "$feed/$name"; cp "$out/$package.sig" "$feed/$name.sig"
for other in Deltabadger_windows-x86_64-setup.exe Deltabadger_linux-x86_64.AppImage Deltabadger_darwin-aarch64.app.tar.gz; do
  [ "$other" = "$name" ] || echo "not this platform's" > "$feed/$other.sig"
done
bash script/release/validate.sh feed "$feed" "v$next" false http://127.0.0.1:8765
ls -l "$out"
