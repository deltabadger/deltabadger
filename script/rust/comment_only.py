#!/usr/bin/env python3
"""Proves a change touches comments only (PLAN-B11 §10 Q6): for every file changed between two commits, the file with
its comments removed is the same token for token on both sides. Usage: comment_only.py <base> [<head>] (no head: the working tree). Exits 1 naming
each file whose code changed, or that was added, deleted or renamed (those are never comment-only). Stdlib only."""
import re
import subprocess
import sys

# Rust alone is lexed; every other file type is reviewed as code (YAML block scalars, HTML attribute values, Python
# triple-quoted strings and template expressions defeat a small lexer: Codex plan r5, branch r2).
LANGS = {'rs': (['//'], [('/*', '*/')], ['"'])}


def lang_of(path):
    name = path.rsplit('/', 1)[-1]
    return LANGS.get(name.rsplit('.', 1)[-1]) if '.' in name else None


def strip(text, lang, rust):
    """`text` with its comments removed and the lines left blank by that dropped; every other byte kept."""
    if '\0' in text:
        raise ValueError('holds a NUL byte (the comment marker)')
    lines, blocks, quotes = lang
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        start = i - 1 if i and text[i - 1] in 'bc' else i  # r"…", br"…" and cr"…"
        if rust and c == 'r' and re.match(r'r#*"', text[i:]) and (start == 0 or not (text[start - 1].isalnum() or text[start - 1] == '_')):
            m = re.match(r'r(#*)"', text[i:])
            end = text.find('"' + m.group(1), i + len(m.group(0)))
            end = n if end < 0 else end + 1 + len(m.group(1))
            out.append(text[i:end]); i = end; continue
        if rust and c == "'":
            m = re.match(r"'(\\.[^']*|[^'\\])'", text[i:])  # a char literal; otherwise a lifetime
            if m:
                out.append(m.group(0)); i += len(m.group(0)); continue
        if c in quotes:
            j = i + 1
            while j < n and text[j] != c:
                j += 2 if text[j] == '\\' else 1
            out.append(text[i:j + 1]); i = j + 1; continue
        hit = next((m for m in lines if text.startswith(m, i)), None)
        if hit:
            j = text.find('\n', i)
            out.append('\0'); i = n if j < 0 else j; continue
        pair = next((p for p in blocks if text.startswith(p[0], i)), None)
        if pair:
            depth, j = 1, i + len(pair[0])
            while j < n and depth:
                if rust and text.startswith(pair[0], j):
                    depth += 1; j += len(pair[0])
                elif text.startswith(pair[1], j):
                    depth -= 1; j += len(pair[1])
                else:
                    j += 1
            out.append('\0'); i = j; continue
        out.append(c); i += 1
    # A comment leaves \0; a line that held one and is blank without it goes. Every other line, blank ones in
    # strings included, stays byte for byte.
    return [l.replace('\0', ' ') for l in ''.join(out).split('\n') if not ('\0' in l and not l.replace('\0', '').strip())]


def show(rev, path):
    if rev is None:
        return open(path, encoding='utf-8').read()
    return subprocess.run(['git', 'show', f'{rev}:{path}'], capture_output=True, check=True).stdout.decode()


def regular(modes):
    """Both sides of a  row are regular files (a symlink's target is not code to lex)."""
    return all(m.lstrip(':') in ('100644', '100755') for m in modes) and len({m.lstrip(':') for m in modes}) == 1


def main(base, head=None):
    diff = subprocess.run(['git', 'diff', '--raw', '--no-renames', '--no-abbrev', base] + ([head] if head else []), capture_output=True, text=True, check=True).stdout
    bad, checked = [], 0
    for row in diff.splitlines():
        meta, path = row.split('	', 1)
        status, modes = meta.split()[-1], meta.split()[:2]
        lang = lang_of(path)
        if not regular(modes):
            bad.append(f'{path}: not a regular file on both sides ({" -> ".join(m.lstrip(":") for m in modes)}): review as code'); continue
        if status != 'M' or lang is None:
            bad.append(f'{path}: {status} (not a comment-only change)' if status != 'M' else f'{path}: no comment syntax known')
            continue
        try:
            old, new = strip(show(base, path), lang, path.endswith('.rs')), strip(show(head, path), lang, path.endswith('.rs'))
        except ValueError as e:
            bad.append(f'{path}: {e}: review as code'); continue
        checked += 1
        if old != new:
            k = next((k for k, (a, b) in enumerate(zip(old, new)) if a != b), min(len(old), len(new)))
            bad.append(f'{path}: code differs at code line {k + 1}:\n  - {old[k] if k < len(old) else "<end>"}\n  + {new[k] if k < len(new) else "<end>"}')
    for b in bad:
        print(b)
    print(f'{checked} file(s) checked; ' + ('every change is inside a comment' if not bad else f'{len(bad)} not comment-only'))
    return 1 if bad else 0


def self_test():
    rs = LANGS['rs']
    same = lambda a, b, lang=rs: strip(a, lang, lang is rs) == strip(b, lang, lang is rs)
    assert same('let a = 1; // X\n', 'let a = 1; // Y\n')
    assert same('// one\n// two\nfn f() {}\n', '// one two\nfn f() {}\n')
    assert same('/* a /* b */ c */ x\n', '/* d */ x\n')
    assert same('fn f() {}\n\n// a\nfn g() {}\n', 'fn f() {}\n\n// a b\n// c\nfn g() {}\n')
    assert not same('let a = "a  b";\n', 'let a = "a b";\n')
    assert not same('let a = "// x";\n', 'let a = "// y";\n')
    assert not same('let a = r#"// x"#;\n', 'let a = r#"// y"#;\n')
    assert not same("let c = '\"'; let s = \"// x\";\n", "let c = '\"'; let s = \"// y\";\n")
    assert not same('let a = "x\n\ny";\n', 'let a = "x\ny";\n')
    assert not same('let a = r#"x\n   \ny"#;\n', 'let a = r#"x\ny"#;\n')
    assert not same('let a = br#"a"//old\n"#;\n', 'let a = br#"a"//new\n"#;\n')
    assert not same('let a = cr#"a"//old\n"#;\n', 'let a = cr#"a"//new\n"#;\n')
    assert not same('m!(a/**/b);\n', 'm!(ab);\n')
    try:
        strip('let a = b"a\0b";\n', rs, True); assert False
    except ValueError:
        pass
    assert regular([':100644', '100644']) and not regular([':120000', '120000']) and not regular([':100644', '120000']) and not regular([':100644', '100755'])
    for other in ('rust/templates/a.html', 'a.yml', 'a.py', 'a.sh', 'a.js', 'a.html', 'Dockerfile', 'a.sql', 'a.toml'):
        assert lang_of(other) is None, other
    print('self-test passed')


if __name__ == '__main__':
    sys.exit(self_test() if sys.argv[1:] == ['--self-test'] else main(*sys.argv[1:3]))
