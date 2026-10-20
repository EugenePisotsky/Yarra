#!/usr/bin/env python3
"""Publish an approved Reteya bundle on Yarra's exact 1 m grid.

Resamples the cooked heightfield without recooking Houdini erosion. NumPy only.
Usage: python export_game.py SOURCE_BUNDLE DESTINATION --start X Z
"""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def resample(source, coordinate_x, coordinate_z, output, cubic=False):
    def taps(coordinate, size):
        coordinate = np.clip(coordinate, 0, size - 1).astype(np.float32)
        base = np.floor(coordinate).astype(np.int32)
        t = coordinate - base
        if cubic:
            t2, t3 = t*t, t*t*t
            weights = [.5*(-t3+2*t2-t), .5*(3*t3-5*t2+2),
                       .5*(-3*t3+4*t2+t), .5*(t3-t2)]
            offsets = [-1,0,1,2]
        else:
            weights, offsets = [1-t,t], [0,1]
        return [(np.clip(base+offset,0,size-1), weight.astype(np.float32))
                for offset,weight in zip(offsets,weights)]
    xt, zt = taps(coordinate_x,source.shape[1]), taps(coordinate_z,source.shape[0])
    horizontal = np.zeros((source.shape[0],len(coordinate_x)),dtype=np.float32)
    for index,weight in xt:
        horizontal += source[:,index].astype(np.float32)*weight
    for first in range(0,len(coordinate_z),128):
        last=min(first+128,len(coordinate_z))
        values=np.zeros((last-first,len(coordinate_x)),dtype=np.float32)
        for index,weight in zt:
            values += horizontal[index[first:last]]*weight[first:last,None]
        if cubic:
            output[first:last]=np.clip(values,source.min(),source.max())
        else:
            output[first:last]=np.clip(np.rint(values),0,255).astype(np.uint8)
    output.flush()


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('source',type=Path);ap.add_argument('destination',type=Path)
    ap.add_argument('--start',type=float,nargs=2,required=True)
    a=ap.parse_args();src=a.source;dst=a.destination
    if dst.exists():
        raise SystemExit('Use a new destination; existing exports are not overwritten.')
    m=json.loads((src/'heightfield.json').read_text())
    nx,nz=m['samples'];step=m['spacing'];origin=np.array(m['origin'])
    lower=origin-step*.5;upper=origin+step*(np.array([nx,nz])-.5)
    if not np.allclose(np.rint(lower),lower) or not np.allclose(np.rint(upper),upper):
        raise SystemExit('Source canvas bounds must lie on whole metres.')
    lower=np.rint(lower).astype(int);upper=np.rint(upper).astype(int)
    shape=tuple(int(v) for v in (upper-lower+1)[::-1]);dst.mkdir(parents=True)
    x=(np.arange(lower[0],upper[0]+1)-origin[0])/step
    z=(np.arange(lower[1],upper[1]+1)-origin[1])/step
    h=np.memmap(src/m['heights'],dtype='<f4',mode='r',shape=(nz,nx))
    if not np.isfinite(h).all():raise ValueError('Nonfinite source heights')
    out=np.memmap(dst/'height.f32',dtype='<f4',mode='w+',shape=shape)
    resample(h,x,z,out,cubic=True)
    print('Height published:',shape,flush=True)
    for mask in m['masks']:
        data=np.memmap(src/mask['file'],dtype='u1',mode='r',shape=(nz,nx))
        target=np.memmap(dst/mask['file'],dtype='u1',mode='w+',shape=shape)
        resample(data,x,z,target)
        del target
        print('Mask published:',mask['name'],flush=True)
    # The current game has two meadow textures. Adapt only the existing meadow
    # coverage: soil is green meadow; sand/rock/scree clear grass. Native masks
    # remain separate for future sand, rock, scree and wetness treatments.
    mats={name:np.memmap(dst/(name+'.u8'),dtype='u1',mode='r',shape=shape)
          for name in ('sand','rock','scree')}
    bare=np.memmap(dst/'bare.u8',dtype='u1',mode='w+',shape=shape)
    for first in range(0,shape[0],128):
        end=min(shape[0],first+128)
        values=sum(v[first:end].astype(np.uint16) for v in mats.values())
        bare[first:end]=np.minimum(values,255).astype(np.uint8)
    bare.flush();del bare
    m['masks'] += [{'name':'green','file':'soil.u8'},{'name':'bare','file':'bare.u8'}]
    m.update(samples=list(shape[::-1]),spacing=1.0,origin=lower.tolist(),start=a.start)
    m['source'] += ' | approved cooked terrain resampled to 1 m; no erosion recook'
    ix,iz=np.rint(np.array(a.start)-lower).astype(int)
    if not (0<=ix<shape[1] and 0<=iz<shape[0]):raise ValueError('Start outside domain')
    height=float(out[iz,ix]);slope=float(np.hypot((out[iz,ix+1]-out[iz,ix-1])*.5,(out[iz+1,ix]-out[iz-1,ix])*.5))
    if height < m['sea_level']+1 or slope>.2:raise ValueError('Start must be dry and gentle')
    report={'source_bundle':str(src.resolve()),'source_samples':[nx,nz],
            'source_spacing_m':step,'source_height_sha256':digest(src/'height.f32'),
            'export_samples':m['samples'],'spacing_m':1,'bounds_xz':[lower.tolist(),upper.tolist()],
            'height_range_m':[float(out.min()),float(out.max())],
            'all_finite':bool(np.isfinite(out).all()),'start_xyz':[a.start[0],height,a.start[1]],
            'start_slope':slope,'height_sha256':digest(dst/'height.f32'),
            'detail_note':'Interpolation preserves approved terrain; it does not synthesize 1 m erosion detail.',
            'material_adapter':'green=soil; bare=sand+rock+scree. Existing meadow textures only.'}
    (dst/'validation.json').write_text(json.dumps(report,indent=2)+'\n')
    for name in ('settings.json','scene.json'):
        if (src/name).exists():(dst/name).write_bytes((src/name).read_bytes())
    (dst/'heightfield.json').write_text(json.dumps(m,indent=2)+'\n')
    print(json.dumps(report,indent=2),flush=True)

if __name__=='__main__':main()
