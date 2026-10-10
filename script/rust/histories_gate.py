#!/usr/bin/env python3
"""Executable error and fill-reader boundary for B2a, with adversarial probes."""
import collections
import json
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[2]
pattern = re.compile(r'\.unwrap\s*\(|\.expect\s*\(|\.ok\s*\(|\blet\s+_\s*=|\bpanic!')
baseline = json.loads((root / 'script/rust/histories_baseline.json').read_text())

def check():
    for relative, old in baseline.items():
        code = (root / relative).read_text()
        found = collections.Counter(line.strip() for line in code.splitlines()
                                    if not line.lstrip().startswith('//') and pattern.search(line))
        assert not found - collections.Counter(old), f'new unchecked/discarded data: {relative}'
    for folder in ['rust/src/engine', 'rust/src/figures']:
        for path in (root / folder).glob('*.rs'):
            if path.name == 'fill.rs':
                continue
            code = path.read_text()
            # Match SQL statements across Rust's line continuations, excluding mutation SQL.
            for query in re.findall(r'"((?:[^"\\]|\\.)*)"', code, re.S):
                selects = re.search(r'\bSELECT\b', query, re.I)
                execution = re.search(r'\b(?:amount_exec|quote_amount_exec)\b', query, re.I)
                transaction = re.search(r'\bFROM\s+transactions\b', query, re.I)
                raw = re.search(r'\b(?:price|amount|quote_amount)\b|\bSELECT\s+(?:\w+\.)?\*', query, re.I)
                if selects and (execution or (transaction and raw)):
                    raise AssertionError(f'raw fill SELECT outside normalizer: {path}')
    subprocess.run([sys.executable, str(root / 'script/rust/mcp_reads_gate.py')], check=True)
    print('PASS B2a error and raw-fill gates', flush=True)

if '--sensitivity' in sys.argv:
    path = root / 'rust/src/engine/basket.rs'
    original = path.read_text()
    for probe, marker in [
        ('fn probe() { let _ = std::fs::read("missing"); }', 'new unchecked/discarded'),
        ('fn probe() { let sql = "SELECT amount_exec FROM transactions"; }', 'raw fill SELECT'),
        ('fn probe() { let sql = "select price from transactions"; }', 'raw fill SELECT'),
        ('fn probe() { let sql = "SELECT * FROM transactions"; }', 'raw fill SELECT'),
    ]:
        try:
            path.write_text(original + '\n' + probe + '\n')
            run = subprocess.run([sys.executable, __file__], capture_output=True, text=True)
            assert run.returncode != 0 and marker in run.stderr, run.stderr
        finally:
            path.write_text(original)
    print('PASS B2a gate sensitivity; sources restored', flush=True)
check()
