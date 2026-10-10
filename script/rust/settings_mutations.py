#!/usr/bin/env python3
"""Independent executable S1 bypasses: require named runtime failures and restore exact bytes."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess

root = Path(__file__).resolve().parents[2]
cases = json.loads((root / 'script/rust/settings_mutations.json').read_text())
parser = argparse.ArgumentParser()
parser.add_argument('--preflight', action='store_true')
parser.add_argument('--labels', help='optional development selection; the plan runs all cases')
args = parser.parse_args()
if args.labels:
    wanted = set(args.labels.split(','))
    cases = [case for case in cases if case['label'] in wanted]
    assert {case['label'] for case in cases} == wanted, 'unknown mutation label'


def apply(source, edit):
    old, new = edit['old'], edit['new']
    assert source.count(old) == edit['count'], (edit['file'], old, source.count(old), edit['count'])
    if edit['which'] is None:
        return source.replace(old, new)
    parts = source.split(old)
    return parts[0] + ''.join((new if index == edit['which'] else old) + part
                             for index, part in enumerate(parts[1:]))


for case in cases:
    for edit in case['edits']:
        apply((root / 'rust/src' / edit['file']).read_text(), edit)
if args.preflight:
    print(f'PASS: {len(cases)} executable mutation anchors')
    raise SystemExit(0)

assert 'CARGO_TARGET_DIR' in os.environ, 'use the plan\'s single target directory'
evidence = Path(os.environ['S1_EVIDENCE']) / 'mutations'
evidence.mkdir(parents=True, exist_ok=True)
for case in cases:
    originals = {root / 'rust/src' / edit['file']: (root / 'rust/src' / edit['file']).read_bytes()
                 for edit in case['edits']}
    try:
        free = os.statvfs('/data')
        assert free.f_bavail * free.f_frsize >= 20 * 1024**3, 'STOP: under 20 GiB free'
        subprocess.run(['df', '-h', '/data'], check=True)
        for edit in case['edits']:
            path = root / 'rust/src' / edit['file']
            path.write_text(apply(path.read_text(), edit))
        command = ['cargo', 'nextest', 'run', '--manifest-path', 'rust/Cargo.toml', '--locked',
                   '-j', '6', '--no-fail-fast']
        command += ['--lib'] if case['lib'] else ['--test', 'settings']
        command += ['-E', case['expression']]
        # Runtime mutations must reach their named assertions; the independent build gate is proved separately.
        # This subprocess-only debug flag is rejected for release and never leaks into restored gates/CI.
        mutation_env=os.environ.copy();mutation_env['S1_R4_MUTATION']='runtime-assertion-proof'
        process = subprocess.Popen(command, cwd=root, env=mutation_env, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True, start_new_session=True)
        try:
            output, _ = process.communicate(timeout=900)
        except subprocess.TimeoutExpired as failure:
            (evidence / (case['label'] + '.timeout-excluded.log')).write_bytes(failure.output or b'')
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            raise AssertionError(f'{case["label"]}: timeout is not mutation proof')
        (evidence / (case['label'] + '.log')).write_text(output)
        assert process.returncode == 100 and 'could not compile' not in output, (case['label'], process.returncode, output[-3000:])
        names = re.findall(r'test\(([^)]+)\)', case['expression'])
        assert any('FAIL [' in line and any(name in line for name in names) for line in output.splitlines()), (case['label'], output[-3000:])
        # A test timeout or a fixture unwrap is not evidence that the contract caught the bypass.
        # Inspect the actual panic location in the test source, including custom assert messages.
        panics = re.findall(r'panicked at ([^:\n]+):(\d+):\d+:', output)
        assert panics and 'Elapsed(())' not in output, (case['label'], output[-3000:])
        assert 'called `Result::unwrap()`' not in output and 'called `Option::unwrap()`' not in output, (case['label'], output[-3000:])
        for filename, line_number in panics:
            source_line = (root / 'rust' / filename).read_text().splitlines()[int(line_number) - 1]
            assert any(anchor in source_line for anchor in
                       ['assert!', 'assert_eq!', 'assert_ne!', 'unwrap_err()', 'expect_err(']), (case['label'], filename, line_number, source_line)
        if case.get('marker'):
            assert case['marker'] in output, (case['label'], output[-3000:])
        if case['label'] == 'q-shared-helper':
            for name in ['q_placement_result', 'q_recovery_result', 'q_poll_result',
                         'a_balance_sync_started', 'old_balance_failure', 'old_ledger_failure',
                         'finishing_a_paused_ledger']:
                assert any('FAIL [' in line and name in line for line in output.splitlines()), name
        print(f'PASS sensitivity {case["label"]}: named runtime assertion failed', flush=True)
    finally:
        for path, original in originals.items():
            path.write_bytes(original)
            assert path.read_bytes() == original, path
print(f'PASS: all {len(cases)} mutations caught; exact source restored')
