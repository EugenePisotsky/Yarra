#!/usr/bin/env python3
"""Add a forest, spreading and sparse oak east of the island's birch stand.

Import assets/packs/yarra_oaks/oaks.catalog.ron first, then cook afterward. The
shared placement helper backs up the source and preserves edits on repeat runs.
"""
import argparse
from pathlib import Path
from place_birch_preview import ROOT, place

SAMPLES = [
    ('forest', 'forest', 2558., 4314., .4, 1.),
    ('spreading', 'spreading', 2582., 4308., 1.3, 1.),
    ('sparse', 'sparse', 2608., 4314., 2.1, 1.),
]
VIEWS = [
    ('oak-forest', 2558., 4287., 180., 5., 24.),
    ('oak-spreading', 2582., 4281., 180., 5., 24.),
    ('oak-sparse', 2608., 4287., 180., 5., 24.),
    ('oak-overhead', 2582., 4310., 0., 75., 24.),
]

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, default=ROOT/'content/world.project.sqlite')
    args = parser.parse_args()
    place(args.project, SAMPLES, VIEWS, 'oak', 'yarra_oaks', 'oak_')
