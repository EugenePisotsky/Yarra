#!/usr/bin/env python3
"""Add two isolated branch samples beside the birch/pine review stands."""
import sqlite3
from place_birch_preview import ROOT, place, height

SAMPLES=[('single','single',2491.,4338.,0.,1.),('depth','depth',2497.,4338.,0.,1.)]
VIEWS=[('pine-branch-single',2491.,4338.,180.,5.,4.,1.0),
       ('pine-branch-depth',2497.,4338.,180.,5.,4.,1.0),
       ('pine-branch-below',2497.,4338.,160.,5.,5.,.45),
       ('pine-branch-overhead',2497.,4338.,165.,70.,4.,1.0),
       ('pine-branch-compare',2494.,4338.,180.,5.,9.,1.0)]

if __name__=='__main__':
    project=ROOT/'content/world.project.sqlite';view_dir=project.with_suffix('.views')
    missing={v[0] for v in VIEWS if not (view_dir/(v[0]+'.ron')).exists()}
    place(project,SAMPLES,[v[:6] for v in VIEWS],'pine-branch-study','yarra_pine_branch_study','pine_branch_')
    with sqlite3.connect('file:'+str(project.resolve())+'?mode=ro',uri=True) as db:
        for name,x,z,yaw,pitch,distance,lift in VIEWS:
            if name not in missing:continue
            y=height(db,32.,x,z)+lift
            (view_dir/(name+'.ron')).write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')
