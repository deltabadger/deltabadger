"""Executable, restoring mutation gate for the import readers (rust/src/tracker/import): each mutant must fail a
recorded vector in rust/tests/import_vectors.rs. Builds through the shared lock (cargo-locked.sh)."""
import json
import os
from pathlib import Path
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
out = Path(os.environ.get('IMPORT_MUTATIONS', '/tmp/import_mutations'))
out.mkdir(parents=True, exist_ok=True)
cargo = os.environ.get('CARGO_LOCKED', str(root.parent / '.rust-work/cargo-locked.sh'))
binance = root / 'rust/src/tracker/import/binance.rs'
csv = root / 'rust/src/tracker/import/csv.rs'
original = {p: p.read_text() for p in (binance, csv)}
results = []


def interrupted(signum, frame):
    raise KeyboardInterrupt(f'interrupted by signal {signum}')


signal.signal(signal.SIGTERM, interrupted)


def run(name):
    args = [cargo, 'nextest', 'run', '--locked', '--no-fail-fast', '--test', 'import_vectors']
    with (out / f'{name}.log').open('w') as log:
        process = subprocess.run(args, cwd=root / 'rust', stdout=log, stderr=subprocess.STDOUT)
    return process.returncode, (out / f'{name}.log').read_text()


split_last = 'parts.push(total - &sum(parts.iter()));'
mutants = [
    ('no_cash_test', binance, 'if CASH_RE.is_match(&spent.coin) {', 'if false {'),
    ('dust_one_key', binance, '&format!("dustcsv_{remark}")', '"dustcsv"'),
    ('convert_window_wider', binance, 'chrono::Duration::seconds(CONVERT_WINDOW)', 'chrono::Duration::seconds(CONVERT_WINDOW + 1)'),
    ('no_remainder_on_last_row', binance, split_last,
     'parts.push({ let share = (total * &weights[weights.len() - 1]).div(&all).unwrap_or_else(BigDec::zero); '
     'if negative(&share) { share.ceil(places) } else { share.floor(places) } });'),
    ('one_sided_booked', binance, 'if incoming.is_empty() || outgoing.is_empty() {', 'if incoming.is_empty() && outgoing.is_empty() {'),
    ('utc_header_ignored', csv, 'if utc { return Some("+00:00".into()); }', ''),
]
try:
    code, log = run('baseline')
    assert code == 0, 'unmodified tests must pass before mutations'
    for name, path, before, after in mutants:
        assert path.read_text() == original[path], f'{path} not restored'
        assert original[path].count(before) == 1, f'{name}: ambiguous or missing mutation target'
        try:
            path.write_text(original[path].replace(before, after))
            code, log = run(name)
            killed = code == 100 and 'FAIL' in log
            results.append({'name': name, 'exit': code, 'killed': killed})
            (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
            if not killed:
                raise AssertionError(f'{name}: survived or failed outside the test; inspect {out / name}.log')
            print(f'{name}: killed', flush=True)
        finally:
            path.write_text(original[path])
    code, log = run('restored')
    assert code == 0, 'restored production must pass'
finally:
    for path, text in original.items():
        path.write_text(text)
print(f'{len(results)} mutations killed; production restored')
