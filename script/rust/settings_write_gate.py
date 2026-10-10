#!/usr/bin/env python3
"""Reject new unchecked external-data and discarded write errors in S1's touched paths."""
import collections
import json
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[2]
baseline = json.loads((root / 'script/rust/settings_write_baseline.json').read_text())
pattern = re.compile(baseline['pattern'])


def failures():
    paths = set(baseline['files'])
    paths.update(str(p.relative_to(root)) for p in (root / 'rust/src/web/settings').glob('*.rs'))
    errors = []
    for path in sorted(paths):
        source = (root / path).read_text().split('\n#[cfg(test)]')[0]
        found = collections.Counter(line.strip() for line in source.splitlines()
                                    if not line.strip().startswith('//') and pattern.search(line))
        entry = baseline['files'].get(path, {})
        allowed = collections.Counter(entry.get('upstream', {}))
        allowed.update(entry.get('reviewed', {}))
        errors.extend(f'{path}: {count} new unchecked/discarded path: {line}'
                      for line, count in (found - allowed).items())
    model=(root / 'rust/src/engine/model.rs').read_text().split('\n#[cfg(test)]')[0]
    declaration=model[:model.index('pub struct CredentialVersion')].rsplit('#[derive(',1)[-1]
    equality=re.search(r'impl PartialEq for CredentialVersion\s*\{(.*?)\n\}',model,re.S)
    if 'PartialEq' in declaration or equality is None or 'self.same_credentials(other)' not in equality[1] or 'state.digest.ct_eq(&current.digest)' not in model or re.search(r'digest\s*==',model):
        errors.append('credential digest comparison must be constant time')
    return errors


if '--self-test' in sys.argv:
    path = root / 'rust/src/web/settings/account.rs'
    green = path.read_bytes()
    try:
        path.write_bytes(green + b'\nfn s1_gate_probe() { let _ = c.execute("UPDATE users SET name=?1", []); }\n')
        assert any('s1_gate_probe' in error for error in failures()), 'gate missed discarded SQL Result'
        print('PASS: discarded-write gate probe rejected')
    finally:
        path.write_bytes(green)
    path=root / 'rust/src/engine/model.rs'
    green=path.read_bytes()
    try:
        assert b'self.same_credentials(other)' in green
        path.write_bytes(green.replace(b'self.same_credentials(other)',b'self.id == other.id && self.digest == other.digest',1))
        assert 'credential digest comparison must be constant time' in failures(), 'gate missed ordinary digest equality'
        print('PASS: ordinary secret-comparison probe rejected')
    finally:
        path.write_bytes(green)
errors = failures()
if errors:
    print('\n'.join(errors), file=sys.stderr)
    raise SystemExit(1)
print('PASS: S1 write/error gate')
