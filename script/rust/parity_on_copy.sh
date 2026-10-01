#!/usr/bin/env bash
# Decision parity on a COPY of a real install (rust milestone plan 2, Task 13). For every bot the Rust engine
# would run: one Rails tick and one Rust tick at that bot's next checkpoint, each on its own copy, with the
# same Kraken prices and a scripted, never-sent AddOrder. Any difference is a failure.
#   script/rust/parity_on_copy.sh <copy_of_storage_dir> <tickers.json>
# Both engines WRITE to the copies they are given: never point this at a live install's directory.
set -euo pipefail
src=$(cd "$1" && pwd); tickers=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
root=$(cd "$(dirname "$0")/../.." && pwd)
if [ -e "$src/.engine.lock" ]; then
  if command -v flock >/dev/null; then
    locked=$(flock -n "$src/.engine.lock" true 2>/dev/null && echo free || echo busy)
  else
    locked=$(python3 -c 'import fcntl,sys; f=open(sys.argv[1]); fcntl.flock(f, fcntl.LOCK_EX|fcntl.LOCK_NB)' "$src/.engine.lock" 2>/dev/null && echo free || echo busy)
  fi
  if [ "$locked" != free ]; then
    echo "refusing: $src is in use by a running engine; copy it first (cp -a)" >&2; exit 2
  fi
fi
rails_root=$(mktemp -d); rust_root=$(mktemp -d); scratch=$(mktemp -d)
trap 'rm -rf "$rails_root" "$rust_root" "$scratch"' EXIT
bin="$root/rust/target/release/deltabadger"
(cd "$root/rust" && cargo build -q --release --bin deltabadger)
"$bin" decide plan "$src" "$tickers" "$rails_root" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cp -a "$rails_root/." "$rust_root/"
(cd "$root" && env -u DATABASE_URL PROXY_KRAKEN=http://127.0.0.1:9 SKIP_TEST_DATABASE=true \
  PRIMARY_DATABASE_URL="sqlite3:$scratch/p.sqlite3" QUEUE_DATABASE_URL="sqlite3:$scratch/q.sqlite3" \
  CACHE_DATABASE_URL="sqlite3:$scratch/c.sqlite3" CABLE_DATABASE_URL="sqlite3:$scratch/w.sqlite3" \
  sh -c "bin/rails db:schema:load && bin/rails runner script/rust/decisions.rb record '$rails_root'")
fail=0
for d in "$rails_root"/bot-*/; do
  name=$(basename "$d")
  "$bin" decide run "$rust_root/$name" > "$scratch/rust.json"
  if ruby -rjson -e 'exit(JSON.parse(File.read(ARGV[0])) == JSON.parse(File.read(ARGV[1])) ? 0 : 1)' "$d/rails.json" "$scratch/rust.json"; then
    echo "PASS $name"
  else
    echo "FAIL $name"; fail=1
    ruby -rjson -e 'a, b = ARGV.map { |f| JSON.parse(File.read(f)) }; puts "  rails: #{a.to_json}\n  rust:  #{b.to_json}"' "$d/rails.json" "$scratch/rust.json"
  fi
done
exit $fail
