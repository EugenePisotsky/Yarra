#!/usr/bin/env python3
"""Place three tall broadleaf samples east of the maple stand.

Import assets/packs/yarra_tall_forest/tall_forest.catalog.ron, then run this and cook.
Backups and stable IDs preserve existing scenery and later editor adjustments.
"""
import argparse
import sqlite3
from pathlib import Path
from place_birch_preview import ROOT, place, height

SAMPLES=[
    ('near','forest',2712.,4312.,.3,1.),
    ('middle','forest',2736.,4308.,2.2,.92),
    ('far','forest',2760.,4314.,4.1,1.04),
]
VIEWS=[
    ('tall-forest',2712.,4272.,180.,5.,24.),
    ('tall-forest-stand',2736.,4274.,180.,5.,24.),
    ('tall-forest-overhead',2736.,4310.,0.,75.,24.),
]

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    args=parser.parse_args()
    overhead=args.project.with_suffix('.views')/'tall-forest-overhead.ron'
    had_overhead=overhead.exists()
    place(args.project,SAMPLES,VIEWS,'tall-forest','yarra_tall_forest','tall_broadleaf_')
    # Aim at the canopy so a 24 m orbit clears these much taller trees.
    if not had_overhead:
        with sqlite3.connect('file:'+str(args.project.resolve())+'?mode=ro',uri=True) as db:
            y=height(db,32.,2736.,4310.)+12.
        overhead.write_text(f'(position: (2736.0, {y:.6f}, 4310.0), yaw_degrees: 0.0, pitch_degrees: 75.0, distance: 24.0, fog_visibility: 20000.0, route: [])\n')
