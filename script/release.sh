#!/usr/bin/env bash
# Cut a release (PLAN-B8 §1.3, r1–r3). Replaces `rails release:*`; needs bash, git, gh and jq.
#
#   script/release.sh patch|minor [--critical] [--notes FILE]
#   script/release.sh major --notes FILE [--critical]
#   script/release.sh beta [patch|minor|major] [--notes FILE]   from a stable version, name the kind (2.70.21 → beta major
#                                                               → 3.0.0-beta.1); from a beta, the next beta of it
#   script/release.sh promote vX.Y.Z      move :latest and GitHub's "Latest" to a published stable release (release.yml)
#   script/release.sh umbrel X.Y.Z [LINE] open the Umbrel release PR pinning that published image (3.0.1 and later)
#
# A release bumps src-tauri/Cargo.toml, src-tauri/tauri.conf.json and the lock's deltabadger-desktop entry, commits
# them to main, fast-forwards nightly, and pushes a signed annotated tag whose message carries the notes and, with
# --critical, the trailer `Critical: true`. release.yml builds and publishes from the tag. From a beta, the bump that
# reaches its version drops the suffix (3.0.0-beta.4 + major → 3.0.0); any other bump is refused.
set -euo pipefail
die() { echo "release: $*" >&2; exit 1; }
cd "$(git rev-parse --show-toplevel)"
VALIDATE=script/release/validate.sh
FILES=(src-tauri/Cargo.toml src-tauri/tauri.conf.json src-tauri/Cargo.lock)
IMAGE=ghcr.io/deltabadger/deltabadger

# Everything is judged on the remote's refs (r3.1).
on_fresh_main() {
  git fetch -q --force --tags origin main || die "could not fetch origin"
  [ "$(git symbolic-ref -q --short HEAD)" = main ] || die "not on main"
  # Untracked files too (D-RELEASE-UNTRACKED-REFUSED): v2.11.0 shipped without a file left untracked.
  [ -z "$(git status --porcelain --untracked-files=all)" ] || die "the working tree is not clean (untracked files count):
$(git status --short --untracked-files=all)"
  [ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main is not origin/main; pull or push first"
  gh auth status > /dev/null 2>&1 || die "gh is not logged in (gh auth login)"
}

# r1.8: a rust.yml run on this exact commit, started by a push or by hand, succeeded in every job, macOS included.
ci_passed() {
  local sha; sha=$(git rev-parse HEAD)
  local runs; runs=$(gh run list --workflow rust.yml --commit "$sha" --json databaseId,event,conclusion \
    --jq '[.[] | select((.event == "push" or .event == "workflow_dispatch") and .conclusion == "success") | .databaseId] | .[]')
  local id jobs
  for id in $runs; do
    jobs=$(gh run view "$id" --json jobs --jq '[.jobs[] | {name, conclusion}]')
    if jq -e 'length > 0 and all(.conclusion == "success") and any(.name == "macos-arm64")' <<< "$jobs" > /dev/null; then return 0; fi
  done
  die "no rust.yml run on ${sha:0:9} passed every job on every platform (macOS runs only when started by hand).
Run: gh workflow run rust.yml --ref main   then wait for it to pass, and run this again."
}

no_release_running() { # asked per active status, so no number of older runs can hide one
  local status busy
  for status in queued in_progress waiting pending requested; do
    busy=$(gh run list --workflow release.yml --status "$status" --limit 200 --json event \
      --jq '[.[] | select(.event != "pull_request")] | length')
    [ "$busy" = 0 ] || die "a release.yml run is $status; wait for it to finish"
  done
}

next_beta() { # <X.Y.Z>: one past the highest beta of it, on the fetched tags and the current version
  local base=$1 n=0 t
  for t in $(git tag -l "v$base-beta.*") "v$current"; do
    [[ $t =~ ^v${base//./\\.}-beta\.([0-9]+)$ ]] && [ "${BASH_REMATCH[1]}" -gt "$n" ] && n=${BASH_REMATCH[1]}
  done
  echo "$base-beta.$((n + 1))"
}

bump() { # <kind> <X> <Y> <Z>
  case $1 in
    patch) echo "$2.$3.$(($4 + 1))" ;;
    minor) echo "$2.$(($3 + 1)).0" ;;
    major) echo "$(($2 + 1)).0.0" ;;
    *) die "unknown kind '$1' (patch, minor or major)" ;;
  esac
}

cmd=${1:-}; shift || true
case $cmd in
  promote)
    tag=${1:?usage: release.sh promote vX.Y.Z}
    [[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "'$tag' is not a stable release tag"
    gh auth status > /dev/null 2>&1 || die "gh is not logged in (gh auth login)"
    no_release_running
    gh workflow run release.yml --ref main -f mode=promote -f "tag=$tag"
    echo "promote $tag started: gh run list --workflow release.yml --limit 1"
    exit 0 ;;

  umbrel)
    v=${1:?usage: release.sh umbrel X.Y.Z [LINE]}; line=${2:-}
    [[ $v =~ ^([0-9]+)\.[0-9]+\.[0-9]+$ ]] && [ "${BASH_REMATCH[1]}" -ge 3 ] || die "'$v' is not a stable 3.x version"
    [ "$v" != 3.0.0 ] || die "3.0.0's Umbrel release is B5-2's PR"
    on_fresh_main
    read -r draft pre < <(gh release view "v$v" --json isDraft,isPrerelease --jq '"\(.isDraft) \(.isPrerelease)"' 2>/dev/null) \
      || die "v$v has no release"
    [ "$draft" = false ] && [ "$pre" = false ] || die "v$v is not a published stable release"
    digest=$(docker buildx imagetools inspect "$IMAGE:$v" --format '{{json .Manifest}}' | jq -r '.digest // empty') || true
    [[ $digest =~ ^sha256:[0-9a-f]{64}$ ]] || die "$IMAGE:$v does not resolve to an index digest"
    if [ -z "$line" ]; then # the release's prose: its notes up to GitHub's generated list, as one line
      line=$(gh release view "v$v" --json body --jq '.body' | tr -d '\r' | awk '/^## What.s Changed|^\*\*Full Changelog/{exit} {print}' \
        | tr -s ' \n\t' ' ' | sed 's/^ //; s/ $//')
    fi
    [ -n "$line" ] || die "v$v has no notes line; pass one: release.sh umbrel $v \"<line>\""
    git show origin/main:deltabadger/docker-compose.yml | grep -Eq "image: $IMAGE:[^@[:space:]]+@sha256:[0-9a-f]{64}" \
      || die "no pinned image line in deltabadger/docker-compose.yml"
    branch=umbrel/v$v tree=$(mktemp -d)/umbrel
    ! git rev-parse -q --verify "refs/heads/$branch" > /dev/null && [ -z "$(git ls-remote --heads origin "$branch")" ] \
      || die "branch $branch exists already"
    git worktree add -q -b "$branch" "$tree" origin/main
    trap 'git worktree remove --force "$tree" 2>/dev/null || true' EXIT
    sed -E -i.bak "s#(image: $IMAGE:)[^@[:space:]]+@sha256:[0-9a-f]{64}#\1$v@$digest#" "$tree/deltabadger/docker-compose.yml"
    awk -v v="$v" -v line="$line" '
      /^version: / { print "version: \"" v "\""; next }
      /^releaseNotes:/ { print "releaseNotes: >-"; print "  " line; skip = 1; next }
      skip && /^[^ ]/ { skip = 0 }
      !skip { print }' "$tree/deltabadger/umbrel-app.yml" > "$tree/umbrel-app.yml.new"
    mv "$tree/umbrel-app.yml.new" "$tree/deltabadger/umbrel-app.yml"; rm -f "$tree/deltabadger/docker-compose.yml.bak"
    grep -q "$IMAGE:$v@$digest" "$tree/deltabadger/docker-compose.yml" || die "no pinned image line in deltabadger/docker-compose.yml"
    git -C "$tree" commit -q -m "Umbrel release $v" -- deltabadger/docker-compose.yml deltabadger/umbrel-app.yml
    git -C "$tree" push -q -u origin "$branch"
    gh pr create --base main --head "$branch" --title "Umbrel release $v" \
      --body "Pins the Umbrel app to \`$IMAGE:$v@$digest\` (the published v$v index), with \`version: \"$v\"\` and its release notes. Merging it releases $v to Umbrel."
    exit 0 ;;

  patch|minor|major|beta) ;;
  *) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac

kind_arg='' critical=false notes_file=''
while [ $# -gt 0 ]; do
  case $1 in
    --critical) critical=true ;;
    --notes) notes_file=${2:?--notes FILE}; shift ;;
    patch|minor|major) [ "$cmd" = beta ] || die "unexpected '$1'"; kind_arg=$1 ;;
    *) die "unexpected '$1'" ;;
  esac
  shift
done
[ -z "$notes_file" ] || [ -s "$notes_file" ] || die "$notes_file: no such file, or empty"
[ -z "$notes_file" ] || ! grep -q '^Critical:' "$notes_file" || die "$notes_file has a 'Critical:' line; that is the --critical trailer's"
[ "$cmd" != major ] || [ -n "$notes_file" ] || die "a major release is an announcement: write it, then release.sh major --notes FILE"

on_fresh_main
ci_passed
no_release_running

current=$(bash "$VALIDATE" version)
[[ $current =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)(-beta\.([0-9]+))?$ ]] || die "src-tauri/Cargo.toml's version '$current' is not X.Y.Z or X.Y.Z-beta.N"
X=${BASH_REMATCH[1]} Y=${BASH_REMATCH[2]} Z=${BASH_REMATCH[3]} beta=${BASH_REMATCH[5]}
if [ -n "$beta" ]; then
  base=$X.$Y.$Z
  if [ "$Z" != 0 ]; then reaches='patch'; elif [ "$Y" != 0 ]; then reaches='minor'; else reaches='major'; fi
  if [ "$cmd" = beta ]; then
    [ -z "$kind_arg" ] || die "$current is a beta already: 'release.sh beta' makes its next beta"
    new=$(next_beta "$base")
  elif [ "$cmd" = "$reaches" ]; then
    new=$base
  else
    die "$current is a beta: finish it with 'release.sh $reaches', or abandon it first"
  fi
else
  if [ "$cmd" = beta ]; then
    [ -n "$kind_arg" ] || die "from $current, name the version the beta leads to: release.sh beta patch|minor|major"
    new=$(next_beta "$(bump "$kind_arg" "$X" "$Y" "$Z")")
  else
    new=$(bump "$cmd" "$X" "$Y" "$Z")
  fi
fi
tag=v$new
! git rev-parse -q --verify "refs/tags/$tag" > /dev/null || die "$tag exists already"

# Notes: the file given; else for a patch or minor a few plain lines from the local Claude CLI, best effort (the
# release keeps GitHub's generated list alone without them); a beta has none.
notes=
if [ -n "$notes_file" ]; then
  notes=$(cat "$notes_file")
elif [ "$cmd" = patch ] || [ "$cmd" = minor ]; then
  previous=$(git describe --tags --abbrev=0 HEAD 2>/dev/null || true)
  log=$([ -n "$previous" ] && git log "$previous..HEAD" --no-merges --format=%s || true)
  intro='These commits are a release of an open-source trading bot.'
  if [ "$cmd" = patch ]; then
    prompt="$intro Reply with ONE short sentence naming what was fixed, starting with
\"Fixes in \". No bullets, no preamble, no closing remarks."
  else
    prompt="$intro Reply with 2-4 bullets, one line each, covering ONLY what a user can now
see or do that they could not before. Skip bug fixes, refactors and internals
entirely. Write what it does for them, not how it was built. Output the bullets and
nothing else: no preamble, no closing remarks, no notes on what you left out. If
nothing here is user-facing, output nothing at all."
  fi
  if [ -n "$log" ] && command -v claude > /dev/null; then
    echo "  writing release notes..."
    notes=$(claude -p "$prompt
$log" < /dev/null 2>/dev/null || true)
  fi
fi
message=$(mktemp); trap 'rm -f "$message"' EXIT
{ echo "$tag"; [ -z "$notes" ] || printf '\n%s\n' "$notes"; [ "$critical" = false ] || printf '\nCritical: true\n'; } > "$message"

echo "$current → $new$([ "$critical" = true ] && echo ' (critical)')"
echo "--- tag message"; cat "$message"; echo "---"
printf 'Proceed? (y/n) '
read -r answer || true
[[ $answer =~ ^[yY]$ ]] || die "aborted; nothing was changed"

bash "$VALIDATE" set-version "$new"
git add -- "${FILES[@]}"
git commit -q -m "Release $tag" -- "${FILES[@]}"
git push -q origin main
# nightly never sits behind a release (as release.rake did).
git push -q origin main:nightly
git tag -s "$tag" -F "$message"
git push -q origin "$tag"
echo "Pushed $tag; release.yml builds and publishes it: gh run list --workflow release.yml --limit 1"
