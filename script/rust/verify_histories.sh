#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
export CARGO_BUILD_JOBS=6 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
unset DATABASE_URL PRIMARY_DATABASE_URL QUEUE_DATABASE_URL CACHE_DATABASE_URL CABLE_DATABASE_URL RAILS_ENV
export APP_ROOT_URL=http://localhost:3000 SKIP_TEST_DATABASE=true
check_disk() {
  df -g /
  [ "$(df -g / | awk 'NR==2 {print $4}')" -ge 15 ] || exit 75
}
cd "$root"
check_disk
python3 script/rust/histories_gate.py --sensitivity
python3 script/rust/histories_accounting_gate.py --sensitivity
python3 script/rust/histories_timestamp_gate.py --sensitivity
check_disk
python3 script/rust/histories_full_suite.py
check_disk
python3 script/rust/histories_mutations.py > "$EVIDENCE/06-mutations.log" 2>&1
check_disk
(cd rust && cargo nextest run -j 5 --lib --test histories --test histories_r1 --test histories_sources --test amount --test eligibility) > "$EVIDENCE/06-restored.log" 2>&1
check_disk
python3 script/rust/histories_ruby_free.py
check_disk
(cd rust && cargo clippy --all-targets --locked -- -D warnings)
check_disk
(cd rust && cargo build --release --locked)
check_disk
RAILS_ENV=test bin/rails db:schema:load
check_disk
PARALLEL_WORKERS=5 bin/rails test
