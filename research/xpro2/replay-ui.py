#!/usr/bin/env python3
"""Replay the qualified menu navigation against freshly constructed virtual devices."""
import argparse
import json
import os
from pathlib import Path
import subprocess


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo',type=Path,required=True,help='Patched historical FujiMod checkout')
    parser.add_argument('--workspace',type=Path,help='Directory owning analysis; defaults to REPO')
    parser.add_argument('--lab',type=Path,required=True,help='Prepared directory below REPO/analysis')
    args=parser.parse_args();repo=args.repo.resolve();lab=args.lab.resolve();workspace=(args.workspace or repo).resolve()
    if not lab.is_relative_to(workspace/'analysis'):parser.error('--lab must be under WORKSPACE/analysis')
    executable=repo/'target/release/examples/xpro2_checkpoint'
    if not executable.is_file():parser.error('Build the xpro2_checkpoint example first')
    env={k:v for k,v in os.environ.items() if not k.startswith('XPRO2_')}
    controls=['selector.left','selector.up','selector.right']+['selector.down']*6+['selector.right','disp_back_button','selector.right']
    env.update(XPRO2_FIRMWARE=str((lab/'firmware.input').relative_to(workspace)),
       XPRO2_PRE_SNAPSHOT_IDLE_TICKS='96000000',XPRO2_SETTLE_TICKS='96000000',
       XPRO2_FOLLOWUP_CONTROLS=','.join(controls),XPRO2_FOLLOWUP_SETTLE_TICKS='12000000',
       XPRO2_FOLLOWUP_SETTLE_STEPS=','.join(['12000000','12000000','96000000']+['12000000']*6+['96000000']*3),
       XPRO2_CAPTURE_MENU_STEPS='9,10,11,12',XPRO2_IDLE_TICKS='12000000',
       XPRO2_PREFS_OBSERVE='1',XPRO2_THEME_OBSERVE='1',XPRO2_GUIDE_OBSERVE='1',
       XPRO2_DUMP_LCD='1',XPRO2_READ_WORDS='a961c,a9be0,a9be4,a9be8,a9bec')
    command=[str(executable),str(workspace),str((lab/'graph-resident-B.json').relative_to(workspace)),str((lab/'menu-start.bin').relative_to(workspace)),'menu_ok_button']
    with (lab/'ui.json').open('x') as report,(lab/'ui.log').open('x') as log:
        result=subprocess.run(command,cwd=workspace,env=env,stdout=report,stderr=log)
    if result.returncode:raise SystemExit(result.returncode)
    value=json.loads((lab/'ui.json').read_text());runs=value['controlRuns']
    assert value['diagnosticWords']['a961c']==0xa9be0
    assert runs[9]['menuState']==runs[11]['menuState'] and runs[9]['menuState'][4]==10
    assert runs[-1]['preferences']['error']==0
    print('PASS: FujiMods folder entry, return and re-entry; preferences loaded')

if __name__=='__main__':main()
