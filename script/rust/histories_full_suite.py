#!/usr/bin/env python3
"""R9c: full Rust suite exactly once; every failure stops proof, without retries."""
from pathlib import Path
import subprocess,os,json,time
root=Path(__file__).resolve().parents[2]
evidence=Path(os.environ['EVIDENCE'])
free=int(subprocess.check_output(['df','-g','/'],text=True).splitlines()[1].split()[3])
if free<15:raise SystemExit('STOP below 15 GB before full Rust suite')
command=['cargo','nextest','run','-j','5']
started=time.time()
with (evidence/'06-full-rust.log').open('w') as log:
    result=subprocess.run(command,cwd=root/'rust',stdout=log,stderr=subprocess.STDOUT)
record={'command':command,'full_exit':result.returncode,'elapsed_s':time.time()-started,'invocations':1,'retries':0,'ruling':'R9c; pinned base includes #509'}
(evidence/'06-full-result.json').write_text(json.dumps(record,indent=2)+'\n')
print((evidence/'06-full-rust.log').read_text(),flush=True)
raise SystemExit(result.returncode)
