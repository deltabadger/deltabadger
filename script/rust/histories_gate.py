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
    # Whole-tree boundary. Only the shared normalizer owns stored fill SELECTs.
    # Venue parsers produce fill columns from responses; they get no SQL exemption.
    allow = {'rust/src/figures/fill.rs': 'normalization and explicitly typed display/polling snapshots'}
    for path in (root / 'rust/src').rglob('*.rs'):
        if path.relative_to(root).as_posix() in allow:
            continue
        code = path.read_text()
        for query in re.findall(r'"((?:[^"\\]|\\.)*)"', code, re.S):
            query = re.sub(r'[\[\]`]', '', query)
            selects = re.search(r'\bSELECT\b', query, re.I)
            execution = re.search(r'\b(?:amount_exec|quote_amount_exec)\b', query, re.I)
            transaction = re.findall(r'\bSELECT\b((?:(?!\bSELECT\b).)*?)\b(?:FROM|JOIN)\s+transactions\b', query, re.I | re.S)
            raw = re.search(r'\b(?:price|amount|quote_amount)\b|\bSELECT\s+(?:\w+\.)?\*', query, re.I)
            fragmented = raw and re.search(r'\{(?:scope|table|from|naming)\}', query)
            reads_fill = any(re.search(r'\b(?:price|amount|quote_amount)\b|(?:^|,)\s*(?:\w+\.)?\*', fields, re.I) for fields in transaction)
            if selects and (execution or reads_fill or fragmented):
                raise AssertionError(f'raw fill SELECT outside normalizer: {path}')
    subprocess.run([sys.executable, str(root / 'script/rust/histories_accounting_gate.py')], check=True)
    subprocess.run([sys.executable, str(root / 'script/rust/index_histories_gate.py')], check=True)
    fill = (root / 'rust/src/figures/fill.rs').read_text()
    assert not re.search(r'f64|Num::Float|ruby_sum',fill), 'Float arithmetic in fill normalizer'
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
    for relative in ['rust/src/engine/amount.rs','rust/src/figures/fill.rs']:
        path = root / relative
        original = path.read_text()
        try:
            path.write_text(original + '\nfn probe() { let amount: f64 = 1.0; }\n')
            run = subprocess.run([sys.executable, __file__], capture_output=True, text=True)
            assert run.returncode != 0 and 'Float' in run.stderr, run.stderr
        finally:
            path.write_text(original)
    for relative in ['rust/src/web/bot/write.rs', 'rust/src/web/bot/start.rs', 'rust/src/web/bot/draft.rs', 'rust/src/web/mcp/tools.rs', 'rust/src/sync/ledger.rs', 'rust/src/tracker/rows.rs']:
        path = root / relative
        original = path.read_text()
        for probe in ['SELECT quote_amount_exec FROM transactions', 'SELECT t.price FROM transactions t', 'SELECT t.* FROM transactions t', 'SELECT t.amount FROM bots b JOIN transactions t ON t.bot_id=b.id', 'SELECT amount {scope}']:
            try:
                path.write_text(original + '\nfn raw_probe() { let sql = "' + probe + '"; }\n')
                run = subprocess.run([sys.executable, __file__], capture_output=True, text=True)
                assert run.returncode != 0 and 'raw fill SELECT' in run.stderr, (relative, run.stderr)
            finally:
                path.write_text(original)
    print('PASS B2a gate sensitivity; sources restored', flush=True)
check()
