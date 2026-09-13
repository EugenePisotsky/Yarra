#!/usr/bin/env python3
"""Measure the centerline approximation of the fixed-budget paired ribbon experiment.

Uses the saved authored curves, including interpolation of packed handles and the companion's
existing 75% curvature blend. This is a normalized rest-curve measurement, not a frame-time,
coverage, opened-edge, wind or screen-space quality metric. Requires numpy; optional plotting requires matplotlib.
"""
import argparse
import json
from pathlib import Path
import re
import numpy as np

SPECIES = ['short_split_fill_ribbon', 'study_middle_arch', 'study_high_arch']


def curves_from_study(path):
    source = path.read_text()
    result = []
    labels = []
    for name in SPECIES:
        start = source.index(f'key: "{name}"')
        body = source[start:source.index('material:', start)]
        power = float(re.search(r'longitudinal_power: ([\d.e+-]+)', body)[1])
        sections = int(re.search(r'high_section_count: (\d+)', body)[1])
        pair = int(re.search(r'blades_per_render_unit: (\d+)', body)[1])
        if abs(power - .92) > 1e-6 or sections < 5 or pair != 2:
            raise ValueError(f'{name}: this experiment expects paired ribbons with at least five authored sections and power 0.92')
        variants = []
        for variant in ['a', 'b']:
            block = body.split('curve_variant_' + variant + ': (')[1].split(')')[0]
            values = {key: float(value) for key, value in re.findall(r'(\w+): ([\d.e+-]+)', block)}
            root, tip = values['root_tangent_radians'], values['tip_tangent_radians']
            variants.append((values['tip_tilt_radians'],
                np.array([np.sin(root), np.cos(root)]) * values['root_handle_length'],
                np.array([np.sin(tip), np.cos(tip)]) * values['tip_handle_length']))
        for alpha in np.linspace(0, 1, 9):
            tilt = variants[0][0] * (1-alpha) + variants[1][0] * alpha
            p3 = np.array([np.sin(tilt), np.cos(tilt)])
            p1 = variants[0][1] * (1-alpha) + variants[1][1] * alpha
            p2 = p3 - (variants[0][2] * (1-alpha) + variants[1][2] * alpha)
            result.append([np.zeros(2), p1, p2, p3])
            labels.append(f'{name} / variant {alpha:.3f}')
    return np.asarray(result), labels


def evaluate(curves, t):
    t = np.asarray(t)[:, None]
    u = 1-t
    return curves[:, 0, None]*u**3 + 3*curves[:, 1, None]*u*u*t + 3*curves[:, 2, None]*u*t*t + curves[:, 3, None]*t**3


def error(curves, samples):
    ts = np.linspace(0, 1, 2001)
    reference = evaluate(curves, ts)
    polygon = evaluate(curves, samples)
    index = np.minimum(np.searchsorted(samples, ts, side='right')-1, len(samples)-2)
    a, b = polygon[:, index], polygon[:, index+1]
    segment, offset = b-a, reference-a
    weight = np.clip(np.sum(offset*segment, axis=2) / np.maximum(np.sum(segment*segment, axis=2), 1e-15), 0, 1)
    return np.linalg.norm(offset-segment*weight[:, :, None], axis=2).max(axis=1)


def main_morph_angles(curves, samples):
    """Reproduce the main centerline morph; angles are in rest space, not screen space."""
    full = evaluate(curves, samples)
    shoulder_t = .5**.92
    shoulder = evaluate(curves, [shoulder_t])[:, 0]
    low = np.empty_like(full)
    for row, t in enumerate(samples):
        if t <= shoulder_t:
            low[:, row] = shoulder * (t / shoulder_t)
        else:
            low[:, row] = shoulder + (curves[:, 3] - shoulder) * ((t - shoulder_t) / (1 - shoulder_t))
    result = {}
    for morph in [1, .75, .5, .25, 0]:
        positions = low * (1 - morph) + full * morph
        directions = np.diff(positions, axis=1)
        directions /= np.linalg.norm(directions, axis=2)[:, :, None]
        turns = np.degrees(np.arccos(np.clip(np.sum(directions[:, :-1] * directions[:, 1:], axis=2), -1, 1)))
        per_curve = turns.max(axis=1)
        result[str(morph)] = dict(median_maximum_turn_degrees=float(np.median(per_curve)),
                                 maximum_turn_degrees=float(per_curve.max()),
                                 minimum_interior_angle_degrees=float(180 - per_curve.max()))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--study', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--plot', action='store_true', help='Also write a centerline SVG using matplotlib')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    curves, labels = curves_from_study(args.study)
    companion = curves.copy()
    companion[:, 1] = curves[:, 3]/3*.25 + curves[:, 1]*.75
    companion[:, 2] = curves[:, 3]*2/3*.25 + curves[:, 2]*.75
    distributions = {
        'main_before': np.array([0, 1/6, 1/3, .5, .75, 1])**.92,
        'main_after': np.array([0, .128, .292, .5, .768, 1])**.92,
        'companion_before': np.linspace(0, 1, 4)**.92,
        'companion_after': np.array([0, .183, .423, .723, 1])**.92,
    }
    metrics = {'scope': 'Maximum distance to the corresponding polygon segment in normalized rest-curve space; each blade normalized to its own height.', 'study': str(args.study.resolve()), 'longitudinal_power': .92, 'curves': len(curves)}
    for key, samples in distributions.items():
        values = error(companion if key.startswith('companion') else curves, samples)
        metrics[key] = dict(parameters=samples.tolist(), median=float(np.median(values)), maximum=float(max(values)), errors=dict(zip(labels, values.tolist())))
    for kind in ['main', 'companion']:
        old = metrics[kind+'_before']; new = metrics[kind+'_after']
        print(f'{kind}: median {old["median"]:.6f} -> {new["median"]:.6f}; maximum {old["maximum"]:.6f} -> {new["maximum"]:.6f}')
    metrics['main_morph_corner_angles'] = {state: main_morph_angles(curves, distributions['main_'+state])
                                          for state in ['before', 'after']}
    print('Main maximum rest-space direction change (before -> after):')
    for morph in ['1', '0.75', '0.5', '0.25', '0']:
        before = metrics['main_morph_corner_angles']['before'][morph]['maximum_turn_degrees']
        after = metrics['main_morph_corner_angles']['after'][morph]['maximum_turn_degrees']
        print(f'  morph {morph}: {before:.2f} -> {after:.2f} degrees')
    (args.output/'curve-errors.json').write_text(json.dumps(metrics, indent=2)+'\n')
    if not args.plot:
        return
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(1, 2, figsize=(12, 4.3), constrained_layout=True)
    index = max(range(len(labels)), key=lambda i: metrics['companion_before']['errors'][labels[i]])
    for ax, kind, data in zip(axes, ['main', 'companion'], [curves, companion]):
        exact = evaluate(data[index:index+1], np.linspace(0, 1, 1001))[0]
        ax.plot(exact[:, 0], exact[:, 1], color='#111111', lw=1.5, label='Intended cubic')
        for state, color in [('before', '#ba4534'), ('after', '#267c9c')]:
            points = evaluate(data[index:index+1], distributions[kind+'_'+state])[0]
            ax.plot(points[:, 0], points[:, 1], 'o-', color=color, lw=1, ms=4, label=state.title())
        ax.set_title(kind.title()+' blade')
        ax.set_aspect('equal');ax.set_xlabel('Forward / blade height');ax.set_ylabel('Up / blade height')
        ax.grid(alpha=.2);ax.legend(loc='upper right', fontsize=8)
    fig.suptitle('Same 18-vertex pair budget · rest centerlines only\n'+labels[index], fontsize=11)
    fig.savefig(args.output/'curve-sampling.svg')


if __name__ == '__main__':
    main()
