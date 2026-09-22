use super::*;
pub(super) fn read_state(
    c: &Connection,
    key: RoadRecordKey,
) -> Result<RoadRecordState, WorldDbError> {
    let (kind, id, table) = key.parts();
    let revision = c
        .query_row(
            "SELECT revision FROM road_versions WHERE kind=?1 AND id=?2",
            params![kind, id.as_slice()],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    let raw = c
        .query_row(
            &format!("SELECT payload FROM {table} WHERE id=?1"),
            [id.as_slice()],
            |r| {
                let bytes = r.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?;
                if bytes.len() > MAX_ROAD_BYTES {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(bytes.to_vec())
            },
        )
        .optional()?;
    let record = raw
        .map(|bytes| {
            let (record, n): (RoadSourceRecord, usize) = bincode::serde::decode_from_slice(
                &bytes,
                bincode::config::standard().with_limit::<MAX_ROAD_BYTES>(),
            )?;
            if n != bytes.len()
                || record.key() != key
                || revision.is_none_or(|r| r <= 0 || r as u64 != record.revision())
            {
                return Err(invalid("record identity/revision or payload mismatch"));
            }
            Ok(record)
        })
        .transpose()?;
    if revision.is_some_and(|r| r <= 0) {
        return Err(invalid("invalid record revision"));
    }
    Ok(RoadRecordState {
        key,
        revision: revision.map(|r| r as u64),
        record,
    })
}
pub(super) fn required(
    c: &Connection,
    key: RoadRecordKey,
) -> Result<RoadSourceRecord, WorldDbError> {
    read_state(c, key)?
        .record
        .ok_or_else(|| invalid(format!("missing dependency {key:?}")))
}
pub(super) fn route(c: &Connection, id: RoadId) -> Result<SourceRoad, WorldDbError> {
    match required(c, RoadRecordKey::Road(id))? {
        RoadSourceRecord::Road(r) => Ok(r),
        _ => unreachable!(),
    }
}
pub(super) fn profile(c: &Connection, id: RoadProfileId) -> Result<CartTrackProfile, WorldDbError> {
    match required(c, RoadRecordKey::Profile(id))? {
        RoadSourceRecord::Profile(r) => Ok(r),
        _ => unreachable!(),
    }
}
pub(super) fn knot(c: &Connection, id: RoadKnotId) -> Result<SourceRoadKnot, WorldDbError> {
    match required(c, RoadRecordKey::Knot(id))? {
        RoadSourceRecord::Knot(r) => Ok(r),
        _ => unreachable!(),
    }
}
pub(super) fn span(c: &Connection, id: RoadSpanId) -> Result<RoadSpan, WorldDbError> {
    let RoadSourceRecord::Span(s) = required(c, RoadRecordKey::Span(id))? else {
        unreachable!()
    };
    let a = knot(c, s.start)?;
    let b = knot(c, s.end)?;
    if a.road != s.road || b.road != s.road {
        return Err(invalid("span endpoints belong to another road"));
    }
    Ok(RoadSpan {
        id: s.id,
        revision: s.revision,
        road: s.road,
        start: a.knot,
        end: b.knot,
    })
}
pub(super) fn store(c: &Connection, record: &RoadSourceRecord) -> Result<(), WorldDbError> {
    // Bound variable-length fields before allocating their encoded representation.
    match record {
        RoadSourceRecord::Profile(p) => p.validate().map_err(|e| invalid(e.to_string()))?,
        RoadSourceRecord::Road(r) => r.road.validate().map_err(|e| invalid(e.to_string()))?,
        _ => {}
    }
    let bytes = bincode::serde::encode_to_vec(record, bincode::config::standard())?;
    if bytes.len() > MAX_ROAD_BYTES {
        return Err(invalid("record byte budget"));
    }
    let (kind, id, _) = record.key().parts();
    let revision = i64::try_from(record.revision()).map_err(|_| WorldDbError::IntegerOverflow)?;
    if revision <= 0 {
        return Err(invalid("invalid record revision"));
    }
    c.execute("INSERT INTO road_versions(kind,id,revision) VALUES(?1,?2,?3) ON CONFLICT(kind,id) DO UPDATE SET revision=excluded.revision",params![kind,id.as_slice(),revision])?;
    match record {
        RoadSourceRecord::Junction(j) => junctions::store(c, j, &bytes)?,
        RoadSourceRecord::Profile(p) => {
            c.execute("INSERT INTO road_profiles(id,ground_preset,payload) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET ground_preset=excluded.ground_preset,payload=excluded.payload",params![id.as_slice(),p.ground.0.as_slice(),bytes])?;
        }
        RoadSourceRecord::Road(r) => {
            c.execute("INSERT INTO roads(id,world_space_id,profile_id,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET world_space_id=excluded.world_space_id,profile_id=excluded.profile_id,payload=excluded.payload",params![id.as_slice(),r.space.0,r.road.profile.0.as_slice(),bytes])?;
        }
        RoadSourceRecord::Knot(k) => {
            c.execute("INSERT INTO road_knots(id,road_id,width,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET road_id=excluded.road_id,width=excluded.width,payload=excluded.payload",params![id.as_slice(),k.road.0.as_slice(),k.knot.width,bytes])?;
        }
        RoadSourceRecord::Span(s) => {
            c.execute("INSERT INTO road_spans(id,road_id,start_id,end_id,min_x,max_x,min_z,max_z,payload) VALUES(?1,?2,?3,?4,0,0,0,0,?5) ON CONFLICT(id) DO UPDATE SET road_id=excluded.road_id,start_id=excluded.start_id,end_id=excluded.end_id,payload=excluded.payload",params![id.as_slice(),s.road.0.as_slice(),s.start.0.as_slice(),s.end.0.as_slice(),bytes])?;
        }
    }
    Ok(())
}
pub(super) fn delete(
    c: &Connection,
    key: RoadRecordKey,
    revision: u64,
) -> Result<(), WorldDbError> {
    let (kind, id, table) = key.parts();
    c.execute(&format!("DELETE FROM {table} WHERE id=?1"), [id.as_slice()])?;
    c.execute(
        "UPDATE road_versions SET revision=?3 WHERE kind=?1 AND id=?2",
        params![kind, id.as_slice(), revision as i64],
    )?;
    Ok(())
}
/// Index padding is profile-independent: maximum supported edge variation + softness.
/// Thus changing a shared profile never needs a synchronous rewrite of every span index.
pub(super) fn index_bounds(s: &RoadSpan, size: f32) -> Result<RoadCellBounds, WorldDbError> {
    let points = s.control_points(s.start.position.cell, size);
    let radius = f64::from(s.start.width.max(s.end.width)) * 0.5 + 8.0;
    let min =
        std::array::from_fn(|i| points.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min) - radius);
    let max = std::array::from_fn(|i| {
        points
            .iter()
            .map(|p| p[i])
            .fold(f64::NEG_INFINITY, f64::max)
            + radius
    });
    let normalize = |p| {
        RoadPoint::from_relative(s.start.position.cell, p, f64::from(size))
            .map(|p| p.cell)
            .map_err(|e| invalid(e.to_string()))
    };
    let bounds = RoadCellBounds {
        minimum: normalize(min)?,
        maximum: normalize(max)?,
    };
    if bounds
        .cell_count()
        .is_none_or(|n| n > MAX_ROAD_INDEX_CELLS_PER_SPAN)
    {
        return Err(invalid(
            "span spatial index exceeds 4096 cells; split the curve into shorter spans",
        ));
    }
    Ok(bounds)
}
pub(super) fn saved_bounds(
    c: &Connection,
    id: RoadSpanId,
) -> Result<Option<RoadAffectedBounds>, WorldDbError> {
    Ok(c.query_row("SELECT r.world_space_id,s.min_x,s.max_x,s.min_z,s.max_z FROM road_spans s JOIN roads r ON r.id=s.road_id WHERE s.id=?1",[id.0.as_slice()],|row|Ok(RoadAffectedBounds{space:WorldSpaceId(row.get(0)?),bounds:RoadCellBounds{minimum:CellCoord{x:row.get(1)?,z:row.get(3)?},maximum:CellCoord{x:row.get(2)?,z:row.get(4)?}}})).optional()?)
}
pub(super) fn reindex(
    c: &Connection,
    id: RoadSpanId,
    remaining: Option<&mut usize>,
) -> Result<RoadAffectedBounds, WorldDbError> {
    let s = span(c, id)?;
    let r = route(c, s.road)?;
    let p = profile(c, r.road.profile)?;
    let d = definition(c, r.space)?;
    RoadSnapshot {
        space: r.space,
        cell_size: d.cell_size,
        loaded_bounds: RoadCellBounds {
            minimum: CellCoord::ZERO,
            maximum: CellCoord::ZERO,
        },
        truncated: false,
        junctions: vec![],
        roads: vec![r.road],
        profiles: vec![p],
        spans: vec![s.clone()],
    }
    .validate()
    .map_err(|e| invalid(e.to_string()))?;
    let bounds = index_bounds(&s, d.cell_size)?;
    if let Some(remaining) = remaining {
        *remaining = remaining
            .checked_sub(bounds.cell_count().unwrap())
            .ok_or_else(|| invalid("gesture exceeds the road spatial-index write budget"))?;
    }
    c.execute(
        "DELETE FROM road_span_cells WHERE span_id=?1",
        [id.0.as_slice()],
    )?;
    c.execute(
        "UPDATE road_spans SET min_x=?2,max_x=?3,min_z=?4,max_z=?5 WHERE id=?1",
        params![
            id.0.as_slice(),
            bounds.minimum.x,
            bounds.maximum.x,
            bounds.minimum.z,
            bounds.maximum.z
        ],
    )?;
    let mut insert = c.prepare_cached(
        "INSERT INTO road_span_cells(world_space_id,cell_x,cell_z,span_id) VALUES(?1,?2,?3,?4)",
    )?;
    for x in bounds.minimum.x..=bounds.maximum.x {
        for z in bounds.minimum.z..=bounds.maximum.z {
            insert.execute(params![r.space.0, x, z, id.0.as_slice()])?;
        }
    }
    Ok(RoadAffectedBounds {
        space: r.space,
        bounds,
    })
}
pub(super) fn adjacent(c: &Connection, id: RoadKnotId) -> Result<Vec<RoadSpanId>, WorldDbError> {
    let mut q=c.prepare("SELECT id FROM road_spans WHERE start_id=?1 UNION SELECT id FROM road_spans WHERE end_id=?1 LIMIT 3")?;
    let ids = q
        .query_map([id.0.as_slice()], |r| {
            Ok(RoadSpanId(crate::storage::blob_array(
                r.get_ref(0)?.as_blob()?,
                "span id",
            )?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > 2 {
        return Err(invalid("branching knot"));
    }
    Ok(ids)
}
