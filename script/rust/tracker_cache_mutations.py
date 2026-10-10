"""D5b-2b-2a: assertion kills only; always restore every changed file."""
from pathlib import Path
import json
import os
import shutil
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
out = Path(os.environ['D5_CACHE_MUTATIONS'])
out.mkdir(parents=True, exist_ok=True)
paths = {k: root / v for k, v in {
    'cache': 'rust/src/tracker/cache.rs', 'jobs': 'rust/src/jobs/mod.rs',
    'producer': 'rust/src/tracker/jobs.rs', 'page': 'rust/src/web/tracker/first_sync.rs',
}.items()}
original = {k: p.read_text() for k, p in paths.items()}
results = []

def interrupted(signum, frame):
    raise KeyboardInterrupt(f'interrupted by signal {signum}')

signal.signal(signal.SIGTERM, interrupted)

def run(name, selection):
    subprocess.run(['df', '-h', '/data'], check=True)
    assert shutil.disk_usage('/data').free >= 20 * 1024**3, 'STOP under 20 GiB'
    args = ['cargo', 'nextest', 'run', '--manifest-path', 'rust/Cargo.toml', '--locked', '-j', '6',
            '--lib', '--test', 'tracker_deadline', '--test', 'tracker_cache_producer', '--test', 'tracker_cache_contract', '-E', selection]
    with (out / f'{name}.log').open('w') as log:
        code = subprocess.run(args, cwd=root, stdout=log, stderr=subprocess.STDOUT).returncode
    return code, (out / f'{name}.log').read_text()

all_tests = 'test(cache_) | test(producer_) | test(deadline_) | test(first_sync_defers)'
mutants = [
    ('deadline_owner_error_skip_settle', 'jobs', 'self.settled().await;', 'if captured.is_none() { return; } self.settled().await;', 'test(deadline_owner_error)'),
    ('deadline_owner_reason', 'jobs', 'log(&format!("[tracker] deadline completion owner unavailable: {reason}"));', 'std::hint::black_box(reason); log("[tracker] deadline completion owner unavailable");', 'test(deadline_owner_lookup_error_logs)'),
    ('computed_strict', 'cache', 'Ok(Dec::parse(text)?)', 'Ok(Dec::strict(text)?)', 'test(cache_real_split)'),
    ('computed_limit', 'cache', 'if text.len()>MAX_COMPUTED_DECIMAL', 'if false && text.len()>MAX_COMPUTED_DECIMAL', 'test(cache_computed_decimal_limit)'),
    ('computed_limit_reason', 'cache', 'crate::engine::log("[tracker] ledger cache cold: computed decimal exceeds 1048576-byte limit");', '', 'test(cache_computed_limit_logs)'),
    ('deadline_drop', 'jobs', 'self.db.deadline_completion(completion_owner, name, scope.as_deref()).await;', '', 'test(deadline_completion)'),
    ('deadline_owner_after_run', 'jobs', 'let owner = match captured { Some(owner) => Some(owner), None => self.completion_owner(name, scope).await };', 'let owner = { std::hint::black_box(captured); self.completion_owner(name, scope).await };', 'test(deadline_completion)'),
    ('deadline_before_database', 'jobs', 'self.settled().await;', '', 'test(deadline_completion)'),
    ('deadline_wrong_owner', 'jobs', 'self.notifications.sync_done(owner);', 'self.notifications.sync_done(owner + 1);', 'test(deadline_completion)'),
    ('history', 'cache', 'v["history"].as_str()!=Some(current.history.as_str())', 'false', 'test(cache_invalidates)'),
    ('prices', 'cache', 'v["prices"].as_i64()!=Some(current.prices)', 'false', 'test(cache_invalidates)'),
    ('expiry', 'cache', 'if now>=expires', 'if false && now>=expires', 'test(cache_invalidates)'),
    ('schema', 'cache', 'v["schema"].as_u64()!=Some(1)', 'false', 'test(cache_failure)'),
    ('owner', 'cache', 'v["owner"].as_i64()!=Some(owner)', 'false', 'test(cache_failure)'),
    ('failed_as_cold', 'cache', 'return Ok(State::Failed(UNAVAILABLE))', 'return Ok(State::Cold)', 'test(cache_failure)'),
    ('invalid_as_zero', 'cache', 'Err(_)=>Ok(State::Cold)', 'Err(_)=>Ok(State::Warm(Box::new(Summary::empty())))', 'test(cache_failure)'),
    ('venue_shape', 'cache', 'if encode(&whole)!=encode(&Summary::empty())', 'if false && encode(&whole)!=encode(&Summary::empty())', 'test(cache_malformed)'),
    ('write_size', 'cache', 'if payload.len()>MAX_PAYLOAD', 'if false && payload.len()>MAX_PAYLOAD', 'test(cache_size)'),
    ('read_size', 'cache', 'if raw.len()>MAX_PAYLOAD', 'if false && raw.len()>MAX_PAYLOAD', 'test(cache_size)'),
    ('publish_version', 'cache', 'if &version(c,owner)? != before', 'if false && &version(c,owner)? != before', 'test(cache_invalidates) | test(producer_retries)'),
    ('producer_passes', 'producer', 'for _ in 0..3 {', 'for _ in 0..1 {', 'test(producer_retries)'),
    ('producer_wake', 'producer', 'cx.wakers.wake(TRACKER_LEDGER,Some(&owner.to_string()),None);', '', 'test(producer_changed_inputs)'),
    ('producer_swallow', 'producer', '}).map_err(message)).await?;', '}).map_err(message)).await.unwrap_or((true,false));', 'test(producer_publishes)'),
    ('owned_cache', 'page', 'AND NOT EXISTS(SELECT 1 FROM app_configs WHERE key=?4)', 'AND (?4 IS NOT NULL OR NOT EXISTS(SELECT 1 FROM app_configs WHERE key=?4))', 'test(first_sync_defers)'),
]
try:
    assert run('baseline', all_tests)[0] == 0
    for name, key, before, after, selection in mutants:
        assert original[key].count(before) == 1, name
        try:
            paths[key].write_text(original[key].replace(before, after))
            code, log = run(name, selection)
            killed = code == 100 and 'test run failed' in log and 'panicked at' in log
            results.append(dict(name=name, exit=code, killed=killed))
            (out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
            assert killed, f'{name} survived or did not fail an assertion'
            print(name, 'killed', flush=True)
        finally:
            paths[key].write_text(original[key])
    assert run('restored', all_tests)[0] == 0
finally:
    for key, text in original.items():
        paths[key].write_text(text)
print(f'{len(results)} cache/deadline mutations killed; restored baseline green')
