#!/usr/bin/env bash
# The 2.x desktop data dir the takeover scenario starts from (PLAN-B6 r3.4(b)): `.secrets` written by the 2.x launcher's
# own `setup_secrets`, then both databases made by the 2.70.21 image's `migrate` with those keys. Nothing of 2.x is kept
# in the tree: the launcher code is read from the v2.70.21 tag (its src-tauri/src/lib.rs, the lines below, verbatim) and
# compiled with the main in this script. Linux only (Docker). Writes <out>/fixture-2x.tar (the dir, modes kept).
#   script/rust/desktop_smoke/fixture_2x.sh <out dir>
set -euo pipefail
cd "$(dirname "$0")/../../.."
out=$1
tag=v2.70.21
image=ghcr.io/deltabadger/deltabadger:2.70.21
here=script/rust/desktop_smoke
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

git rev-parse -q --verify "refs/tags/$tag" > /dev/null || git fetch -q --depth=1 origin "refs/tags/$tag:refs/tags/$tag"
mkdir -p "$work/crate/src"
cp "$here/secrets2x/Cargo.toml" "$here/secrets2x/Cargo.lock" "$work/crate/"
{
  echo "// The 2.x desktop's .secrets code, verbatim from $tag:src-tauri/src/lib.rs (lines 1-4, 24-27, 111-128, 155-315)."
  git show "$tag:src-tauri/src/lib.rs" | sed -n '1,4p;24,27p;111,128p;155,315p'
  cat <<'EOF'
fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: secrets2x <data dir>"));
    fs::create_dir_all(dir.join("db")).unwrap();
    setup_secrets(&dir, &dir.join("db").join("production.sqlite3")).unwrap();
}
EOF
} > "$work/crate/src/main.rs"
# The extract names what it must contain: a moved line range fails here, not as a fixture without keys.
for f in 'fn setup_secrets(' 'fn write_secrets_atomically(' 'fn random_hex(' 'const AR_DERIVATION_SALT'; do
  grep -qF "$f" "$work/crate/src/main.rs" || { echo "::error::$tag's lib.rs no longer has '$f' at the extracted lines"; exit 1; }
done
(cd "$work/crate" && cargo build --locked --release -q)

d=$work/data
"$work/crate/target/release/secrets2x" "$d"
echo "2.x .secrets: $(grep -oE '^[A-Z_]+' "$d/.secrets" | tr '\n' ' ')mode $(stat -c %a "$d/.secrets")"
grep -q '^ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=' "$d/.secrets" || { echo "::error::no encryption pair minted"; exit 1; }

export DOCKER_CONFIG=$work/docker
mkdir -p "$DOCKER_CONFIG"
(
  set -a
  # shellcheck disable=SC1090
  source <(grep -E '^[A-Z_]+=' "$d/.secrets")
  set +a
  docker run --rm --network none \
    -e SECRET_KEY_BASE -e ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY -e ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT \
    -v "$d/db:/app/storage" "$image" migrate
) > "$work/migrate.log" 2>&1 || { cat "$work/migrate.log"; echo "::error::2.70.21 migrate failed"; exit 1; }
# The container writes as its own user; the dir goes back to the runner's.
sudo chown -R "$(id -u):$(id -g)" "$d/db"
# The image's own key file; the desktop keeps its keys in <data>/.secrets.
rm -f "$d/db/.secrets"
[ -s "$d/db/production.sqlite3" ] && [ -s "$d/db/production_queue.sqlite3" ] || { ls -la "$d/db"; echo "::error::databases missing"; exit 1; }
ls -la "$d/db"
mkdir -p "$out"
tar -C "$d" -cf "$out/fixture-2x.tar" .
tar -tvf "$out/fixture-2x.tar"
