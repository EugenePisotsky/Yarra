#!/usr/bin/env python3
"""Capture the wood-sorrel forms under native/reduced game settings (requires Pillow)."""
from pathlib import Path
import argparse
import subprocess
import time
from PIL import Image
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'tmp/sorrel-review';OUT.mkdir(parents=True,exist_ok=True)
parser=argparse.ArgumentParser(description=__doc__)
views=('stand','open','full','patch','close','side','overhead','far')
parser.add_argument('--view',choices=views,action='append',help='Render selected views only; defaults to all')
parser.add_argument('--mode',choices=('native','half'),action='append',help='Render selected resolutions only')
parser.add_argument('--game',type=Path,default=ROOT/'target/release/yarra-app-game',help='Existing game executable to render with')
options=parser.parse_args()
for view in options.view or views:
    for mode in options.mode or (('native','half') if view.endswith('close') else ('native',)):
        name=view+'-'+mode
        started=time.time_ns()
        print('Rendering '+name,flush=True)
        args=[str(options.game.resolve()),'--start-view',str(ROOT/f'content/world.project.views/sorrel-{view}.ron'),
              '--render-repro','landscape','--render-frames','900','--render-snapshot',str(OUT/(name+'.png')),
              '--render-snapshot-frames','850','--render-ui-off','--profile-diagnostic',
              '--profile-size','1920x1080' if mode=='native' else 'game','--profile-surface','1920x1080',
              '--profile-window','windowed','--profile-fps','60','--upscaler','linear' if mode=='native' else 'metalfx-temporal']
        with (OUT/(name+'.log')).open('w') as f:subprocess.run(args,cwd=ROOT,stdout=f,stderr=subprocess.STDOUT,check=True)
        snapshot=OUT/(name+'.png')
        if not snapshot.exists() or snapshot.stat().st_mtime_ns < started:
            raise RuntimeError('Game produced no fresh snapshot; inspect '+str(OUT/(name+'.log')))
        with Image.open(snapshot) as im:
            if not im.convert('RGB').getbbox():
                raise RuntimeError('Game produced a black snapshot; inspect '+str(OUT/(name+'.log')))
print('Sorrel captures complete',flush=True)
