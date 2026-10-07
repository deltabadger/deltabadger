"""D5b-2b-1 behavioral guards, with restored source after every trial."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
out = Path(os.environ['D5_FIRST_MUTATIONS'])
out.mkdir(parents=True, exist_ok=True)
paths = {name: root / path for name, path in {
    'page': 'rust/src/web/tracker/first_sync.rs', 'template': 'rust/templates/tracker/first_sync.html',
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
            '--lib', '--test', 'tracker_first_sync', '--test', 'tracker_first_sync_contract']
    if selection:
        args += ['-E', selection]
    with (out / f'{name}.log').open('w') as log:
        code = subprocess.run(args, cwd=root, stdout=log, stderr=subprocess.STDOUT).returncode
    return code, (out / f'{name}.log').read_text()

query = 'test(first_sync_reads_only)'
page = 'binary(=tracker_first_sync)'
mutants = [
    ('job_state', 'page', 'AND NOT EXISTS(SELECT 1 FROM app_configs job JOIN api_keys synced_key ON job.key IN (?2 || synced_key.id, ?3 || synced_key.id) WHERE synced_key.user_id=?1)', 'AND NOT EXISTS(SELECT 1 FROM app_configs job JOIN api_keys synced_key ON job.key IN (?2 || synced_key.id, ?3 || synced_key.id) WHERE synced_key.user_id=?1 AND 0)', 'test(first_sync_persisted_job_state)'),
    ('job_state_owner', 'page', 'WHERE synced_key.user_id=?1', 'WHERE 1', 'test(first_sync_persisted_job_state)'),
    ('ledger_job_state', 'page', '?2 || synced_key.id', "'unused-ledger'", 'test(first_sync_persisted_job_state)'),
    ('balance_job_state', 'page', '?3 || synced_key.id', "'unused-balance'", 'test(first_sync_persisted_job_state)'),
    ('settings_shape', 'page', 'if !shape.is_null() && !shape.is_object()', 'if false && !shape.is_null() && !shape.is_object()', page),
    ('key_owner', 'page', 'EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1)', 'EXISTS(SELECT 1 FROM api_keys WHERE 1)', query),
    ('foreign_keys', 'page', 'WHERE k.user_id=?1 AND (', 'WHERE (k.user_id=?1 OR k.user_id<>?1) AND (', query),
    ('venue', 'page', "e.type!='Exchanges::Alpaca'", '0', query),
    ('status', 'page', 'COALESCE(k.status,-1)!=1', '0', query),
    ('capability', 'page', 'COALESCE(k.key_type,-1) NOT IN (0,2)', '0', query),
    ('ledger_mark', 'page', 'k.last_synced_at IS NOT NULL', '0', query),
    ('balance_mark', 'page', 'k.balances_synced_at IS NOT NULL', '0', query),
    ('failure', 'page', "COALESCE(k.last_sync_error,'')!=''", '0', query),
    ('unknown_value', 'template', '{{ unavailable|safe }}', '<small>$</small>0.00', page),
    ('hidden', 'template', '{% if !hidden %}', '{% if true %}', page),
    ('show_cash', 'page', 'let show_cash=crate::web::bots::show_cash', 'let show_cash=false && crate::web::bots::show_cash', page),
    ('scope_field', 'template', '{{ scope|safe }}', '', page),
    ('export_filters', 'page', 'let query=crate::web::locale::switch_query(&pairs);', 'let query=crate::web::locale::switch_query(&pairs[..0]);', page),
    ('date_last_value', 'page', 'ctx.params.query(key).unwrap_or("")', 'ctx.params.query.iter().find(|(k,_)|k==key).map(|(_,v)|v.as_str()).unwrap_or("")', page),
    ('date_from', 'template', '{{ from }}', '', page),
    ('date_to', 'template', '{{ to }}', '', page),
    ('calendar_cutoff', 'page', 'if raw < "1583-01-01" {return Ok(None);}', '', page),
    ('invalid_date', 'page', 'chrono::NaiveDate::parse_from_str(raw,"%Y-%m-%d").map_err(|_|super::row::invalid())?;', 'let _date=chrono::NaiveDate::parse_from_str(raw,"%Y-%m-%d").map_err(|_|super::row::invalid());', page),
]
for table in ['account_transactions','account_balances','portfolio_snapshots','portfolio_venue_snapshots']:
    clause = f'SELECT 1 FROM {table} WHERE user_id=?1'
    mutants += [(table, 'page', clause, clause+' AND 0', query),
                (table+'_owner', 'page', clause, f'SELECT 1 FROM {table}', query)]

try:
    assert run('baseline')[0] == 0, 'baseline must pass'
    for name, key, before, after, selection in mutants:
        assert original[key].count(before) == 1, name
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
print(f'{len(results)} first-sync mutations killed, source restored')
