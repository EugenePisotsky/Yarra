#!/usr/bin/env python3
"""Offline GP-022 counter comparison; no game, GPU work or sudo."""
import hashlib
import json
from pathlib import Path
import tempfile
import zipfile
from summarize_gpu_counters import summarize

HERE = Path(__file__).resolve().parent
MAIN = 'main_opaque_pass_3d_resolve_only'
FIELDS = ['Vertices', 'VS Invocations', 'VS ALU Instructions', 'FS Invocations',
          'FS ALU Instructions', 'Bytes Read From Device Memory',
          'Bytes Written To Device Memory', 'Tiled Vertex Buffer Bytes',
          'VS Occupancy', 'VS ALU Limiter', 'FS Occupancy', 'FS ALU Limiter']


def analyze():
    with zipfile.ZipFile(HERE / 'evidence.zip') as archive, tempfile.TemporaryDirectory() as temp:
        inputs = {v: json.loads(archive.read(f'{v}/inputs.json'))
                  for v in ('baseline', 'candidate')}
        a, b = inputs['baseline'], inputs['candidate']
        for key in ('binary_sha256', 'database_sha256', 'canopy_sha256'):
            assert a[key] == b[key], key
        changed = sorted(k for k in a['shader_sha256']
                         if a['shader_sha256'][k] != b['shader_sha256'][k])
        assert changed == ['vegetation_debug_draw.wgsl'], changed
        observations = json.loads(archive.read('replay-observations.json'))
        rows = []
        source_hashes = {}
        for visit in observations['order']:
            name = visit['csv']
            raw = archive.read(name)
            source_hashes[name] = hashlib.sha256(raw).hexdigest()
            path = Path(temp) / 'counters.csv'
            path.write_bytes(raw)
            counters = summarize(path)
            main = next(r for r in counters['encoders'] if r['Encoder Label'] == MAIN)
            rows.append({**visit, 'summed_encoder_ms': counters['summed_encoder_ms'],
                         'main_ms': float(main['GPU Time']) / 1e6,
                         'main_counters': {k: main[k] for k in FIELDS}})
        for key in ('Vertices', 'VS Invocations'):
            assert len({r['main_counters'][key] for r in rows}) == 1, key
        def mean_instructions(variant):
            matching = [r for r in rows if r['variant'] == variant]
            return sum(int(r['main_counters']['VS ALU Instructions']) for r in matching) / len(matching)
        baseline = mean_instructions('baseline')
        candidate = mean_instructions('candidate')
        return {
            'experiment': 'GP-022', 'changed_shaders': changed,
            'inputs': inputs, 'source_sha256': source_hashes, 'replays': rows,
            'main_vs_alu_instruction_change_percent': 100 * (candidate / baseline - 1),
            'limitations': [
                'Two profiles per captured frame, order A-B-B-A, all Medium. Not four independent game runs.',
                'Summed encoder work is replay timing, not live frame latency or power.',
                'Main encoder includes grass and terrain; this is not a grass-only percentage.',
                'No sustained speedup, energy or thermal improvement established.',
                'Separate captures have nondeterministic compaction/preparation selection; frozen shader A/B is the image-equivalence evidence.',
            ],
        }


if __name__ == '__main__':
    print(json.dumps(analyze(), indent=2, sort_keys=True))
