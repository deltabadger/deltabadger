#!/usr/bin/env python3
"""R9: stored timestamp parsing has one owner, and fallible reads cannot disappear."""
from pathlib import Path
import re,subprocess,sys
root=Path(__file__).resolve().parents[2]
folders=['rust/src/engine','rust/src/web/bot','rust/src/web/mcp']
def check():
    for folder in folders:
        for path in (root/folder).rglob('*.rs'):
            code=re.sub(r'//[^\n]*|/\*.*?\*/','',path.read_text(),flags=re.S)
            assert not re.search(r'\bparse_from_\w+\s*\(|\b(?:DateTime|NaiveDateTime|NaiveDate|Time|Date|OffsetDateTime|PrimitiveDateTime)(?:::\s*<[^>]+>)?\s*::\s*parse\w*\s*\(|\.parse\s*::\s*<[^>]*(?:Date|Time)[^>]*>',code),f'timestamp parser outside codec: {path}'
            for statement in re.split(r';|\n\s*}',code):
                calls=list(re.finditer(r'\b(?:parse_time(?:_in_zone|_offset)?|optional_time)\s*\(',statement))
                for call in calls:
                    depth=1;end=call.end()
                    while end<len(statement) and depth:
                        if statement[end]=='(':depth+=1
                        if statement[end]==')':depth-=1
                        end+=1
                    tail=statement[end:]
                    bad=re.search(r'\.(?:ok|unwrap_or\w*|map_or\w*)\s*\(',tail)
                    if bad and '?' not in tail[:bad.start()]:
                        raise AssertionError(f'discarded timestamp parse error: {path}: {statement}')
                if re.search(r'\b(?:parse_time(?:_in_zone|_offset)?|optional_time)\b',statement) and re.search(r'filter_map\s*\(',statement):
                    raise AssertionError(f'discarded timestamp parse error: {path}')
            aliases=[name for name,rhs in re.findall(r'let\s+(\w+)\s*=\s*((?:crate::)?codec::(?:parse_time|optional_time)\([^;]+);',code) if '?' not in rhs]
            for alias in aliases:
                assert not re.search(r'\b'+alias+r'\s*\.(?:ok|unwrap_or\w*|map_or\w*)\s*\(',code),f'discarded timestamp parse error: {path}'
    print('PASS R9 shared timestamp parser and fallible adapters',flush=True)
if '--sensitivity' in sys.argv:
    for relative in ['rust/src/engine/model.rs','rust/src/engine/accounting.rs','rust/src/web/bot/write.rs','rust/src/web/bot/start.rs','rust/src/web/bot/draft.rs','rust/src/web/mcp/reads.rs']:
        path=root/relative;original=path.read_text()
        for probe in ['fn r9_probe(s:&str) { chrono::DateTime::parse_from_rfc3339(s).ok(); }',
                      'fn r9_probe(s:&str) { crate::codec::parse_time(s).unwrap_or_default(); }',
                      'fn r9_probe(s:&str) { let parsed = codec::parse_time(s); parsed.ok(); }',
                      'fn r9_probe(s:&str) { [s].iter().filter_map(|s|codec::parse_time(s).ok()); }',
                      'fn r9_probe(s:&str) { [s].into_iter().map(codec::parse_time).filter_map(Result::ok); }']:
            try:
                path.write_text(original+'\n'+probe+'\n')
                result=subprocess.run([sys.executable,__file__],capture_output=True,text=True)
                assert result.returncode!=0 and ('timestamp parser' in result.stderr or 'timestamp parse error' in result.stderr),(relative,result.stderr)
            finally:path.write_text(original)
    print('PASS R9 timestamp gate sensitivity; all sources restored',flush=True)
check()
