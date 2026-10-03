#!/usr/bin/env python3
"""Place dead broadleaf forms beside the existing tree collection, preserving edits."""
import argparse
import math
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[('upright','upright',2428.,4344.,.35,1.),
         ('spreading','spreading',2443.,4344.,.7,1.),
         ('split','split',2458.,4344.,2.1,1.)]
# Keep the review row beside the longleafs, on dry terrain near the birches.
VIEWS=[('dead-trees-walk',2443.,4337.,180.,5.,15.,0.,0.),
       ('dead-trees-stand',2443.,4344.,180.,8.,24.,6.,18.),
       ('dead-trees-upright',2428.,4344.,0.,8.,24.,7.,4.),
       ('dead-trees-spreading',2443.,4344.,0.,8.,20.,5.,0.),
       ('dead-trees-split',2458.,4344.,35.,8.,22.,6.,0.),
       ('dead-trees-close',2443.,4344.,25.,10.,6.,4.,0.),
       ('dead-trees-bark',2443.,4344.,195.,5.,4.,1.,0.),
       ('dead-trees-twigs',2440.,4344.,180.,10.,5.,7.,0.),
       ('dead-trees-overhead',2443.,4344.,25.,70.,22.,6.,0.),
       ('dead-trees-far',2443.,4344.,0.,8.,24.,6.,70.)]

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    project=parser.parse_args().project.resolve();view_dir=project.with_suffix('.views')
    missing={v[0] for v in VIEWS if not (view_dir/(v[0]+'.ron')).exists()}
    place(project,SAMPLES,[v[:6] for v in VIEWS],'dead-trees','yarra_dead_trees','dead_')
    with sqlite3.connect('file:'+str(project)+'?mode=ro',uri=True) as db:
        for name,x,z,yaw,pitch,distance,lift,offset in VIEWS:
            if name not in missing:continue
            y=height(db,32.,x,z)+lift;a=math.radians(yaw);e=math.radians(pitch)
            x+=math.sin(a)*math.cos(e)*offset;z+=math.cos(a)*math.cos(e)*offset;y+=math.sin(e)*offset
            (view_dir/(name+'.ron')).write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')

if __name__=='__main__':main()
