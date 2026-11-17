#!/usr/bin/env python3
"""Every relative link and backticked repo path in README.md, manual/ and each AGENTS.md resolves (PLAN-B11 B11-7).

    python3 script/rust/doc_links.py

A link resolves against its file's directory. A backticked path (in the developer docs: README, AGENTS.md and
manual/33; the user pages' backticked paths are inside an install's data folder) resolves against the file's directory
or the repo root, as a glob when it has one. A backticked token counts as a path when it has a / and its first segment
is an entry of the repo root or of the file's directory, or it starts with ./ or ../; URLs, env values and the like are
not checked.
"""
import os
import re
import subprocess
import sys

root = subprocess.run(['git', 'rev-parse', '--show-toplevel'], capture_output=True, text=True, check=True).stdout.strip()
os.chdir(root)
files = subprocess.run(['git', 'ls-files', 'README.md', 'manual/*.md', 'AGENTS.md', '**/AGENTS.md'],
                       capture_output=True, text=True, check=True).stdout.split()
# Tracked files and their directories: what a fresh checkout has, so a local run answers as CI does.
tracked = set()
for p in subprocess.run(['git', 'ls-files'], capture_output=True, text=True, check=True).stdout.split():
    while p and p not in tracked:
        tracked.add(p)
        p = os.path.dirname(p)

# Inline links ([x](path), [x](<path>), [x](path "title")) and reference definitions ([x]: path).
link = re.compile(r'\]\(\s*(?:<([^>]+)>|([^)\s]+))|^\s{0,3}\[[^\]]+\]:\s*(?:<([^>]+)>|(\S+))')
tick = re.compile(r'`([^`\s]+)`')
fence = re.compile(r'^\s{0,3}(`{3,}|~{3,})')


def exists(base, path):
    path = path.split('#')[0].rstrip('/')
    if not path:
        return True
    path = os.path.normpath(os.path.join(base, path))
    if any(c in path for c in '*?['):
        # Segment-aware: * and ? stay inside one directory, ** spans any number of them.
        pattern = re.escape(path).replace(r'\*\*/', '(?:.*/)?').replace(r'\*\*', '.*').replace(r'\*', '[^/]*').replace(r'\?', '[^/]')
        return any(re.fullmatch(pattern, t) for t in tracked)
    return path in tracked


bad = []
for f in files:
    here = os.path.dirname(f) or '.'
    in_fence = None
    developer = not f.startswith('manual/') or f == 'manual/33-development.md'
    for n, line in enumerate(open(f, encoding='utf-8'), 1):
        opened = fence.match(line)
        if in_fence:
            # A fence closes on the same character, at least as long, with nothing after it.
            if opened and opened.group(1)[0] == in_fence[0] and len(opened.group(1)) >= len(in_fence) and not line[opened.end():].strip():
                in_fence = None
            continue
        if opened:
            in_fence = opened.group(1)
            continue
        for target in (next(g for g in m if g) for m in link.findall(line)):
            if re.match(r'[a-z]+:|#|/', target):
                continue
            if not exists(here, target):
                bad.append(f'{f}:{n}: link {target}')
        for token in tick.findall(line) if developer else ():
            token = token.rstrip('.,;:')
            if token.startswith('/'):
                continue
            relative = token.startswith(('./', '../'))
            first = token.split('/')[0]
            if not relative and ('/' not in token or (first not in tracked and os.path.normpath(os.path.join(here, first)) not in tracked)):
                continue
            if not (exists(here, token) or (not relative and exists('.', token))):
                bad.append(f'{f}:{n}: path {token}')

print('\n'.join(bad) or f'doc links: {len(files)} files, all resolve')
sys.exit(1 if bad else 0)
