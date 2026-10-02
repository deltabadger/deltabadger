#!/usr/bin/env bash
# Decision parity on a COPY of a real install (rust milestone plan 2, Task 13). For every bot the Rust engine
# would run: one Rails tick and one Rust tick at that bot's next checkpoint, each on its own copy, with the
# same Kraken prices and a scripted, never-sent AddOrder. Any difference is a failure.
#   script/rust/parity_on_copy.sh <copy_of_storage_dir> <tickers.json>
#   tickers.json maps each pair to the venue's recorded price body: Kraken "XBTEUR": <Ticker body>; Alpaca "BTC/USD": {"quotes": <latest quotes body>, "trades": <latest trades body>}.
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
# explicit template: macOS mktemp -d ignores $TMPDIR without one
t=${TMPDIR:-/tmp}; rails_root=$(mktemp -d "${t%/}/parity.XXXXXX"); rust_root=$(mktemp -d "${t%/}/parity.XXXXXX"); scratch=$(mktemp -d "${t%/}/parity.XXXXXX")
# Job control gives the backgrounded Rails step its own process group, so cleanup can kill the whole tree (env -> sh ->
# bin/rails -> ruby), not just its first process. The step runs in the background and is waited on because bash defers a
# trap while a foreground command runs: a SIGTERM would otherwise sit unhandled until Rails finished, or orphan it.
set -m
rails_pid=
cleanup() {
  set +e # best effort throughout: a failed kill (the group already exited) must not skip the rm
  [ -n "$rails_pid" ] && kill -- "-$rails_pid" 2>/dev/null
  rm -rf "$rails_root" "$rust_root" "$scratch"
}
trap cleanup EXIT
trap 'exit 143' INT TERM # not `trap cleanup EXIT INT TERM`: that would clean up and then carry on running
bin="$root/rust/target/release/deltabadger"
(cd "$root/rust" && cargo build -q --release --bin deltabadger)
"$bin" decide plan "$src" "$tickers" "$rails_root" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cp -a "$rails_root/." "$rust_root/"
(cd "$root" && env -u DATABASE_URL PROXY_KRAKEN=http://127.0.0.1:9 SKIP_TEST_DATABASE=true \
  PRIMARY_DATABASE_URL="sqlite3:$scratch/p.sqlite3" QUEUE_DATABASE_URL="sqlite3:$scratch/q.sqlite3" \
  CACHE_DATABASE_URL="sqlite3:$scratch/c.sqlite3" CABLE_DATABASE_URL="sqlite3:$scratch/w.sqlite3" \
  sh -c "bin/rails db:schema:load && bin/rails runner script/rust/decisions.rb record '$rails_root'") &
rails_pid=$!
wait "$rails_pid"
rails_pid=
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
