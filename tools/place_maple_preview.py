#!/usr/bin/env python3
"""Add forest, spreading and sparse maples east of the island's oak stand.

Import assets/packs/yarra_maples/maples.catalog.ron first, then cook afterward.
The shared helper backs up the source and preserves edits on repeat runs.
"""
import argparse
from pathlib import Path
from place_birch_preview import ROOT, place

SAMPLES = [
    ('forest', 'forest', 2634., 4314., .4, 1.),
    ('spreading', 'spreading', 2658., 4308., 1.3, 1.),
    ('sparse', 'sparse', 2684., 4314., 2.1, 1.),
]
VIEWS = [
    ('maple-forest', 2634., 4287., 180., 5., 24.),
    ('maple-spreading', 2658., 4281., 180., 5., 24.),
    ('maple-sparse', 2684., 4287., 180., 5., 24.),
    ('maple-overhead', 2658., 4310., 0., 75., 24.),
]

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, default=ROOT/'content/world.project.sqlite')
    args = parser.parse_args()
    place(args.project, SAMPLES, VIEWS, 'maple', 'yarra_maples', 'maple_')
