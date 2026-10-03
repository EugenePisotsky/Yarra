#!/usr/bin/env python3
"""Add three fern forms in the gap beside the birch/shrub review collection."""
import argparse
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[('upright','upright',2490.,4342.,.3,1.),
         ('spreading','spreading',2494.,4342.,1.8,1.),
         ('sparse','sparse',2498.,4342.,3.1,1.)]
VIEWS=[('ferns-walk',2494.,4346.,0.,10.,5.,0.),
       ('ferns-stand',2494.,4342.,0.,28.,8.,-.35),
       ('ferns-upright',2491.35,4342.,0.,25.,4.,-.55),
       ('ferns-spreading',2495.6,4342.,0.,30.,4.5,-.65),
       ('ferns-sparse',2499.25,4342.,0.,25.,4.,-.65),
       ('ferns-close',2491.06,4341.25,35.,25.,4.,-.55),
       ('ferns-side',2490.,4340.7,90.,8.,4.,-.55),
       ('ferns-overhead',2495.5,4342.,0.,75.,6.,-.6),
       ('ferns-far',2494.,4342.,0.,18.,16.,-.3)]

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    project=parser.parse_args().project.resolve();view_dir=project.with_suffix('.views')
    missing={v[0] for v in VIEWS if not (view_dir/(v[0]+'.ron')).exists()}
    place(project,SAMPLES,[v[:6] for v in VIEWS],'ferns','yarra_ferns','fern_')
    with sqlite3.connect('file:'+str(project)+'?mode=ro',uri=True) as db:
        for name,x,z,yaw,pitch,distance,lift in VIEWS:
            if name in missing:
                y=height(db,32.,x,z)+lift
                (view_dir/(name+'.ron')).write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')

if __name__=='__main__':main()
