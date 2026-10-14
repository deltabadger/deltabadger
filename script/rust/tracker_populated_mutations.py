"""One behavioural mutation per branch of the populated tracker index's regions and its held-rate reader; each must turn
a test red. Run from a clean export with CARGO_TARGET_DIR and D5_POPULATED_GRID (a recorded pages_tracker_populated.rb
grid) set: python3 script/rust/tracker_populated_mutations.py [name ...]"""
import os
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
populated = root / 'rust/src/web/tracker/populated.rs'
fx = root / 'rust/src/web/tracker/fx.rs'
loading = root / 'rust/src/web/figure/loading.rs'
cache = root / 'rust/src/sync/cache.rs'
page = root / 'rust/src/tracker/figures/page.rs'
grid = ['--test', 'tracker_populated']
held = ['--test', 'tracker_fx_held']
mutations = [
    ('a page wants the rate it lacks', fx, 'app.figure_service.want_fx("USD",&requested);', '', held),
    ('the queue fills wanted rates', loading, 'let wanted=service.take_fx();', 'let wanted:Vec<(String,String)>=vec![];', held),
    ('a pending key with no balances is not stale', cache, 'if id.is_none() && !c.query_row(', 'if false && !c.query_row(', grid),
    ('withheld balances blank money in', populated, 'tile(invested_label, hint, cell(reason,', 'tile(invested_label, hint, cell(None,', grid),
    ('withheld balances blank the total', populated, "tile(label(\"tracker.tiles.total_pnl\"), Some(total_hint), cell(reason,", "tile(label(\"tracker.tiles.total_pnl\"), Some(total_hint), cell(None,", grid),
    ('a missing rate withholds the ledger figures', populated, '(None, None) => Ok(Some(no_value("Currency conversion unavailable")))', '(None, None) => Ok(None)', grid),
    ('the card states the reason', populated, 'let (Some(den), None) = (den, reason) else {', 'let (Some(den), _) = (den, reason) else {', grid),
    ('show cash restates without the cash', page, 'if show_cash {return Ok(Warm{figures,all_cash:false,received})}', 'return Ok(Warm{figures,all_cash:false,received});', grid),
    ('a broken key keeps its chip', populated, 'let ok = valid.contains(&v.id) || retired(v);', 'let ok = true;', grid),
    ('retired venues are not connectable', populated, "type!='Exchanges::Bitmart' AND ", '', grid),
    ('one venue is not a scope', populated, 'if exchanges.len() > 1 || broken {', 'if exchanges.len() > 0 || broken {', grid),
    ('a refusal beside the report is stated', populated, 'if refusal.exists() {', 'if false {', grid),
    ('a finished report downloads', populated, 'if !path.exists() { return Ok(String::new()); }', 'return Ok(String::new());', grid),
    ('the scoped venue is current', populated, 'active: ok && scope == Some(v.id),', 'active: false,', grid),
    ('notes find their row', populated, 'n.symbol == symbol', 'n.symbol != symbol', grid),
    ('the centre reads the covered holdings', populated, 'if let Some(c) = &h.cost { cost = num(&cost + c)?; covered = num(&covered + &h.value)?; }', 'if let Some(c) = &h.cost { cost = num(&cost + c)?; covered = num(&covered + c)?; }', grid),
    ('a warm ledger states its figures', populated, 'Ok(warm) => Some(warm),', 'Ok(_) => None,', grid),
]
env = dict(os.environ, CARGO_TERM_COLOR='never')
assert env.get('CARGO_TARGET_DIR') and env.get('D5_POPULATED_GRID'), 'CARGO_TARGET_DIR and D5_POPULATED_GRID must be set'
survived = []
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
