#!/usr/bin/env python3
"""Offline comparison of real grass triangles and a single-depth-layer shadow trial.

Requires NumPy and Pillow for the report. The native exporter is an ignored Rust test,
so none of this experiment is included in the game/editor binary. No catalog is published.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT, TRIANGLES = 1280, 720, 584
MODES = {
    "baseline": "Existing blade lighting · no grass shadows",
    "reference": "Exact triangles · 2 m ray reference",
    "screen": "Depth-only approximation",
    "full-mask": "Reference sun visibility · 2 m",
    "short-mask": "Reference sun visibility · supported reach",
    "world-mask": "Reference sun visibility · full 35 cm",
    "reach": "Actual ray reach · white = 35 cm",
    "ao": "Authored root AO · unchanged",
    "screen-mask": "Depth-only sun visibility",
    "errors": "Disagreement with the short-range reference",
}


def load_frame(folder, stem):
    import numpy as np
    archive = folder / f"{stem}.npz"
    raw = folder / f"{stem}.bin"
    if raw.exists():
        data = raw.read_bytes()
        pixels = WIDTH * HEIGHT
        schema2 = len(data) == pixels * 48 + TRIANGLES * 48
        assert schema2 or len(data) == pixels * 44 + TRIANGLES * 48, f"Unexpected capture layout: {raw}"
        colors = np.frombuffer(data, np.uint8, count=pixels * 12).reshape(3, HEIGHT, WIDTH, 4).copy()
        metrics = np.frombuffer(data, '<f4', offset=pixels * 12, count=pixels * 4).reshape(HEIGHT, WIDTH, 4).copy()
        position = np.frombuffer(data, '<f4', offset=pixels * (32 if schema2 else 28), count=pixels * 4).reshape(HEIGHT, WIDTH, 4).copy()
        triangles = np.frombuffer(data, '<f4', offset=pixels * (48 if schema2 else 44)).reshape(TRIANGLES, 3, 4).copy()
        extra = {}
        if schema2:
            extra['detail'] = np.frombuffer(data, np.uint8, offset=pixels * 28, count=pixels * 4).reshape(HEIGHT, WIDTH, 4).copy()
        assert np.isfinite(metrics).all() and np.isfinite(position).all() and np.isfinite(triangles).all()
        np.savez_compressed(archive, colors=colors, metrics=metrics, position=position, triangles=triangles, **extra)
        raw.unlink()  # Replaced losslessly by the archive; report can be regenerated.
    with np.load(archive) as data:
        return {key: data[key] for key in data.files}


def metrics_for(data, selected, detail=None):
    import numpy as np
    full, short = (data[..., i] < 0.5 for i in range(2))
    confidence = 1.0 - data[..., 2]
    screen = confidence > 0.05
    world_limit = short if detail is None else detail[..., 2] < 128
    reach = None if detail is None else detail[..., 0].astype(float) / 255 * 35.0
    reads = None if detail is None else np.rint(detail[..., 1].astype(float) / 255 * 16)
    def count(mask):
        return int(np.count_nonzero(mask & selected))
    total = int(np.count_nonzero(selected))
    return dict(pixels=total, reference_shadow=count(full), short_reference_shadow=count(short),
                screen_shadow=count(screen), correct_shadow=count(short & screen),
                false_shadow=count(~short & screen), missed_shadow=count(short & ~screen),
                beyond_short_range=count(full & ~short), world_reference_shadow=count(world_limit),
                outside_budget=count(world_limit & ~short),
                weighted_false=float(confidence[selected & ~short].sum()),
                recovered_world_shadow=float(confidence[selected & world_limit].sum()),
                mean_reach_cm=float(reach[selected].mean()) if total and reach is not None else None,
                mean_reads=float(reads[selected].mean()) if total and reads is not None else None,
                max_reads=int(reads[selected].max()) if total and reads is not None else None)



def ray_hits(points, triangles, light, maximum=2.0):
    """Independent CPU check of selected GPU reference pixels; not used to shade images."""
    import numpy as np
    direction = np.array(light, dtype=np.float64)
    direction /= np.linalg.norm(direction)
    tri = triangles[..., :3].astype(np.float64)
    edge1, edge2 = tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0]
    p = np.cross(direction, edge2)
    det = np.einsum('ij,ij->i', edge1, p)
    inv = np.divide(1.0, det, out=np.zeros_like(det), where=np.abs(det) >= 1e-9)
    relative = points[:, None, :].astype(np.float64) + direction * .0015 - tri[None, :, 0]
    u = np.einsum('nmi,mi->nm', relative, p) * inv
    q = np.cross(relative, edge1)
    v = np.einsum('nmi,i->nm', q, direction) * inv
    distance = np.einsum('nmi,mi->nm', q, edge2) * inv
    return np.any((np.abs(det) >= 1e-9) & (u >= 0) & (v >= 0) & (u + v <= 1)
                  & (distance > .00001) & (distance + .0015 <= maximum), axis=1)


def build_report(folder, compare_folders=()):
    import numpy as np
    from PIL import Image
    rows = list(csv.DictReader((folder / "frames.csv").open()))
    metadata = json.loads((folder / "capture.json").read_text()) if (folder / "capture.json").exists() else {}
    frames, geometry_by_phase = [], {}
    cross_verified = False
    modes = dict(MODES)
    peers = []
    for index, peer in enumerate(compare_folders):
        peer = peer.resolve()
        info = json.loads((peer / 'comparison.json').read_text())
        params = info['metadata']
        assert params.get('strength', 1) == metadata.get('strength', 1), 'Comparison strengths differ'
        key = f'peer-{index}'
        label = f"{params.get('method', 'point')} · {params['steps']} reads"
        modes[key] = label
        modes[key + '-mask'] = label + ' · raw visibility'
        modes[key + '-errors'] = label + ' · errors within reach'
        peers.append((key, label, peer, {f['stem']: f for f in info['frames']}))
    for row in rows:
        stem = row['stem']
        frame = load_frame(folder, stem)
        m = frame['metrics']
        identity = m[..., 3].astype(int)
        present = identity > 0
        assert np.array_equal(identity, frame['position'][..., 3].astype(int)), 'Depth capture and final receivers disagree'
        assert np.isin(m[present, :2], [0, 1]).all(), 'References must be binary visibility'
        assert ((m[present, 2] >= 0) & (m[present, 2] <= 1)).all(), 'Invalid contact visibility'
        detail = frame.get('detail')
        if detail is not None:
            reads = np.rint(detail[..., 1].astype(float) / 255 * 16)
            assert reads[present].max() <= metadata['steps'], 'Exceeded depth-read budget'
        images = {mode: f'{stem}-{mode}.png' for mode in MODES}
        geom = frame['triangles'][..., :3]
        key = row['phase']
        if key in geometry_by_phase:
            assert np.max(np.abs(geom - geometry_by_phase[key])) < 1e-6, 'Physical fixture moved when camera/light changed'
        else:
            geometry_by_phase[key] = geom.copy()
        groups = {'all': present, 'grass': identity > 1, 'ground': identity == 1,
                  'isolated': identity == 2, 'crossing': (identity == 4) | (identity == 6), 'patch': identity >= 8}
        stats = {name: metrics_for(m, mask, detail) for name, mask in groups.items()}
        light = [-.35, .92, .18] if row['light'] == 'high' else [-.92, .30, .25]
        candidates = np.flatnonzero(present)
        selected = candidates[np.linspace(0, len(candidates) - 1, 256).astype(int)]
        points = frame['position'][..., :3].reshape(-1, 3)[selected]
        hit = ray_hits(points, frame['triangles'], light)
        expected = m[..., 0].reshape(-1)[selected] < .5
        assert np.count_nonzero(hit != expected) <= 1, 'GPU reference disagrees with independent CPU rays'
        if row['view'] == 'crossing' and row['light'] == 'high' and row['phase'] == '0':
            lower = frame['position'][identity == 4, :3]
            # Triangle ranges follow the actual template: 8 isolated, 8 lower, 8 upper.
            cast_by_upper = ray_hits(lower, frame['triangles'][16:24], light)
            assert np.count_nonzero(cast_by_upper) > 10, 'Crossing shadow must come from the OTHER blade'
            cross_verified = True
        # Equality of coverage is guaranteed by rendering the three colors in one fragment.
        for index, mode in enumerate(['baseline', 'reference', 'screen']):
            Image.fromarray(frame['colors'][index]).save(folder / f'{stem}-{mode}.png')
        for index, mode in enumerate(['full-mask', 'short-mask', 'screen-mask']):
            mask = np.full((HEIGHT, WIDTH, 3), 22, np.uint8)
            mask[present] = np.rint(35 + 200 * m[..., index, None][present]).astype(np.uint8)
            Image.fromarray(mask).save(folder / f'{stem}-{mode}.png')
        for mode, channel in [('world-mask', 2), ('reach', 0), ('ao', 3)]:
            if detail is not None:
                values = detail[..., channel]
            else:
                values = np.rint(m[..., 1] * 255).astype(np.uint8) if channel == 2 else np.full((HEIGHT, WIDTH), 255, np.uint8)
            display = np.where(present, values, 22).astype(np.uint8)
            Image.fromarray(display).save(folder / f'{stem}-{mode}.png')
        err = np.full((HEIGHT, WIDTH, 3), [22, 26, 29], np.uint8)
        short_shadow, screen_shadow = m[..., 1] < .5, m[..., 2] < .95
        err[present & short_shadow & screen_shadow] = [76, 133, 105]
        err[present & ~short_shadow & screen_shadow] = [244, 146, 57]
        err[present & short_shadow & ~screen_shadow] = [184, 102, 229]
        Image.fromarray(err).save(folder / f'{stem}-errors.png')
        peer_stats = {}
        for key, label, peer, peer_frames in peers:
            other = load_frame(peer, stem)
            assert np.array_equal(other['colors'][0], frame['colors'][0]), 'Baseline/camera/lighting changed across methods'
            assert np.array_equal(other['metrics'][..., 0], m[..., 0]), 'Triangle reference changed across methods'
            assert np.max(np.abs(other['triangles'] - frame['triangles'])) < 1e-6, 'Geometry changed across methods'
            for suffix, mode in [('', 'screen'), ('-mask', 'screen-mask'), ('-errors', 'errors')]:
                images[key + suffix] = os.path.relpath(peer / f'{stem}-{mode}.png', folder)
            peer_stats[key] = dict(label=label, stats=peer_frames[stem]['stats'])
        frames.append(dict(**row, stats=stats, images=images, peer_stats=peer_stats))
        print(f"{stem}: grass false/missed={stats['grass']['false_shadow']}/{stats['grass']['missed_shadow']}; isolated false={stats['isolated']['false_shadow']}")
    if len(geometry_by_phase) > 1:
        assert cross_verified, 'Missing crossing-blade reference check'
        initial = next(iter(geometry_by_phase.values()))
        assert max(float(np.max(np.abs(g - initial))) for g in geometry_by_phase.values()) > .001, 'Wind did not move the geometry'
    if len(rows) > 1:
        assert any(f['stats']['crossing']['reference_shadow'] > 10 for f in frames), 'Crossing fixture did not exercise partial shadows'
    assert any(f['stats']['patch']['reference_shadow'] > 100 for f in frames), 'Patch did not exercise occlusion'
    # Every color comparison uses the same restrained direct-light reduction.
    modes['screen'] = f"{metadata.get('method', 'point')} · {metadata['steps']} reads"
    payload = dict(frames=frames, modes=modes, metadata=metadata)
    (folder / 'comparison.json').write_text(json.dumps(payload, indent=2) + '\n')
    template = (ROOT / 'tools/grass_shadow_study.html').read_text()
    (folder / 'comparison.html').write_text(template.replace('/*STUDY_DATA*/', json.dumps(payload).replace('<', '\\u003c')))
    return folder / 'comparison.html'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--load', type=Path, default=ROOT / 'content/vegetation/distance-01.ron')
    parser.add_argument('--quick', action='store_true', help='One camera, light and frozen pose for shader/fixture checks')
    parser.add_argument('--report-only', action='store_true')
    parser.add_argument('--method', choices=['point', 'pixel'], default='pixel')
    parser.add_argument('--steps', type=int, choices=[8, 16], default=16)
    parser.add_argument('--strength', type=float, default=.45)
    parser.add_argument('--thickness', type=float)
    parser.add_argument('--compare', type=Path, action='append', default=[], help='Add a validated run with identical fixture inputs and strength')
    args = parser.parse_args()
    if args.thickness is None:
        args.thickness = .018 if args.method == 'point' else .004
    if not 0 <= args.strength <= 1 or not .0001 <= args.thickness <= .05:
        parser.error('Strength must be 0–1 and thickness must be 0.0001–0.05 m.')
    # Fail before an expensive capture if the report dependencies are missing.
    try:
        import numpy  # noqa: F401
        import PIL  # noqa: F401
    except ImportError:
        parser.error('Run with a Python environment containing numpy and Pillow.')
    folder = args.output.resolve()
    if not args.report_only:
        if folder.exists() and any(folder.iterdir()):
            parser.error('Use a new output directory, or --report-only for an existing capture.')
        folder.mkdir(parents=True, exist_ok=True)
        source = args.load.resolve(strict=True)
        shutil.copyfile(source, folder / 'source-study.ron')
        tracked = [ROOT / 'assets/shaders/vegetation_blade.wgsl', ROOT / 'assets/shaders/vegetation_debug_draw.wgsl',
                   ROOT / 'crates/vegetation_render/src/renderer/shadow_study.rs', ROOT / 'crates/vegetation_render/src/renderer/shadow_study/fixture.wgsl', ROOT / 'crates/vegetation_render/src/renderer/shadow_study/pixel_trace.wgsl', source]
        metadata = dict(source=str(source), sha256={str(p.relative_to(ROOT)) if p.is_relative_to(ROOT) else str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in tracked},
                        schema=2, method=args.method, steps=args.steps, strength=args.strength, ray_length_m=.35, depth_thickness_m=args.thickness, bias_m=.0015, reference_length_m=2, plane_rejection_m=.0015, distance_fade_start=.60,
                        blades=83, triangles=TRIANGLES, resolution=[WIDTH, HEIGHT], msaa=1, opening=False,
                        display='Linear shading to sRGB, without the game tonemapper. Same material functions and exposure in all modes.',
                        performance='Offline experiment only. No production performance measurement.')
        (folder / 'capture.json').write_text(json.dumps(metadata, indent=2) + '\n')
        env = dict(os.environ, YARRA_SHADOW_STUDY_OUTPUT=str(folder), YARRA_SHADOW_STUDY_SOURCE=str(folder / 'source-study.ron'), YARRA_SHADOW_STUDY_METHOD=args.method, YARRA_SHADOW_STUDY_STEPS=str(args.steps), YARRA_SHADOW_STUDY_STRENGTH=str(args.strength), YARRA_SHADOW_STUDY_THICKNESS=str(args.thickness))
        if args.quick:
            env['YARRA_SHADOW_STUDY_QUICK'] = '1'
        else:
            env.pop('YARRA_SHADOW_STUDY_QUICK', None)
        command = ['cargo', 'test', '--offline', '-p', 'yarra-vegetation-render', 'capture_shadow_study', '--', '--ignored', '--nocapture', '--test-threads=1']
        subprocess.run(command, cwd=ROOT, env=env, check=True)
    print(build_report(folder, args.compare))


if __name__ == '__main__':
    main()
