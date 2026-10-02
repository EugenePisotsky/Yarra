#!/usr/bin/env python3
"""Place nine birch samples near the current island start without replacing scenery.

Run after importing assets/packs/yarra_birches/birches.catalog.ron. Backs up the
source database before its transaction. Stable IDs make repeated runs additive
and preserve any subsequent editor adjustments. Cook the world afterward.
"""
import argparse
import json
import math
import sqlite3
import struct
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SAMPLES = [
    ('near_a', 'leafy', 2492., 4316., .15, 1.),
    ('near_b', 'leafy', 2506., 4311., 2.3, .95),
    ('near_c', 'sparse', 2516., 4301., 4.1, 1.),
    ('west_a', 'leafy', 2472., 4282., 1.2, 1.04),
    ('west_b', 'leafy', 2482., 4273., 3.5, .90),
    ('west_c', 'bare', 2464., 4265., .7, 1.),
    ('east_a', 'leafy', 2532., 4260., 5.1, .98),
    ('east_b', 'leafy', 2546., 4244., 2., 1.05),
    ('east_c', 'sparse', 2524., 4234., 4.7, .88),
]
VIEWS = [
    ('birch-near', 2500., 4342., 0., 5., 24.),
    ('birch-west', 2475., 4304., 10., 5., 24.),
    ('birch-east', 2534., 4290., 0., 5., 24.),
    ('birch-overhead', 2498., 4315., 25., 75., 24.),
]


def height(db, size, x, z):
    cx, cz = math.floor(x/size), math.floor(z/size)
    row = db.execute('SELECT resolution,heights FROM terrain_cell_heightfields WHERE world_space_id=1 AND cell_x=? AND cell_z=?', (cx, cz)).fetchone()
    if not row:
        return db.execute('SELECT height FROM source_cells WHERE world_space_id=1 AND cell_x=? AND cell_z=?', (cx, cz)).fetchone()[0]
    n, data = row
    h = struct.unpack('<' + 'f'*(n*n), data)
    px, pz = (x-cx*size)/size*(n-1), (z-cz*size)/size*(n-1)
    ix, iz = min(int(px), n-2), min(int(pz), n-2)
    fx, fz = px-ix, pz-iz
    return (h[iz*n+ix]*(1-fx)+h[iz*n+ix+1]*fx)*(1-fz)+(h[(iz+1)*n+ix]*(1-fx)+h[(iz+1)*n+ix+1]*fx)*fz


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, default=ROOT/'content/world.project.sqlite')
    args = parser.parse_args()
    db = sqlite3.connect('file:'+str(args.project.resolve())+'?mode=rw', uri=True)
    db.execute('PRAGMA foreign_keys=ON')
    row = db.execute('SELECT name,cell_size FROM world_spaces WHERE id=1').fetchone()
    if row != ('Island', 32.):
        raise ValueError('These preview coordinates are for the current 32 m island world only')
    size = row[1]
    backup = ROOT/'tmp'/('birch-placement-'+str(time.time_ns()))
    backup.mkdir(parents=True)
    with sqlite3.connect(backup/'world.project.sqlite') as target:
        db.backup(target)
    before = db.execute('SELECT count(*) FROM object_placements').fetchone()[0]
    report = []
    with db:
        db.execute('BEGIN IMMEDIATE')
        for name, state, x, z, yaw, scale in SAMPLES:
            object_id = uuid.uuid5(uuid.NAMESPACE_URL, 'yarra:birch-preview/v1/'+name).bytes
            definition = db.execute('SELECT definition_id FROM object_definitions WHERE definition_key=?', ('asset/yarra_birches/birch_'+state,)).fetchone()
            if definition is None:
                raise ValueError('Import the birch catalog before placing trees')
            existing = db.execute('SELECT definition_id FROM object_placements WHERE object_id=?', (object_id,)).fetchone()
            if existing and existing != definition:
                raise ValueError('Preview object ID is already used by another definition')
            cx, cz = math.floor(x/size), math.floor(z/size)
            y = height(db, size, x, z)-.025
            if y <= 0:
                raise ValueError('Refusing to plant below sea level')
            if not existing:
                db.execute('INSERT INTO object_placements VALUES (?,?,?,?,?,?,?,?,?,?,1)',
                    (object_id, 1, cx, cz, definition[0], x-cx*size, y, z-cz*size, yaw, scale))
            report.append({'name': name, 'object_id': object_id.hex(), 'state': state,
                           'position': [x, y, z], 'scale': scale, 'created': not bool(existing)})
        assert not db.execute('PRAGMA foreign_key_check').fetchall()
        assert db.execute('SELECT count(*) FROM object_placements').fetchone()[0] == before+sum(r['created'] for r in report)
    views = args.project.with_suffix('.views')
    views.mkdir(exist_ok=True)
    for name, x, z, yaw, pitch, distance in VIEWS:
        path = views/(name+'.ron')
        if not path.exists():
            path.write_text(f'(position: ({x}, {height(db,size,x,z):.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')
    (backup/'placements.json').write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps(report, indent=2))
    print('Source backup and placement IDs:', backup)


if __name__ == '__main__':
    main()
