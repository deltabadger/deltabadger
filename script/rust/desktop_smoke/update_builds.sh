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
  sevenzip="/c/Program Files/7-Zip/7z.exe"
  grep -q '^!define BUNDLEID "com.deltabadger.app.test"$' "$nsi" || { echo "::error::$nsi is not the test identifier's"; exit 1; }
  sed 's/^!define BUNDLEID "com.deltabadger.app.test"$/!define BUNDLEID "com.deltabadger.app"/; s|^!define OUTFILE ".*"$|!define OUTFILE "release-id-setup.exe"|' "$nsi" > "$(dirname "$nsi")/release-id.nsi"
  (cd "$(dirname "$nsi")" && "$makensis" -V2 -INPUTCHARSET UTF8 -OUTPUTCHARSET UTF8 release-id.nsi)
  for which in test release; do
    installer=$out/N-setup.exe; [ $which = release ] && installer=$(dirname "$nsi")/release-id-setup.exe
    rm -rf "$out/7z-$which"; "$sevenzip" x -y -o"$out/7z-$which" "$installer" '[NSIS].nsi' > /dev/null
    script="$out/7z-$which/[NSIS].nsi"
    [ -s "$script" ] || { echo "::error::7-Zip gave no script for the $which installer"; exit 1; }
    hooks=$(grep -c 'deltabadger-test-hooks' "$script" || true)
    rm_calls=$(grep -c 'RmGetList' "$script" || true)
    echo "$which installer: $hooks test-hook lines, $rm_calls RmGetList calls"
    [ "$rm_calls" -ge 1 ] || { echo "::error::the $which installer has no Restart Manager query"; exit 1; }
    if [ $which = test ]; then [ "$hooks" -ge 3 ] || { echo "::error::the test installer lacks its test hooks"; exit 1; }
    else [ "$hooks" = 0 ] || { echo "::error::the release installer carries test hooks"; exit 1; }; fi
    # A per-account install needs no elevation: no UAC prompt (installer.nsi `RequestExecutionLevel user`).
    grep -aq 'level="asInvoker"' "$installer" || { echo "::error::the $which installer asks for elevation"; exit 1; }
  done
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
ls -l "$out"
