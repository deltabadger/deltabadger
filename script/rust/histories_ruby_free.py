#!/usr/bin/env python3
"""Run the actual workflow's test command with Ruby absent from PATH."""
import os
import pathlib
import platform
import shutil
import subprocess
import tempfile

root=pathlib.Path(__file__).resolve().parents[2]
workflow=(root/'.github/workflows/rust.yml').read_text()
block=workflow.split('      - name: Test\n',1)[1].split('        run: |\n',1)[1]
lines=[]
for line in block.splitlines():
    if line and not line.startswith('          '): break
    lines.append(line[10:])
script='\n'.join(lines)
assert 'cargo nextest run' in script and 'platform_excluded.txt' in script
excluded=[l.split('#')[0].strip() for l in (root/'rust/tests/platform_excluded.txt').read_text().splitlines()]
assert not any(l.rsplit(':',1)[-1]=='histories_sources' for l in excluded if l)
with tempfile.TemporaryDirectory(prefix='no-ruby-',dir=root.parent) as tmp:
    for name in ['cargo','cargo-nextest','rustc','rustdoc','cc','clang','ar','xcrun','bash','sh','env','git','python3','uname','ld','sed','awk','grep','tr','basename','df','tail','sort','head','sleep','mktemp']:
        source=shutil.which(name)
        if source: pathlib.Path(tmp,name).symlink_to(source)
    env=os.environ.copy()
    env['PATH']=tmp
    env['RUNNER_OS']={'Darwin':'macOS','Windows':'Windows'}.get(platform.system(),'Linux')
    assert shutil.which('ruby',path=tmp) is None
    assert shutil.which('bundle',path=tmp) is None
    print('Ruby and Bundler absent from PATH; executing the workflow command',flush=True)
    subprocess.run([shutil.which('bash'),'-c',script],cwd=root/'rust',env=env,check=True)
