"""Executable, restoring mutation gate for the implemented D5 write handlers."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
out = Path(os.environ['D5_MUTATIONS'])
out.mkdir(parents=True, exist_ok=True)
source = root / 'rust/src/web/tracker.rs'
pipeline = root / 'rust/src/web/mod.rs'
selection = root / 'rust/src/sync/mod.rs'
original = {p: p.read_text() for p in (source, pipeline, selection)}
results = []
env = dict(os.environ, CARGO_BUILD_JOBS='4', CARGO_PROFILE_DEV_DEBUG='0',
           CARGO_PROFILE_TEST_DEBUG='0', CARGO_INCREMENTAL='0', CARGO_TERM_COLOR='never')
assert Path(env['CARGO_TARGET_DIR']).resolve().is_relative_to(root.parent.resolve())


def interrupted(signum, frame):
    raise KeyboardInterrupt(f'interrupted by signal {signum}')


signal.signal(signal.SIGTERM, interrupted)


def run(name, selected=None):
    subprocess.run(['df', '-g', '/'], check=True)
    if shutil.disk_usage('/').free < 15 * 1024**3:
        raise RuntimeError('STOP: less than 15 GiB free')
    args = ['cargo', 'nextest', 'run', '--manifest-path', str(root / 'rust/Cargo.toml'),
            '--locked', '-j', '4', '--no-fail-fast']
    if selected:
        args += ['--test', 'tracker_sync' if selected.startswith('sync_') else 'tracker_writes', '-E', f'test(={selected})']
    else:
        args += ['--test', 'tracker_writes', '--test', 'tracker_sync']
    with (out / f'{name}.log').open('w') as log:
        process = subprocess.run(args, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
    return process.returncode, (out / f'{name}.log').read_text()


mutants = []
for path, short in [('/tracker/save_export_settings', 'settings'),
                    ('/tracker/fund_classifications', 'funds')]:
    guard = 'eligibility::guard(&tx, &ctx.app.cipher, None)'
    mutants.append((f'{short}_guard', source, guard,
                    f'if ctx.params.route_path == "{path}" {{ Ok::<(), eligibility::Refusal>(()) }} else {{ {guard} }}',
                    'engine_guard_refuses_and_rolls_back_each_write'))
    check = '!matches!(*request.method(), Method::GET | Method::HEAD) && !context.csrf_verified(request.headers())'
    mutants.append((f'{short}_csrf', pipeline, check,
                    f'context.params.route_path != "{path}" && {check}',
                    'missing_csrf_and_foreign_origin_leave_both_writes_untouched'))
    mutants.append((f'{short}_wake', source, 'ctx.app.wake_engine();',
                    f'if ctx.params.route_path != "{path}" {{ ctx.app.wake_engine(); }}',
                    'successful_writes_wake_after_commit'))
mutants += [
    ('settings_allowlist', source, '["export_type", "country", "year", "report_scope"]',
     '["export_type", "country", "year", "report_scope", "show_cash"]',
     'settings_allowlist_blank_preservation_and_owned_update'),
    ('settings_blank', source, '&& !blank(v)', '',
     'settings_allowlist_blank_preservation_and_owned_update'),
    ('settings_owner', source, 'updated_at=?2 WHERE id=?3', 'updated_at=?2 WHERE (id=?3 OR id<>?3)',
     'settings_allowlist_blank_preservation_and_owned_update'),
    ('funds_category', source, 'if kind == 1 && category.is_none() { invalid = true; continue; }',
     'if false { invalid = true; continue; }',
     'classifications_keep_valid_rows_after_an_invalid_row_and_preserve_spelling'),
    ('funds_continue', source, 'invalid = true; continue;', 'invalid = true; break;',
     'classifications_keep_valid_rows_after_an_invalid_row_and_preserve_spelling'),
    ('funds_owner', source, 'WHERE user_id=?1 AND symbol=?2',
     'WHERE (user_id=?1 OR user_id<>?1) AND symbol=?2',
     'unauthenticated_writes_and_foreign_classification_ids_change_nothing_foreign'),
    ('settings_swallow', source,
     'c.execute("UPDATE users SET tracker_settings=?1, updated_at=?2 WHERE id=?3", (updated.to_string(),now,owner))?;',
     'let _ = c.execute("UPDATE users SET tracker_settings=?1, updated_at=?2 WHERE id=?3", (updated.to_string(),now,owner));',
     'settings_database_failure_is_not_success_and_does_not_wake'),
    ('funds_swallow', source,
     'c.execute("INSERT INTO fund_classifications(user_id,symbol,kind,fund_category,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",\n                (owner,&symbol,kind,category,now))?;',
     'let _ = c.execute("INSERT INTO fund_classifications(user_id,symbol,kind,fund_category,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",\n                (owner,&symbol,kind,category,now));',
     'sql_failure_rolls_back_the_entire_classification_batch'),
]
sync_test = 'sync_selects_owned_reading_keys_and_wakes_both_jobs_only_after_guard_and_csrf'
guard = 'eligibility::guard(&tx, &ctx.app.cipher, None)'
check = '!matches!(*request.method(), Method::GET | Method::HEAD) && !context.csrf_verified(request.headers())'
mutants += [
    ('sync_guard', source, guard,
     f'if ctx.params.route_path == "/tracker/sync" {{ Ok::<(), eligibility::Refusal>(()) }} else {{ {guard} }}', sync_test),
    ('sync_csrf', pipeline, check, f'context.params.route_path != "/tracker/sync" && {check}', sync_test),
    ('sync_owner', source, 'WHERE k.id=?1 AND k.user_id=?2',
     'WHERE k.id=?1 AND (k.user_id=?2 OR k.user_id<>?2)', sync_test),
    ('sync_correct_status', selection, 'WHERE k.status = 1 AND k.key_type != 1 AND e.type = ?1 ORDER BY k.id',
     'WHERE k.key_type != 1 AND e.type = ?1 ORDER BY k.id', 'sync_no_keys_has_no_content_and_never_changes_credentials'),
    ('sync_withdrawal', selection, 'WHERE k.status = 1 AND k.key_type != 1 AND e.type = ?1 ORDER BY k.id',
     'WHERE k.status = 1 AND e.type = ?1 ORDER BY k.id', 'sync_no_keys_has_no_content_and_never_changes_credentials'),
]
for job in ['ledger_sync', 'balance_sync']:
    mutants.append((f'sync_{job}_wake', source, 'ctx.app.wake_job(name,&scope);',
                    f'if name != "{job}" {{ ctx.app.wake_job(name,&scope); }}', sync_test))
try:
    code, log = run('baseline')
    assert code == 0, 'unmodified tests must pass before mutations'
    for name, path, before, after, test in mutants:
        assert path.read_text() == original[path], f'{path} not restored'
        assert original[path].count(before) == 1, f'{name}: ambiguous or missing mutation target'
        try:
            path.write_text(original[path].replace(before, after))
            code, log = run(name, test)
            killed = code == 100 and 'test run failed' in log and 'FAIL' in log
            results.append({'name': name, 'test': test, 'exit': code, 'killed': killed})
            (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
            if not killed:
                raise AssertionError(f'{name}: survived or failed outside the test; inspect its log')
            print(f'{name}: killed by {test}', flush=True)
        finally:
            path.write_text(original[path])
    code, log = run('restored')
    assert code == 0, 'restored production must pass'
finally:
    for path, text in original.items():
        path.write_text(text)
print(f'{len(results)} mutations killed; production restored')
