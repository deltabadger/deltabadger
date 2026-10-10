#!/usr/bin/env python3
"""Prove that validation's production writer cannot accept an unproduced status."""
import argparse
import os
from pathlib import Path
import subprocess

parser=argparse.ArgumentParser()
parser.add_argument('--old-red',action='store_true')
parser.add_argument('--root',help='optional development-only source export')
args=parser.parse_args()
root=Path(args.root).resolve() if args.root else Path(__file__).resolve().parents[2]
assert 'CARGO_TARGET_DIR' in os.environ and 'S1_R4_MUTATION' not in os.environ
evidence=Path(os.environ['S1_EVIDENCE']);evidence.mkdir(parents=True,exist_ok=True)
probe=root/'rust/tests/r4_validation_status_type.rs';assert not probe.exists()
try:
    free=os.statvfs('/data');assert free.f_bavail*free.f_frsize>=20*1024**3
    subprocess.run(['df','-h','/data'],check=True)
    probe.write_text("use deltabadger::{engine::model::FencedTransaction,web::settings::keys::store_status};\nfn raw_status(c:&FencedTransaction<'_>){let _=store_status(c,1,1,chrono::Utc::now());}\n")
    result=subprocess.run(['cargo','check','--manifest-path','rust/Cargo.toml','--locked','--test','r4_validation_status_type'],cwd=root,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
    label='old-accepted' if args.old_red else 'rejected'
    (evidence/('r4-validation-status-'+label+'.log')).write_text(result.stdout)
    if args.old_red:
        assert result.returncode==0,result.stdout[-4000:]
        print('RED: old validation writer accepts raw status with no result producer (compiled)')
    else:
        assert result.returncode==101 and 'error[E0308]' in result.stdout and '&Produced<i64>' in result.stdout,result.stdout[-4000:]
        assert 'error[E0061]' not in result.stdout and 'error[E0463]' not in result.stdout
        print('PASS: raw validation status writer probe rejected with E0308; carried Produced<i64> required')
finally:probe.unlink(missing_ok=True)
