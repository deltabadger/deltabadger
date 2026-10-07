"""D5b-2a behavioral guards, with restored source after every trial."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
out = Path(os.environ['D5_INDEX_MUTATIONS'])
out.mkdir(parents=True, exist_ok=True)
paths = {name: root / path for name, path in {
    'fx': 'rust/src/web/tracker/fx.rs', 'totals': 'rust/src/figures/totals.rs',
    'read': 'rust/src/web/tracker/read.rs', 'index': 'rust/src/web/tracker/index.rs',
    'template': 'rust/templates/tracker/early.html',
}.items()}
original = {name: path.read_text() for name, path in paths.items()}
results = []

def interrupted(signum, frame):
    raise KeyboardInterrupt(f'interrupted by signal {signum}')

signal.signal(signal.SIGTERM, interrupted)

def run(name, selection=None):
    subprocess.run(['df', '-g', '/'], check=True)
    assert shutil.disk_usage('/').free >= 15 * 1024**3, 'STOP under 15 GiB'
    args = ['cargo', 'nextest', 'run', '--manifest-path', 'rust/Cargo.toml', '--locked', '-j', '5',
            '--lib', '--test', 'tracker_index', '--test', 'tracker_index_currency', '--test', 'tracker_index_contract', '--test', 'current_fx']
    if selection:
        args += ['-E', selection]
    with (out / f'{name}.log').open('w') as log:
        code = subprocess.run(args, cwd=root, stdout=log, stderr=subprocess.STDOUT).returncode
    return code, (out / f'{name}.log').read_text()

page = 'test(=early_index_pages_match_rails_and_every_get_keeps_all_rows)'
mutants = [
    ('currency_case', 'totals', 'code.to_uppercase()', 'code.to_string()', 'binary(=tracker_index_currency)'),
    ('currency_blank', 'totals', 'code.trim().is_empty()', 'code.is_empty()', 'binary(=tracker_index_currency)'),
    ('currency_prepare', 'fx', 'Ok((figures::totals::normalized_currency(&requested),', 'Ok((requested,', 'binary(=tracker_index_currency)'),
    ('read_guard', 'read', 'c.pragma_update(None,"query_only",true)?;', 'c.pragma_update(None,"query_only",false)?;', 'test(view_boundary)'),
    ('read_restore', 'read', 'c.pragma_update(None,"query_only",was)?;', 'c.pragma_update(None,"query_only",1.max(was))?;', 'test(view_boundary)'),
    ('route_boundary', 'index', 'super::read::only(c,|c|render(c,&ctx,owner))', 'render(c,&ctx,owner)', 'binary(=tracker_index_contract)'),
    ('market_missing', 'index', 'let missing=!crate::web::bots::market_data', 'let missing=false && !crate::web::bots::market_data', page),
    ('market_foreign_rows', 'index', 'FROM account_transactions WHERE user_id=?1 AND', 'FROM account_transactions WHERE (user_id=?1 OR user_id<>?1) AND', page),
    ('market_stock_venue', 'index', "'Exchanges::Alpaca','Exchanges::Ibkr'", "'Exchanges::None','Exchanges::Ibkr'", page),
    ('key_presence', 'index', 'SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1) OR EXISTS', 'SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND 0) OR EXISTS', page),
    ('transaction_presence', 'index', 'EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)"', 'EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1 AND 0)"', page),
    ('pending_report', 'index', 'if settings.get("pending_report")', 'if false && settings.get("pending_report")', page),
    ('scope_bounds', 'index', 'ctx.params.query("exchange_id").filter(|s|!s.trim().is_empty())', 'ctx.params.query("exchange_id").filter(|_|false)', page),
    ('date_deferral', 'index', 'if ["from","to"].iter().any(|key|ctx.params.query(key).is_some_and(|v|!v.trim().is_empty()))', 'if false && ["from","to"].iter().any(|key|ctx.params.query(key).is_some_and(|v|!v.trim().is_empty()))', page),
    ('date_last_value', 'index', '["from","to"].iter().any(|key|ctx.params.query(key).is_some_and(|v|!v.trim().is_empty()))', 'ctx.params.query.iter().any(|(k,v)| (k=="from" || k=="to") && !v.trim().is_empty())', 'test(=repeated_scalar_dates_use_the_last_value)'),
    ('shell_before_frame', 'index', '    if ctx.turbo_frame.is_some() {return layout::frame(&ctx.csrf_token(),&page);}\n    let shell=Shell::load(c,&ctx.app,user)?;', '    let shell=Shell::load(c,&ctx.app,user)?;\n    if ctx.turbo_frame.is_some() {return layout::frame(&ctx.csrf_token(),&page);}', 'test(=turbo_frame_skips_unreadable_navbar_but_full_layout_keeps_rails_error)'),
    ('missing_presentation', 'template', '{% if missing %}', '{% if false %}', page),
]
try:
    assert run('baseline')[0] == 0, 'baseline must pass'
    for name, key, before, after, selection in mutants:
        assert original[key].count(before) == (2 if name == 'market_stock_venue' else 1), name
        try:
            paths[key].write_text(original[key].replace(before, after))
            code, log = run(name, selection)
            killed = code == 100 and 'test run failed' in log and 'FAIL' in log
            results.append(dict(name=name, exit=code, killed=killed))
            (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
            assert killed, f'{name} survived or failed outside an assertion'
            print(name, 'killed', flush=True)
        finally:
            paths[key].write_text(original[key])
    assert run('restored')[0] == 0
finally:
    for key, source in original.items():
        paths[key].write_text(source)
print(f'{len(results)} index mutations killed, source restored')
