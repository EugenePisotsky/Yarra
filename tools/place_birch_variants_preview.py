#!/usr/bin/env python3
"""Place new birch forms beside the existing tree collection, preserving edits."""
import argparse
import math
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[('crown','crown',2490.,4356.,.35,1.),
         ('pendulous','pendulous',2504.,4356.,.7,1.),
         ('double','double',2519.,4356.,2.1,1.),
         ('triple','triple',2535.,4356.,4.1,1.)]
# Focus lift and ray offset allow full crowns beyond the 24 m orbit cap.
VIEWS=[('birch-variants-walk',2511.,4358.,0.,5.,24.,0.,0.),
       ('birch-variants-stand',2512.,4356.,0.,8.,24.,7.,27.),
       ('birch-variants-crown',2490.,4356.,0.,8.,24.,7.,4.),
       ('birch-variants-pendulous',2504.,4356.,0.,8.,24.,7.,4.),
       ('birch-variants-double',2519.,4356.,45.,8.,24.,7.,4.),
       ('birch-variants-triple',2535.,4356.,0.,8.,24.,7.,4.),
       ('birch-variants-close',2504.,4356.,35.,10.,7.,6.,0.),
       ('birch-variants-bare',2490.,4356.,25.,8.,6.,5.,0.),
       ('birch-variants-roots',2535.,4356.,25.,10.,5.,1.,0.),
       ('birch-variants-overhead',2535.,4356.,25.,70.,22.,8.,0.),
       ('birch-variants-far',2512.,4356.,0.,8.,24.,7.,80.)]

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    project=parser.parse_args().project.resolve();view_dir=project.with_suffix('.views')
    missing={v[0] for v in VIEWS if not (view_dir/(v[0]+'.ron')).exists()}
    place(project,SAMPLES,[v[:6] for v in VIEWS],'birch-variants','yarra_birches','birch_')
    with sqlite3.connect('file:'+str(project)+'?mode=ro',uri=True) as db:
        for name,x,z,yaw,pitch,distance,lift,offset in VIEWS:
            if name not in missing:continue
            y=height(db,32.,x,z)+lift;a=math.radians(yaw);e=math.radians(pitch)
            x+=math.sin(a)*math.cos(e)*offset;z+=math.cos(a)*math.cos(e)*offset;y+=math.sin(e)*offset
            (view_dir/(name+'.ron')).write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')

if __name__=='__main__':main()
