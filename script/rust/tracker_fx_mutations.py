"""Ruling 8 regressions must kill behavioral mutations; always restore source."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
out = Path(os.environ['D5_FX_MUTATIONS'])
out.mkdir(parents=True, exist_ok=True)
paths = {name: root / path for name, path in {
    'num': 'rust/src/figures/num.rs', 'totals': 'rust/src/figures/totals.rs', 'fx': 'rust/src/web/tracker/fx.rs',
    'transaction': 'rust/src/web/tracker/transaction.rs', 'row': 'rust/src/web/tracker/row.rs',
    'loading': 'rust/src/web/figure/loading.rs', 'service': 'rust/src/web/figure/service.rs',
}.items()}
original = {name: path.read_text() for name, path in paths.items()}
results = []

def interrupted(signum, frame):
    raise KeyboardInterrupt(f'interrupted by signal {signum}')

signal.signal(signal.SIGTERM, interrupted)

def run(name, test=None):
    subprocess.run(['df', '-g', '/'], check=True)
    assert shutil.disk_usage('/').free >= 15 * 1024**3, 'STOP under 15 GiB'
    args = ['cargo', 'nextest', 'run', '--manifest-path', 'rust/Cargo.toml', '--locked', '-j', '5',
            '--test', 'tracker_fx', '--test', 'current_fx']
    if test:
        args += ['-E', f'test(={test})']
    with (out / f'{name}.log').open('w') as log:
        code = subprocess.run(args, cwd=root, stdout=log, stderr=subprocess.STDOUT).returncode
    return code, (out / f'{name}.log').read_text()

mutants = [
    ('reject_numeric_strings', 'num',
     'serde_json::Value::String(text) => text.trim().parse::<f64>().map_err(|_| NumError::NotANumber)?,',
     'serde_json::Value::String(_) => return Err(NumError::NotANumber),',
     'numeric_string_feed_saves_usd_and_rejects_garbage_without_effects'),
    ('integer_division', 'totals', 'let rate = positive_float(to / from)?;',
     'let rate = positive_float((to as i64 / from as i64) as f64)?;',
     'whole_number_feed_must_not_store_eur_as_usd'),
    ('decimal_division', 'totals', 'let rate = positive_float(to / from)?;',
     'let rate = positive_float(Dec::from_f64(to).map_err(Failure::from)?.div(&Dec::from_f64(from).map_err(Failure::from)?).map_err(Failure::from)?.to_f())?;',
     'integer_and_float_provider_rates_follow_rails_float_then_bigdecimal'),
    ('operand_rounding', 'totals', 'let rate = positive_float(to / from)?;',
     'let rate = positive_float(Dec::from_f64(to).map_err(Failure::from)?.to_f() / Dec::from_f64(from).map_err(Failure::from)?.to_f())?;',
     'integer_and_float_provider_rates_follow_rails_float_then_bigdecimal'),
    ('accept_zero', 'totals', 'if !value.is_finite() || value <= 0.0 { return Err(fx_unavailable()); }',
     'if value == 0.0 { return Ok(1.0); } if !value.is_finite() || value < 0.0 { return Err(fx_unavailable()); }',
     'unusable_rates_refuse_without_rows_or_jobs_and_clearing_still_works'),
    ('accept_negative', 'totals', 'if !value.is_finite() || value <= 0.0 { return Err(fx_unavailable()); }',
     'if !value.is_finite() || value == 0.0 { return Err(fx_unavailable()); }',
     'unusable_rates_are_unavailable_in_shared_denominations_and_totals'),
    ('accept_nonfinite', 'totals', 'if !value.is_finite() || value <= 0.0 { return Err(fx_unavailable()); }',
     'if value <= 0.0 { return Err(fx_unavailable()); }',
     'unusable_rates_are_unavailable_in_shared_denominations_and_totals'),
    ('writer_expiry', 'fx', 'if now>=self.until{return Ok(None);}', 'let _=now;',
     'prepared_rate_expiring_before_the_transaction_refuses_without_writes'),
    ('service_reason', 'service', 'entry.reason=if ready{None}else{reason};', 'entry.reason=None;let _=reason;',
     'shared_figure_presentation_states_the_fx_unavailable_reason'),
    ('missing_rate_fallback', 'fx', 'Err(figures::FiguresError::NotComputed(_)|figures::FiguresError::Raised(_))=>Ok(None)',
     'Err(figures::FiguresError::NotComputed(_)|figures::FiguresError::Raised(_))=>Ok(Some("1.0".into()))',
     'unusable_rates_refuse_without_rows_or_jobs_and_clearing_still_works'),
    ('currency_fence', 'fx', 'current!=self.requested || identity!=self.identity',
     'identity!=self.identity', 'currency_and_provider_changes_during_io_refuse_and_retry_fresh'),
    ('provider_fence', 'fx', 'current!=self.requested || identity!=self.identity',
     'current!=self.requested', 'currency_and_provider_changes_during_io_refuse_and_retry_fresh'),
    ('expiry', 'fx', '.filter(|e|e.until>now)', '', 'warm_rates_expire_and_missing_rates_refuse_then_retry'),
    ('failure_backoff', 'fx', 'pub const FAILURE_TTL:i64=5*60;', 'pub const FAILURE_TTL:i64=0;',
     'warm_rates_expire_and_missing_rates_refuse_then_retry'),
    ('unavailable_row', 'row', 'Currency conversion unavailable', '0.00',
     'unusable_rates_refuse_without_rows_or_jobs_and_clearing_still_works'),
    ('shared_reason', 'loading', 'if fx_unavailable {', 'if false && fx_unavailable {',
     'shared_figure_presentation_states_the_fx_unavailable_reason'),
]
try:
    assert run('baseline')[0] == 0
    for name, path, before, after, test in mutants:
        assert original[path].count(before) == 1, name
        try:
            paths[path].write_text(original[path].replace(before, after))
            code, log = run(name, test)
            killed = code == 100 and 'FAIL' in log and 'test run failed' in log
            results.append(dict(name=name, test=test, exit=code, killed=killed))
            (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
            assert killed, f'{name} survived or failed outside the test'
            print(name, 'killed', flush=True)
        finally:
            paths[path].write_text(original[path])
    assert run('restored')[0] == 0
finally:
    for name, source in original.items():
        paths[name].write_text(source)
print(f'{len(results)} FX mutations killed; restored green')
