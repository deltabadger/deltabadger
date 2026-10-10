#!/usr/bin/env python3
"""R7 SQL-SUM and Float-money tripwires. Rust privacy enforces commitment ownership.
Generic numeric primitives remain byte pinned; no spelling-based duplicate-sum gate.
"""
import pathlib,re,subprocess,sys,json,collections
root=pathlib.Path(__file__).resolve().parents[2]
owner='rust/src/engine/accounting.rs'

def check():
    subprocess.run([sys.executable,str(root/"script/rust/histories_timestamp_gate.py")],check=True)
    # Generic numeric libraries implement operations, not application accounting. Freeze
    # their complete production bytes in the manifest; no new consumer can use this exception.
    import hashlib
    primitives=json.loads((root/'script/rust/histories_arithmetic_primitives.json').read_text(encoding='utf-8'))
    for relative,digest in primitives.items():
        assert hashlib.sha256((root/relative).read_bytes()).hexdigest()==digest, f'changed accounting primitive: {relative}'
    for path in (root/'rust/src').rglob('*.rs'):
        relative=path.relative_to(root).as_posix()
        code=path.read_text(encoding='utf-8')
        code=re.sub(r'"(?:[^"\\]|\\.)*"|//[^\n]*|/\*.*?\*/', lambda m: m[0] if m[0].startswith(chr(34)) else '', code, flags=re.S)
        # No SQL aggregate may bypass normalization. index_redeploy_totals was the old
        # justified display-only exception; R5 removed that SQL, so the allowlist is empty.
        assert not re.search(r'\bSUM\s*\([^)]*\b(?:quote_amount_exec|amount_exec|quote_amount|amount|price|cost)\b',code,re.I), f'fill SUM SQL outside shared module: {relative}'
        if relative in primitives or relative==owner:continue
        assert not re.search(r'\blet\s+(?:mut\s+)?(?:quote_amount|amount_exec|quote_amount_exec|carry|pending|spent|cost|amount|price)\s*:\s*f(?:32|64)\b',code), f'Float money field outside shared module: {relative}'
        statements=re.split(r';|\n\s*\}',code)
        for statement in statements:
            arithmetic=re.sub(r'"(?:[^"\\]|\\.)*"','""',statement)
            money=re.search(r'\b(?:quote_amount|amount_exec|quote_amount_exec|carry|pending|spent|cost|amount|price)\b',arithmetic)
            float_op=re.search(r'\.to_f\s*\(\)\s*[+*/-]|[+*/-]\s*\w+(?:\.\w+)*\.to_f\s*\(|\bas\s+f64\s*[+*/-]|\blet\s+\w+\s*:\s*f(?:32|64)\s*=',arithmetic)
            # Numeric parsing/access/formatting are not arithmetic. Keep them separate
            # from the expression scan; this does not exempt a file or a monetary formula.
            arithmetic=re.sub(r'"(?:[^"\\]|\\.)*"','""',statement)
            if money and float_op and re.search(r'\s[+*/-]\s',arithmetic):
                raise AssertionError(f'Float money arithmetic outside shared module: {relative}: {statement[-250:]}')
        # Typed monetary parameters cannot gain Float arithmetic in an adapter.
        for signature,body in re.findall(r'\bfn\s+\w+\s*\((.*?)\)[^{]*\{(.*?)(?=\n(?:pub |fn |#)|\Z)',code,re.S):
            names=re.findall(r'\b(quote_amount|smart_quote_amount|amount|cost|price|carry|pending|spent)\s*:\s*(?:Option<)?f(?:32|64)',signature)
            for name in names:
                assert not re.search(r'\b'+name+r'\b[^;\n]*[+*/-]',body), f'Float money parameter arithmetic outside shared module: {relative}'
    print('PASS R7 raw SQL and Float tripwire',flush=True)

if '--sensitivity' in sys.argv:
    for relative in ['rust/src/web/bot/write.rs','rust/src/web/bot/start.rs','rust/src/web/bot/draft.rs','rust/src/web/mcp/reads.rs','rust/src/engine/tick.rs']:
        path=root/relative;original=path.read_text(encoding='utf-8')
        for probe in [
            'fn another_pending() { let amount: f64 = 1.0 + 2.0; }',
            'fn another_sql() { let query = "SELECT SUM(quote_amount_exec) FROM transactions"; }',
        ]:
            try:
                path.write_text(original+'\n'+probe+'\n',encoding='utf-8')
                run=subprocess.run([sys.executable,__file__],capture_output=True,text=True)
                assert run.returncode!=0 and 'outside shared module' in run.stderr,(relative,probe,run.stderr)
            finally:path.write_text(original,encoding='utf-8')
    print('PASS R6 arithmetic sensitivity; all sources restored',flush=True)
check()
