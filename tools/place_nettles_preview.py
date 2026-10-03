#!/usr/bin/env python3
"""Add three common-nettle forms in the gap beside the birch/shrub review collection."""
import argparse
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[('young','young',2490.,4353.,.3,1.),
         ('mature','mature',2493.,4353.,1.8,1.),
         ('patch','patch',2496.,4353.,3.1,1.)]
VIEWS=[('nettles-walk',2493.,4356.,0.,12.,5.,0.),
       ('nettles-stand',2493.,4354.,0.,35.,6.,-.55),
       ('nettles-young',2490.65,4353.,0.,30.,4.,-.40),
       ('nettles-mature',2493.5,4353.,0.,30.,4.,-.40),
       ('nettles-patch',2496.65,4353.,0.,30.,4.,-.40),
       ('nettles-close',2493.5,4352.5,35.,30.,4.,-.40),
       ('nettles-side',2493.,4352.25,90.,8.,4.,-.40),
       ('nettles-overhead',2493.6,4353.,0.,75.,4.,-.40),
       ('nettles-far',2493.,4354.,0.,20.,14.,-.5)]

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    project=parser.parse_args().project.resolve();view_dir=project.with_suffix('.views')
    missing={v[0] for v in VIEWS if not (view_dir/(v[0]+'.ron')).exists()}
    place(project,SAMPLES,[v[:6] for v in VIEWS],'nettles','yarra_nettles','nettle_')
    with sqlite3.connect('file:'+str(project)+'?mode=ro',uri=True) as db:
        for name,x,z,yaw,pitch,distance,lift in VIEWS:
            if name in missing:
                y=height(db,32.,x,z)+lift
                (view_dir/(name+'.ron')).write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')

if __name__=='__main__':main()
