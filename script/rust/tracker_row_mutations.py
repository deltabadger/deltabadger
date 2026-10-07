"""Behavioral mutations for transaction corrections and the read-only export modal."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
out = Path(os.environ['D5_ROW_MUTATIONS'])
out.mkdir(parents=True, exist_ok=True)
paths = {name: root / path for name, path in {
    'writer': 'rust/src/web/tracker.rs', 'transaction': 'rust/src/web/tracker/transaction.rs',
    'row': 'rust/src/web/tracker/row.rs', 'modal': 'rust/src/web/tracker/modal.rs',
    'classes': 'rust/src/web/tracker/classifications.rs', 'pipeline': 'rust/src/web/mod.rs',
    'scheduler':'rust/src/jobs/mod.rs', 'cast':'rust/src/web/string_column.rs',
}.items()}
original = {name: path.read_text() for name, path in paths.items()}
env = dict(os.environ, CARGO_BUILD_JOBS='6', CARGO_PROFILE_DEV_DEBUG='0',
           CARGO_PROFILE_TEST_DEBUG='0', CARGO_INCREMENTAL='0', CARGO_TERM_COLOR='never')
assert Path(env['CARGO_TARGET_DIR']).resolve().is_relative_to(root.parent.resolve())
results = []


def interrupted(signum, frame):
    raise KeyboardInterrupt(f'interrupted by signal {signum}')


signal.signal(signal.SIGTERM, interrupted)


def run(name, test=None):
    subprocess.run(['df', '-g', '/'], check=True)
    assert shutil.disk_usage('/').free >= 15 * 1024**3, 'STOP: less than 15 GiB free'
    args = ['cargo', 'nextest', 'run', '--manifest-path', str(root / 'rust/Cargo.toml'),
            '--locked', '-j', '5', '--no-fail-fast', '--test', 'tracker_transactions', '--test', 'tracker_write_parity', '--test', 'tracker_pages', '--test', 'tracker_writes']
    if test:
        args += ['-E', f'test(={test})']
    with (out / f'{name}.log').open('w') as log:
        code = subprocess.run(args, env=env, cwd=root, stdout=log, stderr=subprocess.STDOUT).returncode
    return code, (out / f'{name}.log').read_text()


guard = 'eligibility::guard(&tx, &ctx.app.cipher, None)'
checked = 'each_write_enforces_csrf_owner_guard_and_atomic_sql_failure'
price = 'price_invalid_retains_stated_zero_is_a_price_and_blank_clears_only_price'
venue = 'price_refuses_quote_and_opposite_cash_group_but_clear_succeeds'
transfer = 'transfer_window_direction_quantity_ownership_and_ambiguity'
parity = 'price_and_transfer_match_rails_bytes_and_all_primary_rows'
modal = 'export_modal_is_read_only_and_matches_rails_bytes'
wake = 'cancellation_after_the_write_starts_still_commits_and_wakes_the_existing_jobs'
csrf = '!matches!(*request.method(), Method::GET | Method::HEAD) && !context.csrf_verified(request.headers())'
mutants = []
for action in ['price', 'toggle_transfer']:
    mutants.append((f'{action}_guard', 'writer', guard,
                    f'if ctx.params.route_path.ends_with("/{action}") {{ Ok::<(), eligibility::Refusal>(()) }} else {{ {guard} }}', checked))
    mutants.append((f'{action}_csrf', 'pipeline', csrf,
                    f'{csrf} && !context.params.route_path.ends_with("/{action}")', checked))
mutants += [
    ('deferred_report_stream', 'modal', 'if !super::super::header_text(&headers,"accept").is_some_and', 'if true || !super::super::header_text(&headers,"accept").is_some_and', 'tracker_d5a_routes_and_modal_destinations_are_self_contained'),
    ('owner', 'row', 'WHERE t.user_id=?1 AND t.id=?2', 'WHERE (t.user_id=?1 OR t.user_id<>?1) AND t.id=?2', checked),
    ('price_validation', 'transaction', 'let Some(value)=plain(&raw)else{return Ok(prepared(StatusCode::UNPROCESSABLE_ENTITY))};',
     'let value=plain(&raw).unwrap_or(Dec::zero());', price),
    ('clear', 'transaction', 'manual.shift_remove("price");', '', price),
    ('venue_valued', 'transaction', 'if row.counterpart(c)?.is_some()', 'if false && row.counterpart(c)?.is_some()', venue),
    ('group_count', 'row', 'if count==2 {opposite}else{None}', 'if count==200 {opposite}else{None}', venue),
    ('window', 'transaction', 'if withdrawal{14}else{-14}', 'if withdrawal{15}else{-15}', transfer),
    ('quantity', 'transaction', 'base_amount<=?6', 'base_amount>=?6', transfer),
    ('ambiguity', 'transaction', 'if candidates.len()==1', 'if !candidates.is_empty()', transfer),
    ('sticky_unlink', 'transaction', 'was_linked,now,withdrawal,owner', 'false,now,withdrawal,owner', transfer),
    ('ledger_wake', 'transaction', 'crate::tracker::jobs::TRACKER_LEDGER', '"unregistered_ledger"', wake),
    ('backfill_wake', 'transaction', 'crate::tracker::jobs::PORTFOLIO_BACKFILL', '"unregistered_backfill"', wake),
    ('engine_wake', 'writer', 'ctx.app.wake_engine();', 'if !ctx.params.route_path.starts_with("/tracker/transactions/"){ctx.app.wake_engine();}', wake),
    ('unavailable', 'row', 'None=>(None,"none",None)', 'None=>(Some(Dec::zero()),"ours",None)', parity),
    ('modal_writes', 'modal', 'let stored:Option<String>=',
     'c.execute("UPDATE users SET tracker_settings=\'{}\' WHERE id=?1",[owner])?;let stored:Option<String>=', modal),
    ('proposal', 'classes', 'Some("etf")=>(Some(1),Some(4))', 'Some("etf")=>(Some(1),Some(0))', modal),
    ('classification_owner', 'classes', 'WHERE user_id=?1 AND exchange_id=?2 AND', 'WHERE (user_id=?1 OR user_id<>?1) AND exchange_id=?2 AND', modal),
    ('classification_year', 'classes', "transacted_at<'2026-01-01 00:00:00'", "transacted_at<'2027-01-01 00:00:00'", modal),
]
mutants += [
    ('late_consumer_drop', 'scheduler', 'let resolver=self.resolver.as_ref().ok_or("job resolver unavailable")?;', 'continue; #[allow(unreachable_code)] let resolver=self.resolver.as_ref().ok_or("job resolver unavailable")?;', 'round1_historical_user_without_startup_key_runs_real_rebuild'),
    ('sql_precision', 'transaction', 'base_amount<=?6', 'ROUND(base_amount,15)<=?6', 'round1_sql_predicate_preserves_sqlite_precision'),
    ('overflow_id', 'transaction', 'let Some(id)=crate::web::bot::id_from_path(&id) else{return Ok(super::layout::missing())};', 'let id=crate::ruby::to_i(&id);', 'round1_overflow_id_never_selects_maximum_row'),
    ('price_model_validation', 'transaction', 'validate_save(c,&row,row.linked)?;', '', 'round1_model_validation_refuses_blank_currency_on_every_save'),
    ('transfer_model_validation', 'transaction', 'validate_save(c,&saving,if was_linked{None}else{Some(deposit)})?;', 'drop(saving);', 'round1_model_validation_refuses_blank_currency_on_every_save'),
    ('turbo_content_type', 'transaction', 'header::CONTENT_TYPE,turbo::CONTENT_TYPE', 'header::CONTENT_TYPE,"text/html"', 'round1_turbo_media_type_is_independent_of_accept'),
    ('string_boolean_cast', 'cast', 'Value::Bool(true)=>Some("t".into())', 'Value::Bool(true)=>Some("true".into())', 'round1_string_columns_use_active_model_casts'),
]

try:
    assert run('baseline')[0] == 0, 'baseline must pass'
    for name, path, before, after, test in mutants:
        count = original[path].count(before)
        assert count == (3 if name == 'turbo_content_type' else 2 if name == 'ledger_wake' else 1), f'ambiguous/missing mutation: {name}: {count}'
        try:
            if name == 'owner':
                paths['transaction'].write_text(original['transaction'].replace('id=?1 AND user_id=?2', 'id=?1 AND (user_id=?2 OR user_id<>?2)'))
            paths[path].write_text(original[path].replace(before, after))
            code, log = run(name, test)
            killed = code == 100 and 'FAIL' in log and 'test run failed' in log
            results.append(dict(name=name, test=test, exit=code, killed=killed))
            (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
            assert killed, f'{name} survived or failed outside the test'
            print(f'{name}: killed by {test}', flush=True)
        finally:
            paths[path].write_text(original[path])
            paths['transaction'].write_text(original['transaction'])
    assert run('restored')[0] == 0, 'restored production must pass'
finally:
    for name, source in original.items():
        paths[name].write_text(source)
print(f'{len(results)} row/modal mutations killed; production restored')
