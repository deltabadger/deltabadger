#!/usr/bin/env bash
# Whether a release tag moves :latest (PLAN-B3 r1.3): `true` only for a stable tag (no '-') whose major is
# LATEST_MAJOR, the repository variable (2 until the 3.0 release, then 3). script/release/validate.sh's `tags` calls it
# for release.yml, and script/release/validate_test.sh proves it, so the rule that publishes is the rule that is tested.
# Usage: latest_tag.sh <tag> <latest_major>   e.g. latest_tag.sh v2.70.22 2 → true
set -euo pipefail
tag=${1:?usage: latest_tag.sh <tag> <latest_major>}
major=${2:-}
# Unset or empty fails loudly: a silent `false` would stop a 2.x patch from moving :latest.
if [[ ! $major =~ ^[0-9]+$ ]]; then
  echo "::error::LATEST_MAJOR is '${major}', not a major version: set it with gh variable set LATEST_MAJOR --body <N>" >&2
  exit 1
fi
version=${tag#v}
if [[ $version != *-* && ${version%%.*} == "$major" ]]; then echo true; else echo false; fi
