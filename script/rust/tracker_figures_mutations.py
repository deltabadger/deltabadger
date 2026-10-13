"""One behavioural mutation per family of the tracker's page figures (walk summary, cache schema 3, Figures result).
Each must turn a test red. Run from a clean export with CARGO_TARGET_DIR set and RAILS_ENV=test (tracker_parity
runs Rails' recorder): python3 script/rust/tracker_figures_mutations.py [name ...]"""
import os
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
walk, figures, cache = (root / 'rust/src/tracker' / f for f in ('walk.rs', 'figures.rs', 'cache.rs'))
parity = ['--test', 'tracker_parity']
unit = ['--lib', '-E', 'test(/tracker::cache/)']
mutations = [
    ('received', walk, 'received: in_kind(r) });', 'received: false });', parity),
    ('realised (return of capital)', walk, '(&gains + &w.engine.roc_at(venue))?', 'gains.clone()', parity),
    ('fees (trade fees)', walk, '        fees = (&fees + &r.fee_fiat_value)?;\n', '', parity),
    ('cash_usd', walk, 'if stable(currency) { amount.clone() } else { ps.convert_fiat(amount, currency, "USD", now)? }', '{ ps.convert_fiat(amount, currency, "USD", now)?; amount.clone() }', parity),
    ('unpriced_proceeds', walk, 'unpriced_proceeds: sum(disposals.iter().map(|d| Ok(d.unpriced_proceeds.clone())))?,', 'unpriced_proceeds: Dec::zero(),', parity),
    ('round trips', walk, 'if d.closes_position { closed.push(open.remove(i)); }', '', parity),
    ('openings', walk, 'for r in rows.iter().filter(|r| r.opening) {', 'for r in rows.iter().filter(|_| false) {', parity),
    ('position dates', walk, 'opened_at: lots.iter().map(|l| l.date).min(),', 'opened_at: lots.iter().map(|l| l.date).max(),', parity),
    ('note floor', figures, '} >= Dec::one();', '} >= Dec::zero();', parity),
    ('at_par note', figures, 'None => { notes.note("at_par", currency, None, None, None, Dec::zero(), true); Dec::one() }', 'None => Dec::one(),', parity),
    ('cash unrealised', figures, 'cash_unrealised = (&cash_unrealised + &(&worth - &basis)?)?;', 'let _ = (&worth, &basis);', parity),
    ('backstop', figures, 'if magnitude(&drift) > tolerance {', 'if magnitude(&drift) > tolerance && false {', parity),
    ('holding percent', figures, '* &Dec::from_i64(100))?))', '* &Dec::from_i64(10))?))', parity),
    ('note venue', figures, 'venue_of(c, user_id, &p.symbol)?.as_deref()', 'None', parity),
    ('cache schema 3 decode', cache, 'received:decimal(&v["received"])?', 'received:decimal(&v["realised"])?', unit),
    ('cache schema 2 read', cache, 'v["schema"].as_u64()!=Some(3)', 'v["schema"].as_u64()<Some(2)', unit),
    ('cache publish bound', cache, '[&s.total_invested,&s.received,&s.realised,&s.fees,&s.cash_usd,&s.unpriced_proceeds]', '[&s.total_invested]', unit),
]
env = dict(os.environ, CARGO_TERM_COLOR='never')
assert env.get('CARGO_TARGET_DIR'), 'CARGO_TARGET_DIR must be set'
survived = []
# Optional arguments name the mutations to run (all by default).
for name, path, old, new, test in [m for m in mutations if not sys.argv[1:] or m[0] in sys.argv[1:]]:
    text = path.read_text()
    assert text.count(old) == 1, f'{name}: anchor not found once in {path.name}'
    path.write_text(text.replace(old, new))
    try:
        code = subprocess.run(['cargo', 'nextest', 'run', '--manifest-path', str(root / 'rust/Cargo.toml'), '-j', '6', '--no-fail-fast', *test],
                              env=env, cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode
    finally:
        path.write_text(text)
    # nextest exits 100 for failed tests; anything else (a build failure among them) is not a kill.
    print(f"{'KILLED ' if code == 100 else f'SURVIVED (exit {code})'} {name}", flush=True)
    if code != 100:
        survived.append(name)
sys.exit(1 if survived else 0)
