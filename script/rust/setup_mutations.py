"""Mutations of the first-run setup that rust/tests/setup.rs must kill. Run from the repo root, holding the build lock:
  CARGO_TARGET_DIR=... python3 script/rust/setup_mutations.py
Each mutant is applied, the named tests run, and the file is restored whatever happens. Exit 1 if a mutant survives."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
mutants = [
    # (name, file, original, replacement, test filter)
    ('blank_token_matches', 'rust/src/web/setup.rs', '!crate::ruby::blank(expected) && ', '', 'a_blank_setup_token'),
    ('non_constant_compare', 'rust/src/web/setup.rs',
     'bool::from(Sha256::digest(given.as_bytes()).ct_eq(&Sha256::digest(expected.as_bytes())))', 'given == expected', 'constant_time'),
    ('check_outside_transaction', 'rust/src/web/setup.rs', '            if admin_exists(&tx)? {\n                return Ok(Created::AdminExists);\n            }\n', '', 'two_concurrent_creates'),
    ('no_admin_check', 'rust/src/web/setup.rs', '    if app.db(admin_exists).await? {', '    if false && app.db(admin_exists).await? {', 'setup_closes_once_an_admin_exists'),
    ('session_token_kept_after_rotation', 'rust/src/web/setup.rs', 'if !token_matches(&given, expected) {', 'if !token_matches(&given, expected) && param.is_some() {', 'rails_and_rust_answer_the_setup_grid_alike'),
    ('setup_without_csrf', 'rust/src/web/mod.rs', '!matches!(*request.method(), Method::GET | Method::HEAD) && !context.csrf_verified',
     '!matches!(*request.method(), Method::GET | Method::HEAD) && context.params.route_path != "/setup" && !context.csrf_verified', 'admin_creation_requires_the_csrf_token'),
    ('token_logged', 'rust/src/web/locale.rs', '"unlock_token", "token", ', '"unlock_token", ', 'the_setup_token_is_never_echoed'),
    ('no_gate', 'rust/src/web/mod.rs', '    } else if to_setup {\n        setup::to_setup()\n', '', 'the_gate_covers'),
    ('password_line_feed', 'rust/src/web/user.rs', '(index + 1 < lines.len() || line', '(line', 'validation_messages'),
    ('claim_written_outside_transaction', 'rust/src/web/setup.rs', '        tx.commit()?;\n        // Setup::SeedAndSyncJob', '        drop(tx);\n        // Setup::SeedAndSyncJob', 'rails_and_rust_answer_the_setup_grid_alike'),
    ('claim_no_sync_wake', 'rust/src/web/setup.rs', 'jobs.wake(crate::jobs::setup_sync::NAME, None, None);', 'let _ = jobs;', 'rails_and_rust_answer_the_setup_grid_alike'),
    ('claim_while_locked', 'rust/src/web/setup.rs', '    require_setup_token(ctx)\n}', '    let _ = require_setup_token; Ok(None)\n}', 'rails_and_rust_answer_the_setup_grid_alike'),
    ('sync_never_completes', 'rust/src/jobs/setup_sync.rs', 'if failed || in_progress { app_config::set', 'if false { app_config::set', 'the_setup_sync_runs'),
    ('name_not_line_anchored', 'rust/src/web/setup.rs', r'r"(?m)^\p{L}+(\s+\p{L}+)*$"', r'r"\A\p{L}+(\s+\p{L}+)*\z"', 'validation_messages'),
]
survivors = []
for name, file, original, replacement, selection in mutants:
    path = root / file
    source = path.read_text()
    assert source.count(original) == 1, f'{name}: the original text is not in {file} exactly once'
    try:
        path.write_text(source.replace(original, replacement))
        run = subprocess.run(['cargo', 'test', '--locked', '--manifest-path', 'rust/Cargo.toml', '--test', 'setup', selection],
                             cwd=root, capture_output=True, text=True)
    finally:
        path.write_text(source)
    killed = run.returncode != 0 and 'test result: FAILED' in run.stdout
    print(f"{'killed  ' if killed else 'SURVIVED'} {name}", flush=True)
    if not killed:
        survivors.append(name)
sys.exit(1 if survivors else 0)
