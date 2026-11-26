#!/usr/bin/env bash
# script/release.sh's cases (PLAN-B8 §3 B8-3, r1.3, r1.8, r3.1): a temp repository with a bare origin, tags signed with a
# throwaway SSH key, and fake gh, claude and docker on PATH. Needs git ≥ 2.34, jq, ssh-keygen. rust.yml runs it.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
failures=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; failures=$((failures + 1)); }
check() { local name=$1; shift; if "$@" > /dev/null 2>&1; then ok "$name"; else bad "$name"; fi; }

# ---- fakes: gh answers from $STATE files, applying --jq as gh does; everything else it is asked is logged
export STATE=$work/state; mkdir -p "$STATE" "$work/bin"
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "gh $*" >> "$STATE/gh.log"
jq_expr=.; status=; args=("$@")
for i in "${!args[@]}"; do
  [ "${args[$i]}" = --jq ] && jq_expr=${args[$((i + 1))]}
  [ "${args[$i]}" = --status ] && status=${args[$((i + 1))]}
done
answer() { # a list filtered by --status, as gh does
  if [ -n "$status" ]; then jq --arg s "$status" '[.[] | select(.status == $s)]' < "$STATE/$1" | jq -r "$jq_expr"; else jq -r "$jq_expr" < "$STATE/$1"; fi
}
case "$1 $2" in
  "auth status") [ ! -e "$STATE/no_auth" ] ;;
  "run list") if [[ " $* " == *" rust.yml "* ]]; then answer rust_runs.json; else answer release_runs.json; fi ;;
  "run view") if [ -e "$STATE/jobs_$3.json" ]; then answer "jobs_$3.json"; else answer jobs.json; fi ;; # per run when given
  "release view") [ -e "$STATE/release.json" ] && answer release.json || exit 1 ;;
  "workflow run"|"pr create") ;;
  *) echo "fake gh: unexpected $*" >&2; exit 1 ;;
esac
EOF
printf '#!/usr/bin/env bash\necho "Fixes in the bots."\n' > "$work/bin/claude"
printf '#!/usr/bin/env bash\necho "{\\"digest\\":\\"sha256:%s\\"}"\n' "$(printf 'ab%.0s' {1..32})" > "$work/bin/docker"
cat > "$work/bin/curl" <<'EOF'
#!/usr/bin/env bash
# The public seed snapshot: $STATE/seed.gz, written to -o; none there is a failed download.
echo "curl $*" >> "$STATE/curl.log"
out=; args=("$@"); for i in "${!args[@]}"; do [ "${args[$i]}" = -o ] && out=${args[$((i + 1))]}; done
[ -e "$STATE/seed.gz" ] && [ -n "$out" ] && cp "$STATE/seed.gz" "$out"
EOF
chmod +x "$work/bin/"*
export PATH=$work/bin:$PATH
DIGEST=sha256:$(printf 'ab%.0s' {1..32})
reset_state() {
  rm -f "$STATE"/*
  echo '[{"databaseId":7,"event":"workflow_dispatch","conclusion":"success"}]' > "$STATE/rust_runs.json"
  echo '{"jobs":[{"name":"linux-x86_64","conclusion":"success"},{"name":"linux-arm64","conclusion":"success"},{"name":"windows-x86_64 (1/2)","conclusion":"success"},{"name":"windows-x86_64 (2/2)","conclusion":"success"},{"name":"macos-arm64","conclusion":"success"}]}' > "$STATE/jobs.json"
  echo '[]' > "$STATE/release_runs.json"
  cp "$root/rust/seed/seed_data.json.gz" "$STATE/seed.gz" # the served snapshot is the committed one
}

# ---- the repository: the files the command reads, on main at 2.70.21, pushed to a bare origin
export GIT_CONFIG_GLOBAL=$work/gitconfig GIT_CONFIG_NOSYSTEM=1
ssh-keygen -q -t ed25519 -N '' -f "$work/sign" -C release-test
git config --global user.name "Release Test"; git config --global user.email release@example.invalid
git config --global init.defaultBranch main
git config --global gpg.format ssh; git config --global user.signingKey "$work/sign"
echo "release@example.invalid $(cat "$work/sign.pub")" > "$work/allowed"; git config --global gpg.ssh.allowedSignersFile "$work/allowed"
git init -q --bare "$work/origin.git"
seed=$work/seed; mkdir -p "$seed"/{script/release,script/rust,src-tauri,deltabadger,rust/seed}
cp "$root/rust/seed/seed_data.json.gz" "$seed/rust/seed/"
cp "$root/script/release.sh" "$seed/script/"
cp "$root/script/release/validate.sh" "$seed/script/release/"
cp "$root/script/rust/latest_tag.sh" "$seed/script/rust/"
cp "$root/src-tauri/Cargo.toml" "$root/src-tauri/Cargo.lock" "$root/src-tauri/tauri.conf.json" "$seed/src-tauri/"
cp "$root/deltabadger/umbrel-app.yml" "$root/deltabadger/docker-compose.yml" "$seed/deltabadger/"
(cd "$seed" && git init -q && bash script/release/validate.sh set-version 2.70.21 && git add -A && git commit -q -m seed \
  && git tag v2.70.21 && git remote add origin "$work/origin.git" && git push -q origin main v2.70.21)
repo=$work/checkout
git clone -q "$work/origin.git" "$repo"
other=$work/other; git clone -q "$work/origin.git" "$other" # pushes behind the command's back

release() { (cd "$repo" && bash script/release.sh "$@"); }
yes_release() { printf 'y\n' | release "$@"; }
state() { # what a refusal must leave untouched: origin's refs, the checkout's refs, HEAD, files
  { git -C "$work/origin.git" for-each-ref --format='%(refname) %(objectname)'; git -C "$repo" for-each-ref --format='%(refname) %(objectname)' refs/heads refs/tags
    git -C "$repo" rev-parse HEAD; git -C "$repo" status --porcelain --untracked-files=all; git -C "$repo" ls-files -s
    (cd "$repo" && find . -path ./.git -prune -o -type f -print0 | xargs -0 shasum); } | sort
}
refused() { # <name> <wanted words> <args...>
  local name=$1 want=$2; shift 2
  local before after out; before=$(state)
  if out=$(printf '%s\n' "${ANSWER:-y}" | release "$@" 2>&1); then bad "$name: went ahead: $out"; return; fi
  after=$(state)
  if [[ $out != *"$want"* ]]; then bad "$name: said: $out"
  elif [ "$before" != "$after" ]; then bad "$name: changed something: $(diff <(echo "$before") <(echo "$after"))"
  elif grep -q "workflow run\|pr create" "$STATE/gh.log" 2>/dev/null; then bad "$name: started a workflow or a PR"
  else ok "$name"; fi
}
version() { git -C "$repo" show "${1:-HEAD}:src-tauri/Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1; }
message() { git -C "$repo" tag -l --format='%(contents)' "$1" | sed '/^-----BEGIN SSH SIGNATURE/,$d'; }
critical() { git -C "$repo" tag -l --format='%(trailers:key=Critical,valueonly)' "$1" | tr -d '[:space:]'; }

# ---- releases
reset_state
check "release.rake is gone" test ! -e "$root/lib/tasks/release.rake"
refused "a stable version needs the kind of its beta" "release.sh beta patch|minor|major" beta
yes_release beta major > /dev/null
check "2.70.21 + beta major → 3.0.0-beta.1" test "$(version)" = 3.0.0-beta.1
check "pushed to main" test "$(git -C "$work/origin.git" rev-parse main)" = "$(git -C "$repo" rev-parse HEAD)"
check "no nightly branch is pushed" test -z "$(git -C "$work/origin.git" branch --list nightly)"
check "the tag is pushed, annotated, on the release commit" test "$(git -C "$work/origin.git" rev-parse 'v3.0.0-beta.1^{commit}')" = "$(git -C "$repo" rev-parse HEAD)"
check "the tag is signed" git -C "$repo" tag -v v3.0.0-beta.1
check "exactly the three version files change" test "$(git -C "$repo" show --format= --name-only HEAD | sort | xargs)" = "src-tauri/Cargo.lock src-tauri/Cargo.toml src-tauri/tauri.conf.json"
check "the lock's entry moved" test "$(grep -A1 -x 'name = "deltabadger-desktop"' "$repo/src-tauri/Cargo.lock" | tail -1)" = 'version = "3.0.0-beta.1"'
check "tauri.conf.json moved" test "$(jq -r .version "$repo/src-tauri/tauri.conf.json")" = 3.0.0-beta.1
check "a beta has no notes and no trailer" test "$(message v3.0.0-beta.1 | xargs)" = v3.0.0-beta.1

yes_release beta > /dev/null
check "the next beta" test "$(version)" = 3.0.0-beta.2

# ---- the seed snapshot (PLAN-B8 r2.6, PLAN-B11 r1.4): the served one, when it differs, rides in the bump commit
gzip -dc "$root/rust/seed/seed_data.json.gz" \
  | jq -c '.["tickers/kraken.json"].data += [.["tickers/kraken.json"].data[0] | .ticker = "B114TESTEUR"]' | gzip -n > "$STATE/seed.gz"
sha() { { sha256sum "$1" 2>/dev/null || shasum -a 256 "$1"; } | cut -d' ' -f1; }
yes_release beta > /dev/null
check "a changed snapshot: the next beta" test "$(version)" = 3.0.0-beta.3
check "the bump commit holds the version files and the snapshot" test "$(git -C "$repo" show --format= --name-only HEAD | sort | xargs)" = "rust/seed/seed_data.json.gz src-tauri/Cargo.lock src-tauri/Cargo.toml src-tauri/tauri.conf.json"
git -C "$repo" show v3.0.0-beta.3:rust/seed/seed_data.json.gz > "$work/tagged.gz"
check "the tag's snapshot is the served one, byte for byte" test "$(sha "$work/tagged.gz")" = "$(sha "$STATE/seed.gz")"
check "it has the extra ticker" bash -c "gzip -dc '$work/tagged.gz' | jq -e '[.[\"tickers/kraken.json\"].data[].ticker] | index(\"B114TESTEUR\")' > /dev/null"
check "the floors pass on it (validate.sh seed)" bash "$repo/script/release/validate.sh" seed "$work/tagged.gz"
check "release.yml's sha256 check takes it as the attached asset (same-seed)" bash "$repo/script/release/validate.sh" same-seed "$STATE/seed.gz"
check "and refuses the committed one before it" bash -c "! bash '$repo/script/release/validate.sh' same-seed '$root/rust/seed/seed_data.json.gz'"
(cd "$other" && git fetch -q && git tag v3.0.0-beta.5 origin/main && git push -q origin v3.0.0-beta.5)
yes_release beta > /dev/null
check "a beta tag pushed elsewhere is counted (fetched first)" test "$(version)" = 3.0.0-beta.6
refused "from a beta, another bump is refused" "finish it with 'release.sh major'" minor
refused "a beta takes no kind" "makes its next beta" beta minor
refused "a major needs written notes" "release.sh major --notes FILE" major
for t in 'Critical: true' 'critical: true' 'Critical : true'; do
  printf 'Better bots.\n\n%s\n' "$t" > "$work/sneaky.md"; refused "notes ending in '$t'" "use --critical instead" major --notes "$work/sneaky.md"
done
printf 'A new, lighter Deltabadger.\n\n## Upgrade\n\nNothing to do.\n' > "$work/notes.md"
yes_release major --notes "$work/notes.md" > /dev/null
check "beta + major → 3.0.0 (the suffix dropped)" test "$(version)" = 3.0.0
check "the notes are in the tag, headings kept" test "$(message v3.0.0 | sed 1,2d)" = "$(cat "$work/notes.md")"
check "no Critical trailer without --critical" test -z "$(critical v3.0.0)"
(cd "$repo" && echo fix > fix.txt && git add fix.txt && git commit -q -m "Fix the bots" && git push -q origin main)
yes_release patch --critical > /dev/null
check "patch → 3.0.1" test "$(version)" = 3.0.1
check "--critical writes the trailer" test "$(critical v3.0.1)" = true
check "a patch's notes come from claude" bash -c "git -C '$repo' tag -l --format='%(contents:body)' v3.0.1 | grep -qx 'Fixes in the bots.'"
yes_release minor > /dev/null
check "minor → 3.1.0" test "$(version)" = 3.1.0
check "umbrel-app.yml was never touched" test -z "$(git -C "$repo" log --format=%h v2.70.21..HEAD -- deltabadger)"

# ---- refusals: nothing written, nothing pushed, no workflow started
reset_state
ANSWER=n refused "answering n" "aborted" patch
git -C "$repo" checkout -q -b side; refused "off main" "not on main" patch; git -C "$repo" checkout -q main
echo change >> "$repo/src-tauri/tauri.conf.json"; refused "a changed file" "not clean" patch; git -C "$repo" checkout -q -- src-tauri/tauri.conf.json
touch "$repo/new_model.rb"; refused "an untracked file" "untracked files count" patch; rm "$repo/new_model.rb"
(cd "$other" && git pull -q && echo x > later && git add later && git commit -q -m later && git push -q origin main)
refused "origin moved on (fetched first)" "main is not origin/main" patch
git -C "$repo" pull -q
echo '[]' > "$STATE/rust_runs.json"; refused "no CI run on the commit" "gh workflow run rust.yml --ref main" patch
echo '[{"databaseId":7,"event":"schedule","conclusion":"success"}]' > "$STATE/rust_runs.json"; refused "only a scheduled run" "no rust.yml runs" patch
echo '[{"databaseId":7,"event":"push","conclusion":"failure"}]' > "$STATE/rust_runs.json"; refused "a red run" "no rust.yml runs" patch
reset_state
echo '{"jobs":[{"name":"linux-x86_64","conclusion":"success"},{"name":"macos-arm64","conclusion":"skipped"}]}' > "$STATE/jobs.json"; refused "a skipped platform job" "no rust.yml runs" patch
echo '{"jobs":[{"name":"linux-x86_64","conclusion":"success"}]}' > "$STATE/jobs.json"; refused "a push run without macOS" "no rust.yml runs" patch
# A push run without macOS plus a macOS-only run by hand on the same commit: every job passed somewhere.
two_runs() { # <push jobs> <dispatch jobs>
  echo '[{"databaseId":7,"event":"push","conclusion":"success"},{"databaseId":8,"event":"workflow_dispatch","conclusion":"success"}]' > "$STATE/rust_runs.json"
  echo "{\"jobs\":$1}" > "$STATE/jobs_7.json"; echo "{\"jobs\":$2}" > "$STATE/jobs_8.json"
}
two_runs '[{"name":"linux-x86_64","conclusion":"success"},{"name":"image-amd64","conclusion":"success"}]' \
  '[{"name":"macos-arm64","conclusion":"failure"},{"name":"image-amd64","conclusion":"skipped"}]'
refused "a red macOS-only run beside a push run" "no rust.yml runs" patch
two_runs '[{"name":"linux-x86_64","conclusion":"success"}]' \
  '[{"name":"macos-arm64","conclusion":"success"},{"name":"image-amd64","conclusion":"skipped"}]'
refused "a job skipped in every run" "no rust.yml runs" patch
echo '[{"databaseId":8,"event":"workflow_dispatch","conclusion":"success"}]' > "$STATE/rust_runs.json"
echo '{"jobs":[{"name":"macos-arm64","conclusion":"success"}]}' > "$STATE/jobs_8.json"
refused "a macOS-only run without a passed push run" "no rust.yml runs" patch
two_runs '[{"name":"linux-x86_64","conclusion":"success"},{"name":"linux-arm64","conclusion":"success"},{"name":"windows-x86_64 (1/2)","conclusion":"success"},{"name":"windows-x86_64 (2/2)","conclusion":"success"},{"name":"image-amd64","conclusion":"success"}]' \
  '[{"name":"macos-arm64","conclusion":"success"},{"name":"image-amd64","conclusion":"skipped"}]'
ANSWER=n refused "a push run plus a macOS-only run by hand passes the CI check (reaches the question)" "aborted" patch
reset_state
for st in queued in_progress waiting pending requested; do
  echo '[{"status":"completed","event":"push"},{"status":"'$st'","event":"pull_request"},{"status":"'$st'","event":"push"}]' > "$STATE/release_runs.json"
  refused "a release run $st" "a release.yml run is $st" patch
done
echo '[{"status":"in_progress","event":"pull_request"}]' > "$STATE/release_runs.json"
yes_release patch > /dev/null && check "a pull request's dry run doesn't block" test "$(version)" = 3.1.1
reset_state; touch "$STATE/no_auth"; refused "gh not logged in" "gh auth login" patch
reset_state; rm "$STATE/seed.gz"; refused "the seed snapshot can't be downloaded" "could not download" patch
reset_state; gzip -dc "$root/rust/seed/seed_data.json.gz" | jq -c 'del(.["tickers/kraken.json"])' | gzip > "$STATE/seed.gz"
refused "a served snapshot that is not a seed bundle" "not a seed bundle" patch
reset_state
(cd "$other" && git fetch -q && git tag v3.1.2 origin/main && git push -q origin v3.1.2)
git -C "$repo" fetch -q --tags origin # the command fetches it too; that read is not a change
refused "the version's tag exists on origin" "v3.1.2 exists already" patch

# ---- promote and umbrel
reset_state
release promote v3.1.0 > /dev/null
check "promote starts release.yml in promote mode" grep -q "workflow run release.yml --ref main -f mode=promote -f tag=v3.1.0" "$STATE/gh.log"
reset_state; refused "promote refuses a beta" "not a stable release tag" promote v3.2.0-beta.1
refused "umbrel refuses 3.0.0" "B5-2" umbrel 3.0.0
reset_state; refused "umbrel refuses a version with no release" "has no release" umbrel 3.1.0
echo '{"isDraft":true,"isPrerelease":false,"body":"x"}' > "$STATE/release.json"; refused "umbrel refuses a draft" "not a published stable release" umbrel 3.1.0
echo '{"isDraft":false,"isPrerelease":false,"body":"Fixes in the bot'"'"'s \"Pause all\" (C:\\new\\test).\n\n## What'"'"'s Changed\n* a thing"}' > "$STATE/release.json"
release umbrel 3.1.0 > /dev/null
pushed() { git -C "$work/origin.git" show "umbrel/v3.1.0:deltabadger/$1"; }
check "umbrel pins every image to the index digest" bash -c "test \"\$(git -C '$work/origin.git' show umbrel/v3.1.0:deltabadger/docker-compose.yml | grep -c 'image: ghcr.io/deltabadger/deltabadger:3.1.0@$DIGEST')\" = \"\$(grep -c 'image: ghcr.io/deltabadger/deltabadger:' '$repo/deltabadger/docker-compose.yml')\""
check "umbrel sets the version" bash -c "git -C '$work/origin.git' show umbrel/v3.1.0:deltabadger/umbrel-app.yml | grep -qx 'version: \"3.1.0\"'"
check "umbrel sets the notes line" bash -c "git -C '$work/origin.git' show umbrel/v3.1.0:deltabadger/umbrel-app.yml | grep -A1 '^releaseNotes:' | grep -qxF '  Fixes in the bot'\\''s \"Pause all\" (C:\\new\\test).'"
check "umbrel keeps the rest of the app file" bash -c "diff <(git -C '$repo' show HEAD:deltabadger/umbrel-app.yml | grep -v '^version:\|^releaseNotes:\|^  Deltabadger will now') <(git -C '$work/origin.git' show umbrel/v3.1.0:deltabadger/umbrel-app.yml | grep -v '^version:\|^releaseNotes:\|^  Fixes in the bot')"
check "umbrel opens the PR" grep -q "pr create --base main --head umbrel/v3.1.0" "$STATE/gh.log"
check "umbrel leaves the checkout on a clean main" test "$(git -C "$repo" symbolic-ref --short HEAD) $(git -C "$repo" status --porcelain | wc -l | xargs)" = "main 0"

[ "$failures" = 0 ] && echo "all passed" || { echo "$failures failed"; exit 1; }
