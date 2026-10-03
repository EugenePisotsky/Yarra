#!/usr/bin/env python3
"""Place small and medium forest shrubs near the birches, preserving scenery and edits."""
import argparse
import math
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[('rounded','rounded',2490.,4328.,.3,1.),
         ('spreading','spreading',2496.,4328.,1.4,1.),
         ('sparse','sparse',2502.,4328.,2.5,1.),
         ('medium_rounded','medium_rounded',2466.,4328.,.4,1.),
         ('medium_spreading','medium_spreading',2474.,4328.,1.5,1.),
         ('medium_upright','medium_upright',2482.,4328.,2.6,1.)]
# A clear gap between the birches and the longleaf preview row.
VIEWS=[('shrubs-walk',2493.,4333.,0.,5.,7.,0.,0.),
       ('shrubs-stand',2496.,4330.,0.,12.,14.,.5,0.),
       ('shrubs-rounded',2490.9,4328.,0.,12.,4.5,.45,0.),
       ('shrubs-spreading',2497.1,4328.,0.,15.,4.5,.25,0.),
       ('shrubs-sparse',2503.,4328.,0.,12.,4.5,.6,0.),
       ('shrubs-close',2490.8,4328.,25.,10.,4.,.5,0.),
       ('shrubs-roots',2502.7,4328.,25.,8.,4.,.05,0.),
       ('shrubs-overhead',2496.,4328.,0.,70.,11.,.3,0.),
       ('shrubs-far',2496.,4330.,0.,8.,24.,.4,0.),
       ('shrubs-medium-walk',2478.,4334.,0.,5.,8.,0.,0.),
       ('shrubs-medium-stand',2474.,4328.,0.,12.,14.,.8,0.),
       ('shrubs-medium-rounded',2468.,4328.,0.,12.,7.,.6,0.),
       ('shrubs-medium-spreading',2476.5,4328.,0.,15.,7.,.4,0.),
       ('shrubs-medium-upright',2483.8,4328.,0.,12.,7.,.9,0.),
       ('shrubs-medium-close',2467.4,4328.,25.,12.,4.,.6,0.),
       ('shrubs-medium-overhead',2474.,4328.,0.,70.,16.,.6,0.),
       ('shrubs-medium-far',2474.,4328.,12.,8.,24.,.4,0.)]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    project=parser.parse_args().project.resolve();view_dir=project.with_suffix('.views')
    missing={v[0] for v in VIEWS if not (view_dir/(v[0]+'.ron')).exists()}
    place(project,SAMPLES,[v[:6] for v in VIEWS],'shrubs','yarra_shrubs','shrub_')
    with sqlite3.connect('file:'+str(project)+'?mode=ro',uri=True) as db:
        for name,x,z,yaw,pitch,distance,lift,offset in VIEWS:
            if name not in missing:continue
            y=height(db,32.,x,z)+lift;a=math.radians(yaw);e=math.radians(pitch)
            x+=math.sin(a)*math.cos(e)*offset;z+=math.cos(a)*math.cos(e)*offset;y+=math.sin(e)*offset
            (view_dir/(name+'.ron')).write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')

if __name__=='__main__':main()
