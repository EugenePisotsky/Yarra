"""Minute buckets for the terrain walking test; accepts a grass_profile suite directory."""
import argparse
import json
from pathlib import Path
import statistics
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tools'))
from grass_profile_report import fields, quantile, timestamp


def summarize(directory):
    metadata = json.loads((directory / 'run.json').read_text())
    lines = (directory / 'game.log').read_text().splitlines()
    events = [fields(line) for line in lines if 'GRASS_PROFILE ' in line]
    start = next((e for e in events if e.get('event') == 'measure_start'), None)
    if start is None:
        return {'state': 'warmup'}
    finish = next((e for e in events if e.get('event') == 'complete'), None)
    begin = int(start['unix_ms']) / 1000
    end = int(finish['unix_ms']) / 1000 if finish else float('inf')
    buckets = {}

    def bucket(seconds):
        index = min(max(0, int((seconds - 0.001) // 60)),
                    max(0, int((metadata['settings']['seconds'] - 0.001) // 60)))
        return buckets.setdefault(index, {'frames': 0, 'seconds': 0., 'late': 0,
                                         'max_interval_ms': 0., 'window_fps': [],
                                         'gpu': [], 'metal_mb': [], 'app_mb': [],
                                         'thermal': set(), 'source_pages': set(),
                                         'meshes': set(), 'images': set(), 'focused': True})

    for event in events:
        if event.get('event') != 'sample':
            continue
        b = bucket(float(event['route_s']))
        frames, seconds = int(event['frames']), float(event['window_s'])
        b['frames'] += frames
        b['seconds'] += seconds
        b['late'] += int(event['late_updates'])
        b['max_interval_ms'] = max(b['max_interval_ms'], float(event['update_max_ms']))
        b['focused'] &= event['focused'] == 'true'
        if seconds >= 0.9:
            b['window_fps'].append(frames / seconds)
    previous_payload = previous_stamp = None
    for line in lines:
        if 'RENDER_AUDIT ' in line:
            event = fields(line)
            stamp = int(event['unix_ms']) / 1000
            if begin <= stamp <= end:
                b = bucket(stamp - begin)
                b['thermal'].add(event['thermal'])
                b['source_pages'].add(int(event['source_pages']))
                b['meshes'].add(int(event['mesh_assets']))
                b['images'].add(int(event['image_assets']))
        if 'metal-HUD: ' not in line:
            continue
        stamp = timestamp(line, metadata['local_utc_offset_seconds'])
        payload = line.split('metal-HUD: ', 1)[1].strip()
        if stamp is None or payload == previous_payload:
            continue
        previous_payload = payload
        inside = previous_stamp is not None and previous_stamp >= begin and stamp <= end
        previous_stamp = stamp
        if not inside:
            continue
        values = list(map(float, payload.split(',')))
        if len(values) >= 5 and (len(values) - 3) % 2 == 0:
            b = bucket(stamp - begin)
            b['gpu'].extend(values[4::2])
            b['metal_mb'].append(values[1])
            b['app_mb'].append(values[2])
    result = []
    for index, b in sorted(buckets.items()):
        if not b['seconds']:
            continue
        result.append({
            'minute': index + 1, 'seconds': b['seconds'],
            'fps': b['frames'] / b['seconds'], 'late_updates': b['late'],
            'max_interval_ms': b['max_interval_ms'],
            'worst_window_fps': min(b['window_fps'], default=None),
            'gpu_mean_ms': statistics.mean(b['gpu']) if b['gpu'] else None,
            'gpu_p95_ms': quantile(b['gpu'], .95),
            'metal_mb': statistics.mean(b['metal_mb']) if b['metal_mb'] else None,
            'app_mb': statistics.mean(b['app_mb']) if b['app_mb'] else None,
            **{key: sorted(b[key]) for key in ['thermal', 'source_pages', 'meshes', 'images']},
            'focused': b['focused'],
        })
    return {'state': 'complete' if finish else 'running', 'minutes': result}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('suite', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = {p.name: summarize(p) for p in sorted((args.suite / 'runs').iterdir())
              if (p / 'game.log').exists()}
    text = json.dumps(result, indent=2) + '\n'
    if args.output:
        args.output.write_text(text)
    else:
        print(text)
