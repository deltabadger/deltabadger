#!/usr/bin/env python3
"""Keep M3's accounting boundary and diff-based error gate executable after rebase."""
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[2]

def check():
    subprocess.run([sys.executable, str(root / 'script/rust/mcp_control_gate.py')], check=True)
    for folder in ['rust/src/figures', 'rust/src/web/figure']:
        for path in (root / folder).glob('*.rs'):
            if path.name == 'fill.rs':
                continue
            code = path.read_text()
            production = code.split('#[cfg(test)]')[0]
            assert not any(column in production for column in ['amount_exec', 'quote_amount_exec']), f'raw fill SQL outside predicate: {path}'
            assert '.raw.' not in code and '.raw;' not in code, f'raw fill column access outside predicate: {path}'
            for receiver in ['order', 'o']:
                for column in ['amount_exec', 'quote_amount_exec', 'price', 'amount', 'cost']:
                    assert f'{receiver}.{column}' not in code, f'raw fill column access outside predicate: {path}'
    print('PASS M3 swallow and raw-fill gates')

if '--sensitivity' in sys.argv:
    for relative, probe, marker in [
        ('rust/src/web/mcp/reads.rs', 'fn error_probe(s: &str) { s.parse::<i64>().unwrap(); }', 'new unchecked/discarded'),
        ('rust/src/figures/walk.rs', 'fn raw_probe(order: &Order) { let value = &order.raw.price; }', 'raw fill column access'),
    ]:
        path = root / relative
        original = path.read_text()
        try:
            path.write_text(original + '\n' + probe + '\n')
            result = subprocess.run([sys.executable, __file__], capture_output=True, text=True)
            assert result.returncode != 0 and marker in result.stderr, result.stderr
        finally:
            path.write_text(original)
    print('PASS M3 swallow/raw-fill gate sensitivity; sources restored')
check()
