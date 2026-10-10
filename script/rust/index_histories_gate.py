#!/usr/bin/env python3
"""B2b structural gate (RULING-B2B-1 MQ4): no engine sizing path reads the walk's `contributed` figure.

Rails books a REGULAR buy's drained flight cash as already-owned money, so `contributed` can be below what was paid in.
That is a figures-page value only; this fails the moment anything outside display code reads it. The engine walk
(basket.rs) produces it on its own lines and nothing reads it back.
"""
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[2]
DISPLAY = ('rust/src/figures/', 'rust/src/web/figure', 'rust/src/web/mcp/reads.rs')
PRODUCER = {'rust/src/engine/basket.rs': [
    'pub struct Walk { pub amounts: HashMap<i64, BigDec>, pub restated_at_us: Option<i64>, pub contributed: BigDec, pub cash: BigDec }',
    'fn default() -> Self { Self { amounts: HashMap::new(), restated_at_us: None, contributed: BigDec::zero(), cash: BigDec::zero() } }',
    'w.contributed = BigDec::parse(&books.contributed.to_d().map_err(data)?.to_s_f()).map_err(data)?;',
]}
WORD = re.compile(r'\b(?:contributed|total_quote_amount_invested)\b')


def check():
    for path in sorted((root / 'rust/src').rglob('*.rs')):
        relative = path.relative_to(root).as_posix()
        if relative.startswith(DISPLAY):
            continue
        allowed = PRODUCER.get(relative, [])
        for line in path.read_text().splitlines():
            code = line.split('//', 1)[0].strip()
            if WORD.search(code) and code not in allowed:
                raise AssertionError(f'MQ4: contributed read outside display code: {relative}: {code}')
    print('PASS B2b MQ4: no sizing path reads contributed', flush=True)


if '--sensitivity' in sys.argv:
    for relative, probe in [
        ('rust/src/engine/tick.rs', 'fn mq4_probe(w: &super::basket::Walk) -> bool { w.contributed.is_zero() }'),
        ('rust/src/engine/basket.rs', 'fn mq4_probe(w: &Walk) -> bool { w.contributed.is_zero() }'),
        ('rust/src/engine/amount.rs', 'fn mq4_probe(m: &crate::figures::walk::Metrics) -> bool { m.total_quote_amount_invested.is_zero() }'),
        ('rust/src/web/bot/start.rs', 'fn mq4_probe(b: &crate::figures::books::Books) -> bool { b.contributed.is_zero() }'),
    ]:
        path = root / relative
        original = path.read_text()
        try:
            path.write_text(original + '\n' + probe + '\n')
            run = subprocess.run([sys.executable, __file__], capture_output=True, text=True)
            assert run.returncode != 0 and 'MQ4' in run.stderr, (relative, run.stderr)
        finally:
            path.write_text(original)
    print('PASS B2b MQ4 gate sensitivity; sources restored', flush=True)
check()
