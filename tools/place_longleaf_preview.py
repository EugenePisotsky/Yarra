#!/usr/bin/env python3
"""Place the current four-form longleaf pine kit.

Back up the source world first. Stable IDs preserve later editor adjustments and
allow safe repeat runs. Existing scenery and editor adjustments are preserved.
"""
import argparse
import json
import math
import sqlite3
import time
import uuid
from pathlib import Path
from place_birch_preview import ROOT, height

SAMPLES=[('near_birches','longleaf',2510.,4346.,.35,1.),
         ('half','longleaf_half_bare',2474.,4346.,.35,1.),
         ('nearly','longleaf_nearly_bare',2486.,4346.,.35,1.),
         ('one_side','longleaf_one_sided',2498.,4346.,0.,1.)]
VIEWS=[('longleaf-whole',2510.,4346.,28.,8.,24.),
       ('longleaf-close',2510.,4346.,28.,8.,8.),
       ('longleaf-side',2510.,4346.,115.,8.,9.),
       ('longleaf-overhead',2510.,4346.,28.,70.,23.),
       ('longleaf-far',2510.,4346.,28.,8.,24.),
       ('longleaf-kit',2492.,4346.,0.,8.,24.),
       ('longleaf-half',2474.,4346.,0.,8.,24.),
       ('longleaf-nearly',2486.,4346.,0.,8.,24.),
       ('longleaf-one-sided',2498.,4346.,0.,8.,24.),
       ('longleaf-bare-close',2486.,4346.,25.,8.,10.)]
HEIGHTS={'longleaf-whole':8.,'longleaf-close':11.,'longleaf-side':11.,
         'longleaf-overhead':11.,'longleaf-far':8.,'longleaf-kit':9.,
         'longleaf-half':8.,'longleaf-nearly':8.,'longleaf-one-sided':8.,'longleaf-bare-close':11.}
EXTRA={'longleaf-whole':12.,'longleaf-far':48.,'longleaf-kit':24.,
       'longleaf-half':12.,'longleaf-nearly':12.,'longleaf-one-sided':12.}


def object_id(tag,name):
    return uuid.uuid5(uuid.NAMESPACE_URL,'yarra:'+tag+'-preview/v1/'+name).bytes


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--project',type=Path,default=ROOT/'content/world.project.sqlite')
    project=parser.parse_args().project.resolve();view_dir=project.with_suffix('.views')
    with sqlite3.connect('file:'+str(project)+'?mode=rw',uri=True) as db:
        db.execute('PRAGMA foreign_keys=ON')
        if db.execute('SELECT name,cell_size FROM world_spaces WHERE id=1').fetchone()!=('Island',32.):
            raise ValueError('These coordinates are for the 32 m island world')
        backup=ROOT/'tmp'/('longleaf-kit-placement-'+str(time.time_ns()));backup.mkdir(parents=True)
        with sqlite3.connect(backup/'world.project.sqlite') as target:db.backup(target)
        report={'samples':[]}
        with db:
            db.execute('BEGIN IMMEDIATE')
            before=dict(db.execute('SELECT object_id,hex(definition_id)||":"||world_space_id||":"||owner_cell_x||":"||owner_cell_z||":"||local_x||":"||local_y||":"||local_z||":"||yaw||":"||scale||":"||source_revision FROM object_placements'))
            for name,state,x,z,yaw,scale in SAMPLES:
                oid=object_id('longleaf',name);key='asset/yarra_longleaf/pine_'+state
                definition=db.execute('SELECT definition_id FROM object_definitions WHERE definition_key=?',(key,)).fetchone()
                if definition is None:raise ValueError('Import the complete longleaf catalog first: '+key)
                existing=db.execute('SELECT definition_id FROM object_placements WHERE object_id=?',(oid,)).fetchone()
                if existing and existing!=definition:raise ValueError('Longleaf preview ID points to another asset')
                cx,cz=math.floor(x/32.),math.floor(z/32.);y=height(db,32.,x,z)-.025
                if y<=0:raise ValueError('Refusing to plant below sea level')
                if not existing:
                    db.execute('INSERT INTO object_placements VALUES (?,?,?,?,?,?,?,?,?,?,1)',
                               (oid,1,cx,cz,definition[0],x-cx*32.,y,z-cz*32.,yaw,scale))
                report['samples'].append({'object_id':oid.hex(),'asset':key,'created':not bool(existing),'position':[x,y,z]})
            after=dict(db.execute('SELECT object_id,hex(definition_id)||":"||world_space_id||":"||owner_cell_x||":"||owner_cell_z||":"||local_x||":"||local_y||":"||local_z||":"||yaw||":"||scale||":"||source_revision FROM object_placements'))
            assert all(after.get(oid)==value for oid,value in before.items()),'Unrelated placement changed'
            assert len(after)==len(before)+sum(s['created'] for s in report['samples'])
            assert not db.execute('PRAGMA foreign_key_check').fetchall()
        view_dir.mkdir(exist_ok=True)
        for name,x,z,yaw,pitch,distance in VIEWS:
            path=view_dir/(name+'.ron')
            if path.exists():continue
            y=height(db,32.,x,z)+HEIGHTS[name];extra=EXTRA.get(name,0.)
            angle=math.radians(yaw);elevation=math.radians(pitch)
            x+=math.sin(angle)*math.cos(elevation)*extra
            z+=math.cos(angle)*math.cos(elevation)*extra;y+=math.sin(elevation)*extra
            path.write_text(f'(position: ({x}, {y:.6f}, {z}), yaw_degrees: {yaw}, pitch_degrees: {pitch}, distance: {distance}, fog_visibility: 20000.0, route: [])\n')
        (backup/'placements.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(report,indent=2));print('Source backup and placement report:',backup)


if __name__=='__main__':main()
