#!/usr/bin/env python3
"""Add three bay shrub samples east of the tall-tree stand, with review bookmarks."""
import argparse
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[
    ('a','upright',2782.,4310.,.3,1.),
    ('b','upright',2793.,4314.,2.2,.88),
    ('c','upright',2804.,4309.,4.1,1.06),
]
VIEWS=[
    ('bay-near',2782.,4308.,180.,5.,4.),
    ('bay-stand',2793.,4304.,180.,7.,8.),
    ('bay-overhead',2793.,4312.,0.,70.,17.),
]

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    args=parser.parse_args()
    overhead=args.project.with_suffix('.views')/'bay-overhead.ron'
    had_overhead=overhead.exists()
    place(args.project,SAMPLES,VIEWS,'bay','yarra_bay','bay_')
    if not had_overhead:
        with sqlite3.connect('file:'+str(args.project.resolve())+'?mode=ro',uri=True) as db:
            y=height(db,32.,2793.,4312.)+3.
        overhead.write_text(f'(position: (2793.0, {y:.6f}, 4312.0), yaw_degrees: 0.0, pitch_degrees: 70.0, distance: 17.0, fog_visibility: 20000.0, route: [])\n')
