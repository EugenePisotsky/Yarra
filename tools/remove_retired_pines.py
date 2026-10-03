#!/usr/bin/env python3
"""Remove pre-longleaf pine registrations and local artifacts, then recook the world.

Run with game/editor closed. A source backup and moved artifacts are retained in
ignored tmp storage outside the asset tree. Safe to repeat on existing projects.
"""
import argparse
import json
import shutil
import sqlite3
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKS = ('yarra_pines', 'yarra_pines_v2', 'yarra_pines_branch', 'yarra_pine_branch_study')


def retired(key):
    return key.split('/', 1)[0] in PACKS


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project', type=Path, default=ROOT / 'content/world.project.sqlite')
    project = parser.parse_args().project.resolve()
    backup = ROOT / 'tmp' / ('retired-pines-' + str(time.time_ns()))
    backup.mkdir(parents=True)
    report = {'assets': [], 'placements_removed': 0, 'moved': []}
    with sqlite3.connect('file:' + str(project) + '?mode=rw', uri=True) as db:
        db.execute('PRAGMA foreign_keys=ON')
        with sqlite3.connect(backup / project.name) as target:
            db.backup(target)
        with db:
            db.execute('BEGIN IMMEDIATE')
            assets = [(aid, key) for aid, key in db.execute('SELECT asset_id,asset_key FROM source_assets')
                      if retired(key)]
            before = dict((row[0], row[1:]) for row in db.execute('SELECT * FROM object_placements'))
            removed = set()
            for aid, key in assets:
                # Collection references are serialized, not protected by SQL FKs.
                if db.execute('SELECT 1 FROM environment_presets WHERE instr(payload,?)>0 LIMIT 1', (aid,)).fetchone():
                    raise ValueError('Remove this asset from environment collections first: ' + key)
                definitions = db.execute('SELECT definition_id FROM object_definitions WHERE visual_asset_id=?', (aid,)).fetchall()
                for (did,) in definitions:
                    removed.update(row[0] for row in db.execute('SELECT object_id FROM object_placements WHERE definition_id=?', (did,)))
                    db.execute('DELETE FROM object_placements WHERE definition_id=?', (did,))
                    db.execute('DELETE FROM object_definitions WHERE definition_id=?', (did,))
                db.execute('DELETE FROM source_assets WHERE asset_id=?', (aid,))
                report['assets'].append(key)
            after = dict((row[0], row[1:]) for row in db.execute('SELECT * FROM object_placements'))
            if after != {oid: row for oid, row in before.items() if oid not in removed}:
                raise ValueError('Unrelated placement changed')
            if db.execute('PRAGMA foreign_key_check').fetchall():
                raise ValueError('Source world has broken foreign keys')
            report['placements_removed'] = len(removed)
            report['placements_preserved'] = len(after)
    local = ROOT / 'assets/local'
    paths = [p for p in local.iterdir()
             if any(p.name == name or p.name.startswith(name + '.previous-') for name in PACKS)]
    paths += list(project.with_suffix('.views').glob('pine-*.ron'))
    for source in sorted(paths):
        destination = backup / ('views' if source.suffix == '.ron' else 'local') / source.name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(source), destination)
        report['moved'].append(str(source.relative_to(ROOT)) if source.is_relative_to(ROOT) else str(source))
    (backup / 'removal.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))
    print('Backup:', backup)
    print('Run yarra-world-cook cook to publish the cleaned runtime catalog.')


if __name__ == '__main__':
    main()
