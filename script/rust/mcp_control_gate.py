#!/usr/bin/env python3
"""Diff gate for newly introduced external-data panic or discarded-error paths."""
import collections
import json
import pathlib
import re

root = pathlib.Path(__file__).resolve().parents[2]
baseline = json.loads((root / 'script/rust/mcp_control_baseline.json').read_text())
pattern = re.compile(r'\.unwrap\s*\(|\.expect\s*\(|\.unwrap_or_default\s*\(|\blet\s+_\s*=|\.ok\s*\(|\bpanic!')
errors = []
for path, old in baseline.items():
    lines = (root / path).read_text().splitlines()
    found = collections.Counter()
    for index, line in enumerate(lines):
        code = line.strip()
        if code.startswith('//') or not pattern.search(code):
            continue
        if path.endswith('/mcp_input.rs') and code == 'BigDec::parse(&text).ok()' and index and lines[index-1].strip() == '// A strict input refusal is a value, matching BotApi::Number.parse returning nil.':
            continue  # This one documented None is the service's strict-number refusal.
        found[code] += 1
    for line, count in (found - collections.Counter(old)).items():
        errors.append(f'{path}: {count} new unchecked/discarded path(s): {line}')
assert not errors, '\n'.join(errors)
source = (root / 'rust/src/web/mcp/control.rs').read_text()
assert 'write::settings(' in source and 'write::lifecycle(' in source
assert not re.search(r'\b(?:INSERT|UPDATE|DELETE)\s+(?:INTO|bots|transactions)', source), 'MCP adapter contains a second write path'
parser = (root / 'rust/src/web/bot/mcp_input.rs').read_text()
assert not re.search(r'\.trim\s*\(', parser), 'MCP input parsing must use Ruby strip, never .trim()'
finish = source.split('fn finish(', 1)[1].split('pub fn call', 1)[0]
assert not re.search(r'\b_\s*=>', finish), 'Outcome classification must be exhaustive without a wildcard'
print('PASS M4 external-data/error gate and shared-writer boundary')
