"""D5b-2b-1 behavioral guards, with restored source after every trial."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
out = Path(os.environ['D5_COMPLETION_MUTATIONS'])
out.mkdir(parents=True, exist_ok=True)
paths = {name: root / path for name, path in {
    'sync': 'rust/src/sync/jobs.rs', 'ledger': 'rust/src/tracker/jobs.rs',
    'delivery': 'rust/src/jobs/notifications.rs', 'web': 'rust/src/web/mod.rs', 'main': 'rust/src/main.rs',
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
            '--lib', '--test', 'tracker_completion', '--test', 'tracker_first_sync_contract', '--test', 'cable']
    args += ['-E', selection or 'test(completion)']
    with (out / f'{name}.log').open('w') as log:
        code = subprocess.run(args, cwd=root, stdout=log, stderr=subprocess.STDOUT).returncode
    return code, (out / f'{name}.log').read_text()

mutants = [
    ('drop_sync', 'sync', 'cx.db.notifications.sync_done(owner);', '', 'test(completion_sync_success)'),
    ('drop_ledger', 'ledger', 'cx.db.notifications.ledger_done(self.user_id);', '', 'test(completion_ledger_refreshes)'),
    ('sync_success_only', 'sync', 'if outcome != Outcome::NothingNew { cx.db.notifications', 'if outcome == Outcome::Done { cx.db.notifications', 'test(completion_sync_success) | test(completion_early)'),
    ('drop_log', 'delivery', '(self.log)("[tracker] completion broadcast failed");', '', 'test(completion_delivery_failure)'),
    ('wrong_owner', 'delivery', 'format!("user_{owner}:sync")', 'format!("user_{}:sync", owner + 1)', 'test(completion_payloads)'),
    ('refresh_before_commit', 'ledger', 'match ledger_run(&cx.db,', 'cx.db.notifications.ledger_done(self.user_id); match ledger_run(&cx.db,', 'test(completion_ledger_refreshes)'),
    ('drop_cable', 'web', 'app.hub.broadcast(stream, payload); Ok(())', 'let _unused = (&app, stream, payload); Ok(())', 'test(completion_notifications_reach)'),
    ('drop_scheduler_wiring', 'main', 'scheduler.with_notifications(web.job_notifications())', 'scheduler', 'test(completion_scheduler_is_connected)'),
]

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
print(f'{len(results)} completion mutations killed, source restored')
