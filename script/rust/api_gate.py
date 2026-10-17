#!/usr/bin/env python3
"""The REST API's authorization checks are each held by the page grid: every mutation below must turn it red.

  script/rust/api_gate.py   # from anywhere in the checkout; restores every file it touched, even on failure

Each mutation removes one check and runs the API scenarios that exercise it (rust/tests/pages.rs with PAGES); the run
must fail on that scenario. CARGO_LOCKED names a cargo wrapper to run instead of cargo (a shared build lock).
"""
import os
import pathlib
import subprocess

root = pathlib.Path(__file__).resolve().parents[2]
cargo = os.environ.get('CARGO_LOCKED', 'cargo')
MUTATIONS = [
    ('drop the scope check', 'rust/src/web/bearer.rs',
     '    if !scopes(&token.scopes).contains(&required_scope) { return Ok(Err(Refusal::InsufficientScope)); }\n', '',
     'api_bots_tokens step 3', 'api_bots_tokens'),
    ('drop the grant intersection', 'rust/src/web/rest.rs',
     'enabled.iter().any(|n| n == tool) && granted.iter().any(|n| n == tool)', 'enabled.iter().any(|n| n == tool)',
     'api_bots_not_granted step 0', 'api_bots_not_granted'),
    ('drop the personal-owner check', 'rust/src/web/consent.rs',
     'personal_access_token AND personal_owner_id=?2', 'personal_access_token AND (personal_owner_id=?2 OR 1)',
     'api_bots_tokens step 8', 'api_bots_tokens'),
    # REST reads the token; a token that came from a refresh retires nothing (may_mutate: []).
    ('retire refresh ancestors on a REST call', 'rust/src/web/rest.rs',
     'bearer::resolve(c, authorization, "api"', 'bearer::authenticate(c, authorization, "api"',
     'api_bots_tokens step 11', 'api_bots_tokens'),
    # Rails' params: the path's id wins over the query's and the body's.
    ('let the query id win over the path', 'rust/src/web/rest.rs',
     'out.extend(self.path.iter().map(|(name, value)| (name.to_string(), json!(value))));',
     'for (name, value) in self.path { out.entry(name.to_string()).or_insert(json!(value)); }',
     'api_update_ids step 4', 'api_update_ids'),
    # BotApi::Bots::Start reads the flag before it asks whether the bot runs: a junk flag on a running bot is 422.
    ('check running before the start flag', 'rust/src/web/bot/write.rs',
     'if action == Action::Start && submitted.start_fresh().is_err() {', 'if action == Action::Start && submitted.start_fresh().is_err() && !working {',
     'api_start_flags step 3', 'api_start_flags'),
]


def run(label, marker, pages):
    env = dict(os.environ, PAGES=pages)
    done = subprocess.run([cargo, 'nextest', 'run', '--locked', '--test', 'pages', '-E', 'test(rails_and_rust_serve)'],
                          cwd=root / 'rust', env=env, capture_output=True, text=True)
    output = done.stdout + done.stderr
    assert 'could not compile' not in output, f'{label}: compile failure\n{output}'
    assert done.returncode != 0 and 'FAIL [' in output and marker in output, f'{label}: the grid stayed green\n{output[-4000:]}'
    print(f'PASS {label}: {marker} failed', flush=True)


originals = {}
try:
    for label, relative, needle, replacement, marker, pages in MUTATIONS:
        path = root / relative
        text = originals.setdefault(path, path.read_text())
        assert text.count(needle) == 1, f'{label}: the code to mutate moved ({relative})'
        path.write_text(text.replace(needle, replacement))
        run(label, marker, pages)
        path.write_text(text)
finally:
    for path, text in originals.items():
        path.write_text(text)
print('PASS api gate: every REST authorization check is held by the grid; sources restored')
