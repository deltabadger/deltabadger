#!/usr/bin/env bash
# What desktop.yml's update proofs need on a Windows or Linux leg (PLAN-B7 §9 B7-2), after N was built and smoked:
#   script/rust/desktop_smoke/update_builds.sh <triple> <out dir> <updater key file> [hooks]
# In <out dir>: N's packages; the debug engine (P6's in-tick barrier); N+1 = this tree with its version bumped and one
# proof migration, built in place and then put back (never committed), signed with the job's throwaway key; another
# platform's correctly signed package (r5.1); N+1's signature by another key (P3); versions.json. Windows also checks
# that the test hooks are in the test installer and not in an installer for the released identifier.
set -euo pipefail
cd "$(dirname "$0")/../../.."
triple=${1:?triple} out=${2:?out dir} key=${3:?key file} only=${4:-}
mkdir -p "$out"
b=src-tauri/target/release/bundle
exe=; case "$triple" in *windows*) exe=.exe ;; esac
# Bound to N+1's version, as `tauri build` signs (the app is built with requireSignedVersion).
sign() { TAURI_SIGNING_PRIVATE_KEY=$(cat "$2") TAURI_SIGNING_PRIVATE_KEY_PASSWORD= bunx tauri signer sign --app-version "$3" "$1" > /dev/null; }

# ---- r5.4: the release installer carries no test hook (Windows; its own step, the job's last: `hooks`) ----
# The installer built for the released identifier from this tree, installed silently into a folder of its own with every
# test hook's trigger file present: it must ignore all of them (no wait at the barrier, no 20 s sleep, no early end
# leaving .updating, no test-only record of its Restart Manager answer), and ask for no elevation.
if [ "$only" = hooks ]; then
  [ -n "$exe" ] || exit 0
  echo '{"bundle":{"createUpdaterArtifacts":false}}' > "$RUNNER_TEMP/release-id.conf.json"
  bunx tauri build --bundles nsis --config "$RUNNER_TEMP/release-id.conf.json" -- --locked
  n=$(bash script/release/validate.sh version)
  installer=$(ls $b/nsis/*_"$n"_*-setup.exe)
  hooks="$LOCALAPPDATA/deltabadger-test-hooks"
  rm -rf "$hooks"; mkdir -p "$hooks"; touch "$hooks/hold" "$hooks/sleep" "$hooks/quit"
  dir=$(cygpath -w "$out/release-install")
  rm -rf "$out/release-install"
  t0=$SECONDS
  code=$(powershell -NoProfile -Command "\$p = Start-Process -FilePath '$(cygpath -w "$installer")' -ArgumentList '/S','/D=$dir' -PassThru; if (-not \$p.WaitForExit(120000)) { \$p.Kill(); 'hung' } else { \$p.ExitCode }" | tr -d '\r')
  took=$((SECONDS - t0))
  echo "the release installer: exit $code in $took s; test-hook files: $(ls "$hooks" | tr '\n' ' ')"
  [ "$code" = 0 ] || { echo "::error::the release installer exited $code with the test hooks' files present"; exit 1; }
  [ -f "$out/release-install/Deltabadger.exe" ] || { echo "::error::the release installer installed nothing"; exit 1; }
  [ ! -e "$out/release-install/.updating" ] || { echo "::error::the release installer left .updating (its test Quit ran)"; exit 1; }
  [ ! -e "$hooks/waiting" ] && [ ! -e "$hooks/rm-result" ] || { echo "::error::the release installer ran a test hook"; exit 1; }
  [ "$took" -lt 20 ] || { echo "::error::the release installer took $took s (its test Sleep ran?)"; exit 1; }
  # A per-account install needs no elevation: no UAC prompt (installer.nsi `RequestExecutionLevel user`).
  grep -aq 'level="asInvoker"' "$installer" || { echo "::error::the release installer asks for elevation"; exit 1; }
  grep -aq 'level="asInvoker"' "$out/N-setup.exe" || { echo "::error::the test installer asks for elevation"; exit 1; }
  rm -rf "$hooks"
  echo "the release installer ignored every test hook and asks for no elevation"
  exit 0
fi

# ---- N, as built by the job's build step ----
if [ -n "$exe" ]; then
  cp $b/nsis/*-setup.exe "$out/N-setup.exe"
else
  cp $b/appimage/*.AppImage "$out/N.AppImage"
  cp $b/deb/*.deb "$out/N.deb"
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
  # The reference pair is what N+1's installer carries (the bundler patches the exe it packs: target/'s copy differs).
  rm -rf "$out/next-files"
  7z x -y -o"$out/next-files" "$out/$package" >/dev/null
  for f in Deltabadger.exe deltabadger-engine.exe; do
    found=$(find "$out/next-files" -name "$f" -type f | head -1)
    [ -n "$found" ] || { echo "N+1's installer has no $f" >&2; exit 1; }
    cp "$found" "$out/next-$f"
  done
  rm -rf "$out/next-files"
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
