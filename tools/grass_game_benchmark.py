#!/usr/bin/env python3
"""Matched native game runs with isolated shader assets and an explicit runtime database.

Does not modify the live assets/catalog. Metal HUD reports whole-game GPU duration, not an
isolated grass pass. Run sequentially, with other GPU applications closed or suspended.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--world-db', required=True, type=Path)
    parser.add_argument('--shaders', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/yarra-app-game')
    parser.add_argument('--view', choices=['low-walk', 'grass-close', 'grass-away', 'grass-zoom', 'grass-overhead', 'grass-top-down'], default='low-walk')
    parser.add_argument('--frames', type=int, default=3600)
    parser.add_argument('--counters', action='store_true')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    assets = output / 'assets'
    assets.mkdir()
    for source in (ROOT / 'assets').iterdir():
        if source.name != 'shaders':
            (assets / source.name).symlink_to(source, target_is_directory=source.is_dir())
    shutil.copytree(ROOT / 'assets/shaders', assets / 'shaders')
    for source in args.shaders.glob('*.wgsl'):
        shutil.copy2(source, assets / 'shaders' / source.name)
    command = [str(args.binary.resolve()), '--world-db',
               str(args.world_db.resolve()), '--render-repro', args.view,
               '--render-frames', str(args.frames), '--render-snapshot', str(output / 'game.png')]
    if args.counters:
        command.append('--grass-counters')
    metadata = {
        'scope': 'Whole-game Metal HUD GPU duration; not isolated grass or phone performance',
        'command': command,
        'wind_phase_policy': ('fixed_zero' if args.view in ('grass-zoom', 'grass-top-down')
                              else 'deterministic_frame_clock'),
        'database_sha256': hashlib.sha256(args.world_db.read_bytes()).hexdigest(),
        'shader_sha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in sorted((assets / 'shaders').glob('*.wgsl'))},
        'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
    }
    (output / 'run.json').write_text(json.dumps(metadata, indent=2) + '\n')
    process = None
    try:
        with (output / 'run.log').open('w') as log:
            process = subprocess.Popen(command, cwd=output,
                env=dict(os.environ, MTL_HUD_ENABLED='1', MTL_HUD_LOG_ENABLED='1'),
                stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
            elapsed = 0
            while True:
                try:
                    code = process.wait(timeout=5)
                    break
                except subprocess.TimeoutExpired:
                    elapsed += 5
                    print(f'{output.name}: {elapsed}s', flush=True)
                    if elapsed >= 180:
                        raise RuntimeError('Game benchmark timed out')
            if code:
                raise RuntimeError(f'Game exited with {code}')
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    packets = []
    previous = None
    log = (output / 'run.log').read_text()
    if 'panicked at' in log or ' ERROR ' in log:
        raise RuntimeError('Rendering error; inspect run.log')
    for line in log.splitlines():
        if 'metal-HUD: ' not in line:
            continue
        payload = line.split('metal-HUD: ', 1)[1].strip()
        if payload == previous:
            continue
        previous = payload
        values = [float(v) for v in payload.split(',')]
        if len(values) < 5 or (len(values) - 3) % 2:
            raise ValueError('Unexpected Metal HUD packet')
        packets.append(values)
    samples = [v for packet in packets[5:] for v in packet[4::2]]
    if not samples:
        raise RuntimeError('No GPU timing samples')
    result = dict(scope=metadata['scope'], packets=len(packets), discarded_startup_packets=5,
                  samples=len(samples), gpu_mean_ms=statistics.mean(samples),
                  gpu_median_ms=statistics.median(samples),
                  gpu_p95_ms=sorted(samples)[int((len(samples)-1)*.95)])
    (output / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    main()
