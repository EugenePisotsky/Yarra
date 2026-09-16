#!/usr/bin/env python3
"""Cook isolated authored-density variants; refuse a baseline that differs from the live world."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def backup(source, destination):
    with sqlite3.connect(source.resolve().as_uri() + '?mode=ro', uri=True) as src:
        with sqlite3.connect(destination) as dst:
            src.backup(dst)


def tables(path):
    """Hash ordered logical rows, including BLOB bytes; ignore SQLite layout/WAL differences."""
    result = {}
    with sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True) as db:
        names = [r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                 if not r[0].startswith('sqlite_')]
        for name in names:
            quoted = '"' + name.replace('"', '""') + '"'
            rows = []
            for row in db.execute('SELECT * FROM ' + quoted):
                encoded = [dict(blob=value.hex()) if isinstance(value, bytes) else value for value in row]
                rows.append(json.dumps(encoded, separators=(',', ':'), ensure_ascii=True))
            result[name] = hashlib.sha256('\n'.join(sorted(rows)).encode()).hexdigest()
    return result


def change_density(catalog, density):
    pattern = r'(key: "short_split_fill",(?:(?!key:).)*?density_per_square_meter:\s*)([0-9.]+)'
    result, count = re.subn(pattern, lambda m: m[1] + str(float(density)), catalog, flags=re.S)
    if count != 1:
        raise ValueError(f'Expected one short_split_fill population, found {count}')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--densities', nargs='+', type=int, default=[72, 96, 128])
    args = parser.parse_args()
    if len(set(args.densities)) != len(args.densities) or any(not 1 <= n <= 512 for n in args.densities):
        parser.error('Use distinct densities in 1..512')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    source_project = ROOT / 'content/demo.project.sqlite'
    source_runtime = ROOT / 'assets/generated/demo.runtime.sqlite'
    source_catalog = output / 'source-catalog.ron'
    subprocess.run([str(ROOT / 'target/debug/yarra-world-cook'), 'export-vegetation',
                    str(source_project), str(source_catalog)], check=True)
    catalog = source_catalog.read_text()
    match = re.search(r'key: "short_split_fill",(?:(?!key:).)*?density_per_square_meter:\s*([0-9.]+)', catalog, re.S)
    if not match:
        raise ValueError('Missing authored baseline density')
    baseline = float(match[1])
    # First recook the unchanged source catalog and prove that all runtime content matches.
    densities = list(dict.fromkeys([baseline, *args.densities]))
    before_project, before_runtime = tables(source_project), tables(source_runtime)
    manifest = {'baseline_density': baseline, 'variants': {}, 'baseline_runtime_tables': before_runtime}
    for density in densities:
        folder = output / f'density-{density:g}'
        folder.mkdir()
        backup(source_project, folder / 'project.sqlite')
        ron = folder / 'catalog.ron'
        ron.write_text(change_density(catalog, density))
        cmd = [str(ROOT / 'target/debug/yarra-world-cook'), 'import-vegetation', str(ron),
               str(folder / 'project.sqlite'), str(folder / 'runtime.sqlite')]
        with (folder / 'cook.log').open('w') as log:
            subprocess.run(cmd, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
        current = tables(folder / 'runtime.sqlite')
        changed = sorted(k for k in current.keys() | before_runtime.keys()
                         if current.get(k) != before_runtime.get(k))
        allowed = {'runtime_metadata'} if density == baseline else {'runtime_metadata', 'vegetation_catalog'}
        if set(changed) - allowed:
            raise ValueError(f'Recooked world differs outside density/metadata: {changed}')
        if density != baseline and 'vegetation_catalog' not in changed:
            raise ValueError('Density failed to change the cooked catalog')
        manifest['variants'][f'{density:g}'] = {'world_db': str(folder / 'runtime.sqlite'),
            'catalog_sha256': hashlib.sha256(ron.read_bytes()).hexdigest(),
            'runtime_tables': current, 'changed_tables': changed}
        print(f'{density:g} roots/m²: verified; changed tables {changed}', flush=True)
    if tables(source_project) != before_project or tables(source_runtime) != before_runtime:
        raise RuntimeError('Source databases changed during preparation; do not use these variants')
    manifest['source_databases_unchanged'] = True
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')


if __name__ == '__main__':
    main()
