"""Lay every 1 m detail patch (Houdini exports/detail/patches.json) over a 1 m import: clone the
base heights and masks, write each patch's rows, and where two patches overlap blend them by
distance to each one's border (both already hand over to the base there). Writes a manifest.
Usage: merge_patches.py BASE_MANIFEST PATCH_LIST OUT_DIR"""
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

base_manifest, patch_list, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
base = json.loads(base_manifest.read_text())
listing = json.loads(patch_list.read_text())
band = listing['edge_blend_m']
patches = []
for entry in listing['patches']:
    manifest_path = patch_list.parent / entry['manifest']
    m = json.loads(manifest_path.read_text())
    assert m['spacing'] == 1.0
    px, pz = m['samples']
    c0, r0 = (int(m['origin'][i] - base['origin'][i]) for i in range(2))
    patches.append({'name': entry['name'], 'dir': manifest_path.parent, 'manifest': m, 'rows': (r0, r0 + pz), 'cols': (c0, c0 + px)})
assert base['spacing'] == 1.0
bx, bz = base['samples']
out.mkdir(parents=True, exist_ok=True)

def ramp(p, rows, cols):
    """Hand-over weight of patch `p` on base rows/cols: 0 at its border, 1 past the band."""
    i = np.arange(*rows) - p['rows'][0]; j = np.arange(*cols) - p['cols'][0]
    pz = p['rows'][1] - p['rows'][0]; px = p['cols'][1] - p['cols'][0]
    d = np.minimum(np.minimum(i, pz - 1 - i)[:, None], np.minimum(j, px - 1 - j)[None, :]).astype('f4')
    t = np.clip(d / band, 0, 1)
    return t * t * (3 - 2 * t) + 1e-4

def overlap(a, b):
    rows = (max(a['rows'][0], b['rows'][0]), min(a['rows'][1], b['rows'][1]))
    cols = (max(a['cols'][0], b['cols'][0]), min(a['cols'][1], b['cols'][1]))
    return (rows, cols) if rows[0] < rows[1] and cols[0] < cols[1] else None

def lay(src, dst, file_of, dtype):
    subprocess.run(['cp', '-c', str(src), str(dst)], check=True)   # APFS clone; only patched blocks are written
    a = np.memmap(dst, dtype=dtype, mode='r+', shape=(bz, bx))
    data = {}
    for p in patches:
        (r0, r1), (c0, c1) = p['rows'], p['cols']
        data[p['name']] = np.fromfile(p['dir'] / file_of(p), dtype).reshape(r1 - r0, c1 - c0)
        a[r0:r1, c0:c1] = data[p['name']]
    for i, p in enumerate(patches):
        for q in patches[i + 1:]:
            o = overlap(p, q)
            if not o:
                continue
            (r0, r1), (c0, c1) = o
            wp, wq = ramp(p, *o), ramp(q, *o)
            vp = data[p['name']][r0 - p['rows'][0]:r1 - p['rows'][0], c0 - p['cols'][0]:c1 - p['cols'][0]].astype('f4')
            vq = data[q['name']][r0 - q['rows'][0]:r1 - q['rows'][0], c0 - q['cols'][0]:c1 - q['cols'][0]].astype('f4')
            v = (wp * vp + wq * vq) / (wp + wq)
            a[r0:r1, c0:c1] = v if dtype == '<f4' else np.rint(v).astype(dtype)
    a.flush()

resolve = lambda f: Path(f) if Path(f).is_absolute() else (base_manifest.parent / f).resolve()
lay(resolve(base['heights']), out / 'height.f32', lambda p: p['manifest']['heights'], '<f4')
patched = {m['name'] for m in patches[0]['manifest']['masks']}
assert all({m['name'] for m in p['manifest']['masks']} == patched for p in patches)
masks = []
for m in base['masks']:
    src = resolve(m['file'])
    if m['name'] in patched:
        file_of = lambda p, n=m['name']: next(e['file'] for e in p['manifest']['masks'] if e['name'] == n)
        lay(src, out / f"{m['name']}.u8", file_of, 'u1')
        masks.append({'name': m['name'], 'file': str((out / f"{m['name']}.u8").resolve())})
    else:
        masks.append({'name': m['name'], 'file': str(src)})
manifest = dict(base, heights=str((out / 'height.f32').resolve()), masks=masks)
manifest['source'] = f"{base.get('source', '')} | 1 m detail patches {', '.join(p['name'] for p in patches)}"
(out / 'heightfield.json').write_text(json.dumps(manifest, indent=2) + '\n')
for p in patches:
    print(f"patched {p['name']}: rows {p['rows']}, columns {p['cols']}")
print('overlaps', [(p['name'], q['name']) for i, p in enumerate(patches) for q in patches[i + 1:] if overlap(p, q)])
