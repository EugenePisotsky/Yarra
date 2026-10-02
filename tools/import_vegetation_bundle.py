#!/usr/bin/env python3
"""Adapt built vegetation bundles to Yarra's existing tree material/LOD contract.

Uses Python's standard library and Khronos ktx. Example:
  python3 tools/import_vegetation_bundle.py --bundle /path/to/birch_leafy/current \
    --bundle /path/to/birch_sparse/current --bundle /path/to/birch_bare/current

The source bundles are never modified. This imports three mesh LODs only: the
authoring billboard's view selection and elevated coverage are not implemented
in Yarra yet. Crown shading is an adjustable experiment, not a final art rule.
"""
import argparse
import hashlib
import json
import math
import shutil
import struct
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read_glb(path):
    data = path.read_bytes()
    magic, version, length = struct.unpack_from('<III', data)
    if (magic, version, length) != (0x46546C67, 2, len(data)):
        raise ValueError(f'Invalid GLB: {path}')
    chunks = {}
    offset = 12
    while offset < length:
        size, kind = struct.unpack_from('<II', data, offset)
        chunks[kind] = data[offset + 8:offset + 8 + size]
        offset += 8 + size
    return json.loads(chunks[0x4E4F534A]), bytearray(chunks[0x004E4942])


def rows(doc, blob, index):
    a = doc['accessors'][index]
    v = doc['bufferViews'][a['bufferView']]
    count = {'SCALAR': 1, 'VEC2': 2, 'VEC3': 3, 'VEC4': 4}[a['type']]
    fmt = '<' + {5123: 'H', 5125: 'I', 5126: 'f'}[a['componentType']] * count
    stride = v.get('byteStride', struct.calcsize(fmt))
    start = v.get('byteOffset', 0) + a.get('byteOffset', 0)
    return [struct.unpack_from(fmt, blob, start + i * stride) for i in range(a['count'])]


def overwrite(doc, blob, index, values):
    a = doc['accessors'][index]
    assert a['componentType'] == 5126 and a['count'] == len(values)
    v = doc['bufferViews'][a['bufferView']]
    fmt = '<' + 'f' * len(values[0])
    start = v.get('byteOffset', 0) + a.get('byteOffset', 0)
    stride = v.get('byteStride', struct.calcsize(fmt))
    for i, value in enumerate(values):
        assert all(math.isfinite(x) for x in value)
        struct.pack_into(fmt, blob, start + i * stride, *value)


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def cross(a, b):
    return (a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0])


def unit(a):
    length = math.sqrt(dot(a, a))
    return tuple(x / max(length, 1e-12) for x in a)


def adapt_foliage(doc, blob, blend):
    moving_ids = set()
    for mesh in doc['meshes']:
        for primitive in mesh['primitives']:
            material = doc['materials'][primitive['material']]
            a = primitive['attributes']
            if material.get('alphaMode') == 'MASK':
                material['extras'] = {'yarra_wind': 'foliage_uv1_v1', 'yarra_shading': 'crown_v1'}
                material['extensions'] = {'KHR_materials_specular': {'specularFactor': .5}}
                material['normalTexture']['scale'] = .6
                n = rows(doc, blob, a['NORMAL'])
                canopy = rows(doc, blob, a['_CANOPY_NORMAL'])
                n = [unit(tuple(x*(1-blend)+y*blend for x, y in zip(v, c))) for v, c in zip(n, canopy)]
                overwrite(doc, blob, a['NORMAL'], n)
                # Rebuild tangents against the adapted normals and unchanged UVs.
                p = rows(doc, blob, a['POSITION'])
                uv = rows(doc, blob, a['TEXCOORD_0'])
                ix = [r[0] for r in rows(doc, blob, primitive['indices'])]
                tangents, bitangents = [[0., 0., 0.] for _ in p], [[0., 0., 0.] for _ in p]
                for t in range(0, len(ix), 3):
                    i, j, k = ix[t:t+3]
                    e1, e2 = [p[j][c]-p[i][c] for c in range(3)], [p[k][c]-p[i][c] for c in range(3)]
                    u1, u2 = [uv[j][c]-uv[i][c] for c in range(2)], [uv[k][c]-uv[i][c] for c in range(2)]
                    det = u1[0]*u2[1]-u1[1]*u2[0]
                    if abs(det) < 1e-12:
                        continue
                    for vertex in (i, j, k):
                        for c in range(3):
                            tangents[vertex][c] += (e1[c]*u2[1]-e2[c]*u1[1])/det
                            bitangents[vertex][c] += (e2[c]*u1[0]-e1[c]*u2[0])/det
                result = []
                for normal, tangent, bitangent in zip(n, tangents, bitangents):
                    tangent = tuple(tangent[c]-normal[c]*dot(tangent, normal) for c in range(3))
                    if dot(tangent, tangent) < 1e-12:
                        helper = min(((1,0,0),(0,1,0),(0,0,1)), key=lambda v: abs(dot(v, normal)))
                        tangent = cross(normal, helper)
                    tangent = unit(tangent)
                    result.append((*tangent, -1. if dot(cross(normal, tangent), bitangent) < 0 else 1.))
                overwrite(doc, blob, a['TANGENT'], result)
                meta = rows(doc, blob, a['_CARD_META'])
                axes = rows(doc, blob, a['_CARD_AXIS'])
                axes = [axis if m[0] > .5 else (0.,0.,0.) for axis, m in zip(axes, meta)]
                overwrite(doc, blob, a['_CARD_AXIS'], axes)
                moving_ids.update(int(m[1]) for m in meta if m[0] > .5)
            # These authoring attributes are retained in the original GLB. The
            # game registers the card transform attributes below.
            for key in list(a):
                if key.startswith('_') and key not in ('_CARD_AXIS', '_CARD_PIVOT', '_CARD_NORMAL', '_CARD_FACING'):
                    del a[key]
    doc['extensionsUsed'] = sorted(set(doc.get('extensionsUsed', [])) | {'KHR_materials_specular'})
    return moving_ids


def texture(ktx, files, output, srgb, normal):
    cmd = [str(ktx), 'create', '--format', 'R8G8B8A8_SRGB' if srgb else 'R8G8B8A8_UNORM',
           '--assign-tf', 'srgb' if srgb else 'linear', '--encode', 'uastc',
           '--uastc-quality', '2', '--zstd', '8', '--threads', '4']
    cmd += ['--levels', str(len(files))] if len(files) > 1 else ['--generate-mipmap']
    if normal:
        cmd += ['--normalize']  # Keep RGB normal encoding expected by Bevy.
    subprocess.run(cmd + [str(f) for f in files] + [str(output)], check=True)
    subprocess.run([str(ktx), 'validate', str(output)], check=True, stdout=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', type=Path, action='append', required=True)
    parser.add_argument('--output', type=Path, default=ROOT/'assets/local/yarra_birches')
    parser.add_argument('--catalog', type=Path, default=ROOT/'assets/packs/yarra_birches/birches.catalog.ron')
    parser.add_argument('--ktx', type=Path, default=Path(shutil.which('ktx') or ROOT.parent/'yarra/.tools/ktx/4.4.2/bin/ktx'),
                        help='Khronos ktx executable (PATH, then the legacy sibling tool cache)')
    parser.add_argument('--canopy-blend', type=float, default=.85)
    args = parser.parse_args()
    if not args.ktx.is_file():
        parser.error('Khronos ktx was not found; install it on PATH or pass --ktx /path/to/ktx')
    if not 0 <= args.canopy_blend <= 1:
        parser.error('--canopy-blend must be 0..1')
    output = args.output.resolve()
    pack = output.relative_to(ROOT/'assets').as_posix()
    output.parent.mkdir(parents=True, exist_ok=True)
    entries, reports, texture_hashes = [], [], {}
    with tempfile.TemporaryDirectory(prefix='.birch-import-', dir=output.parent) as tmp:
        stage = Path(tmp)/'pack'
        texdir, source = stage/'runtime/textures', stage/'source'
        texdir.mkdir(parents=True); (source/'textures').mkdir(parents=True)
        for bundle in args.bundle:
            manifest = json.loads((bundle/'manifest.json').read_text())
            if manifest['runtime']['version'] != 'vegetation_v1':
                raise ValueError('Unsupported vegetation contract')
            name = manifest['asset']
            if not name.replace('_', '').replace('-', '').isalnum():
                raise ValueError('Invalid asset name')
            levels = json.loads((bundle/'texture_mips.json').read_text())['foliage_basecolor']
            variants, counts, first_ids = [], [], None
            bounds = [max(max(abs(v['min'][c]), abs(v['max'][c])) * (1 if c == 1 else 2)
                          for v in manifest['bounds'].values()) for c in range(3)]
            for lod, threshold in enumerate((480., 180., 0.)):
                glb = bundle/f'lod{lod}.glb'
                doc, blob = read_glb(glb)
                for im in doc['images']:
                    uri = im['uri']
                    files = [bundle/uri] + ([bundle/p for p in levels] if uri.endswith('foliage_basecolor.png') else [])
                    filename = Path(uri).with_suffix('.ktx2').name
                    digest = hashlib.sha256(b''.join(p.read_bytes() for p in files)).hexdigest()
                    if filename in texture_hashes and texture_hashes[filename] != digest:
                        raise ValueError(f'Bundles must share texture content: {filename}')
                    if filename not in texture_hashes:
                        print('Compressing', filename, flush=True)
                        texture(args.ktx, files, texdir/filename, 'basecolor' in uri, 'normal' in uri)
                        texture_hashes[filename] = digest
                        shutil.copy2(bundle/uri, source/'textures'/Path(uri).name)
                    im['uri'] = '../textures/'+filename
                ids = adapt_foliage(doc, blob, args.canopy_blend)
                if first_ids is not None and ids != first_ids:
                    raise ValueError('Rotating-card IDs change between LODs')
                first_ids = ids
                folder = stage/'runtime'/name
                folder.mkdir(parents=True, exist_ok=True)
                target = folder/f'{name}_lod{lod}.gltf'
                doc['buffers'] = [{'uri': target.with_suffix('.bin').name, 'byteLength': len(blob)}]
                doc['asset']['generator'] = 'Yarra import_vegetation_bundle.py'
                doc['extras'] = {'source_bundle': name, 'experimental_canopy_blend': args.canopy_blend}
                target.write_text(json.dumps(doc, indent=1)+'\n')
                target.with_suffix('.bin').write_bytes(blob)
                count = sum(doc['accessors'][p['indices']]['count']//3 for mesh in doc['meshes'] for p in mesh['primitives'])
                assert count == manifest['triangles'][f'lod{lod}']
                counts.append(count)
                variants.append(f'(uri: "{pack}/runtime/{name}/{target.name}", bounds: ({bounds[0]:.4f}, {bounds[1]:.4f}, {bounds[2]:.4f}), gpu_bytes_estimate: {len(blob)}, minimum_screen_height: {threshold}),')
            shutil.copy2(bundle/'lod0.glb', source/f'{name}.glb')
            for filename in ('manifest.json', 'settings.json', 'branch_library.json'):
                shutil.copy2(bundle/filename, source/f'{name}_{filename}')
            entries.append(f'(key: "{output.name}/{name}", display_name: "{name.replace("_", " ").title()}", source_uri: "{pack}/source/{name}.glb", variants: [\n'+ '\n'.join(variants)+'\n]),')
            reports.append({'asset': name, 'triangles': counts, 'facing_cards': len(first_ids), 'source': str(bundle.resolve())})
        (stage/'import.json').write_text(json.dumps({'assets': reports, 'source_texture_sha256': texture_hashes,
            'canopy_blend': args.canopy_blend, 'lighting': 'Existing experimental crown_v1 shader; no global lighting changes',
            'billboard': 'Not registered: view-selection shader pending',
            'translucency': 'Source sidecar not sampled by current Yarra foliage shader'}, indent=2)+'\n')
        if output.exists():
            output.rename(output.with_name(output.name+'.previous-'+str(time.time_ns())))
        stage.rename(output)
    args.catalog.parent.mkdir(parents=True, exist_ok=True)
    args.catalog.write_text('(schema_version: 1, assets: [\n'+'\n'.join(entries)+'\n])\n')
    print(json.dumps(reports, indent=2))
    print('Catalog:', args.catalog)


if __name__ == '__main__':
    main()
