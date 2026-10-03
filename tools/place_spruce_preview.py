#!/usr/bin/env python3
"""Place a spruce review stand beside the longleafs and birches, preserving edits."""
import argparse
import math
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[('a','forest',2525.,4346.,.30,1.),('b','forest',2538.,4346.,2.3,.86),
         ('c','forest',2551.,4346.,4.2,1.04)]
# Focus height and ray offset support whole-tree views beyond the 24 m orbit cap.
VIEWS=[('spruce-stand',2538.,4346.,0.,10.,24.,7.,20.),
       ('spruce-whole',2525.,4346.,0.,10.,24.,7.,6.),
       ('spruce-close',2525.,4346.,25.,12.,6.,3.8,0.),
       ('spruce-below',2525.,4346.,110.,5.,7.,2.2,0.),
       ('spruce-overhead',2525.,4346.,25.,70.,22.,8.,0.),
       ('spruce-far',2525.,4346.,0.,8.,24.,7.,65.)]

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    project=parser.parse_args().project.resolve();view_dir=project.with_suffix('.views')
    missing={v[0] for v in VIEWS if not (view_dir/(v[0]+'.ron')).exists()}
    place(project,SAMPLES,[v[:6] for v in VIEWS],'spruce','yarra_spruces','spruce_')
    with sqlite3.connect('file:'+str(project)+'?mode=ro',uri=True) as db:
        for name,x,z,yaw,pitch,distance,lift,offset in VIEWS:
            if name not in missing:continue
            y=height(db,32.,x,z)+lift;a=math.radians(yaw);e=math.radians(pitch)
            x+=math.sin(a)*math.cos(e)*offset;z+=math.cos(a)*math.cos(e)*offset;y+=math.sin(e)*offset
            (view_dir/(name+'.ron')).write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')

if __name__=='__main__':main()
