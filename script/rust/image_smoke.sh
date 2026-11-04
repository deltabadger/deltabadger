#!/usr/bin/env bash
# The publish gate for a 3.0 image (PLAN-B3 §6, r1.8): B3-2's quick subset, offline. release.yml runs it on
# each platform before that platform's image is pushed; rust.yml's image job runs it on deltabadger:ci, which proves
# the gate. rust/tests/image.rs stays the full proof; this only stops a broken image from being published.
#   1. an empty named volume, started as root → /health-check 200, /ui-ready 200, docker stop → exit 0;
#   2. a Behind volume (2.70.21's migrate, then its three newest schema_migrations rows deleted) → /health-check 503,
#      /ui-ready 200, the page carries C1, docker stop → exit 0.
# Every container runs on --network none and is asked from inside with the image's curl. Containers and volumes this
# script made are removed on every exit.
# Usage: image_smoke.sh <image>
set -euo pipefail
image=${1:?usage: image_smoke.sh <image>}
rails_2x=ghcr.io/deltabadger/deltabadger:2.70.21
c1='this data is from an older version of Deltabadger'
containers=() volumes=()

cleanup() {
  ((${#containers[@]})) && docker rm -f "${containers[@]}" > /dev/null 2>&1
  ((${#volumes[@]})) && docker volume rm -f "${volumes[@]}" > /dev/null 2>&1
  return 0
}
trap cleanup EXIT

fail() { echo "image_smoke: FAIL: $*" >&2; [ -n "${app:-}" ] && docker logs "$app" 2>&1 | tail -n 40 >&2; exit 1; }
# Sets $vol (not echoed: a $(…) subshell would lose the cleanup entry).
volume() { vol=$(docker volume create); volumes+=("$vol"); }
start() { app=$(docker run -d --network none -e IBKR_CATALOGUE_URL=off -v "$1:/app/storage" "$image"); containers+=("$app"); }
# The status of GET $1 from inside the container; 000 while nothing answers (5 s cap, as Container::offline).
status() { docker exec "$app" curl -s --max-time 5 -o /dev/null -w '%{http_code}' "http://localhost:3000$1" 2> /dev/null || true; }
wait_for() {
  local deadline=$((SECONDS + 120)) got
  while ((SECONDS < deadline)); do
    got=$(status "$1"); [ "$got" = "$2" ] && return 0
    [ "$(docker inspect -f '{{.State.Running}}' "$app")" = true ] || fail "$1: the container exited"
    sleep 1
  done
  fail "$1 never answered $2 (last $got)"
}
expect() { local got; got=$(status "$1"); [ "$got" = "$2" ] || fail "$1 answered $got, not $2"; }
stops_with_0() {
  docker stop "$app" > /dev/null
  local code; code=$(docker inspect -f '{{.State.ExitCode}}' "$app")
  [ "$code" = 0 ] || fail "docker stop exited $code, not 0"
}

echo "image_smoke: $image: an empty volume, started as root"
volume
start "$vol"
wait_for /health-check 200
expect /ui-ready 200
stops_with_0

echo "image_smoke: $image: a Behind 2.x volume"
volume; behind=$vol
docker run --rm --network none -v "$behind:/app/storage" "$rails_2x" migrate > /dev/null
# 2.70.21 ships no sqlite3 binary, so its own Rails deletes the rows (no Ruby on the runner).
left=$(docker run --rm --network none -v "$behind:/app/storage" "$rails_2x" rake runner '
  c = ActiveRecord::Base.connection
  c.execute("DELETE FROM schema_migrations WHERE version IN (SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 3)")
  print c.select_value("SELECT COUNT(*) FROM schema_migrations")' | tail -n 1)
[[ $left =~ ^[0-9]+$ ]] || fail "the Behind fixture: rake runner printed '$left'"
start "$behind"
wait_for /health-check 503
expect /ui-ready 200
expect / 503
page=$(docker exec "$app" curl -s --max-time 5 http://localhost:3000/ || true)
[[ $page == *"$c1"* ]] || fail "the refusal page lacks C1"
stops_with_0

echo "image_smoke: $image: ok"
