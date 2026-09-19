#!/usr/bin/env python3
"""Prepare/open the hill test landscape; default launch spawns at the summit."""
import argparse
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def cargo(package, args):
    subprocess.run(['cargo', 'run', '--release', '-p', package, '--',
                    *map(str, args)], cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['game', 'editor', 'prepare', 'descent'], nargs='?', default='game')
    parser.add_argument('--view', choices=['summit', 'slope', 'valley'], default='summit')
    parser.add_argument('--directory', type=Path, default=ROOT / 'tmp/hill-landscape')
    parser.add_argument('--recook', action='store_true', help='Publish existing hill source edits before opening')
    args, extra = parser.parse_known_args()
    if extra and extra[0] == '--':
        extra = extra[1:]
    elif extra:
        parser.error('Pass additional application arguments after --')
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    project, runtime = directory / 'project.sqlite', directory / 'runtime.sqlite'
    if not project.exists():
        cargo('yarra-world-cook', ['create-hill-fixture', project])
    if not runtime.exists() or args.recook or args.action == 'prepare':
        cargo('yarra-world-cook', ['cook', project, runtime, '--terrain-materials', ROOT / 'assets'])
    print(f'Hill source: {project}\nRuntime: {runtime}', flush=True)
    if args.action == 'prepare':
        return
    view = project.with_suffix('.views') / (args.view + '.ron')
    if args.action == 'descent':
        view = project.with_suffix('.views') / 'summit.ron'
        extra = ['--render-repro', 'landscape-descent', '--render-prepass', *extra]
    command = ['--terrain-lod', '--world-db', runtime, '--start-view', view, *extra]
    if args.action == 'editor':
        command += ['--project-db', project]
    cargo('yarra-app-editor' if args.action == 'editor' else 'yarra-app-game', command)


if __name__ == '__main__':
    main()
