//! Indexed junction membership and bounded one-hop connection reads.
use super::*;

pub(super) fn get(c: &Connection, id: RoadJunctionId) -> Result<SourceRoadJunction, WorldDbError> {
    let RoadSourceRecord::Junction(j) = required(c, RoadRecordKey::Junction(id))? else {
        unreachable!()
    };
    Ok(j)
}
pub(super) fn for_knot(
    c: &Connection,
    id: RoadKnotId,
) -> Result<Option<RoadJunctionId>, WorldDbError> {
    Ok(c.query_row(
        "SELECT junction_id FROM road_junction_knots WHERE knot_id=?1",
        [id.0.as_slice()],
        |r| {
            Ok(RoadJunctionId(crate::storage::blob_array(
                r.get_ref(0)?.as_blob()?,
                "junction id",
            )?))
        },
    )
    .optional()?)
}
pub(super) fn store(
    c: &Connection,
    source: &SourceRoadJunction,
    bytes: &[u8],
) -> Result<(), WorldDbError> {
    let j = &source.junction;
    let size = definition(c, source.space)?.cell_size;
    let bounds = j.bounds(size).map_err(|e| invalid(e.to_string()))?;
    if bounds
        .cell_count()
        .is_none_or(|n| n > MAX_ROAD_INDEX_CELLS_PER_SPAN)
    {
        return Err(invalid("junction index budget; use a smaller radius"));
    }
    c.execute("INSERT INTO road_junctions(id,world_space_id,profile_id,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET world_space_id=excluded.world_space_id,profile_id=excluded.profile_id,payload=excluded.payload",params![j.id.0.as_slice(),source.space.0,j.profile.0.as_slice(),bytes])?;
    c.execute(
        "DELETE FROM road_junction_knots WHERE junction_id=?1",
        [j.id.0.as_slice()],
    )?;
    for k in &j.knots {
        c.execute(
            "INSERT INTO road_junction_knots(junction_id,knot_id) VALUES(?1,?2)",
            params![j.id.0.as_slice(), k.0.as_slice()],
        )?;
    }
    c.execute(
        "DELETE FROM road_junction_cells WHERE junction_id=?1",
        [j.id.0.as_slice()],
    )?;
    for x in bounds.minimum.x..=bounds.maximum.x {
        for z in bounds.minimum.z..=bounds.maximum.z {
            c.execute(
                "INSERT INTO road_junction_cells VALUES(?1,?2,?3,?4)",
                params![source.space.0, x, z, j.id.0.as_slice()],
            )?;
        }
    }
    Ok(())
}
pub(super) fn validate(c: &Connection, source: &SourceRoadJunction) -> Result<(), WorldDbError> {
    let j = &source.junction;
    let size = definition(c, source.space)?.cell_size;
    j.validate(size).map_err(|e| invalid(e.to_string()))?;
    let mut roads = BTreeMap::new();
    let mut spans = BTreeMap::new();
    for id in &j.knots {
        let k = knot(c, *id)?;
        let r = route(c, k.road)?;
        if r.space != source.space {
            return Err(invalid("junction cannot connect different worlds"));
        }
        roads.insert(r.road.id, r.road);
        let incident = adjacent(c, *id)?;
        if incident.is_empty() {
            return Err(invalid("junction cannot contain an unused road point"));
        }
        for id in incident {
            spans.insert(id, span(c, id)?);
        }
    }
    RoadSnapshot {
        space: source.space,
        cell_size: size,
        loaded_bounds: j.bounds(size).map_err(|e| invalid(e.to_string()))?,
        truncated: false,
        roads: roads.into_values().collect(),
        profiles: vec![profile(c, j.profile)?],
        spans: spans.into_values().collect(),
        junctions: vec![j.clone()],
    }
    .validate()
    .map_err(|e| invalid(e.to_string()))
}
/// Discover junctions in the window, and at the endpoints of initially discovered spans.
/// Complete their incident spans once; never recursively load the road network.
pub(super) fn expand(
    c: &Connection,
    space: WorldSpaceId,
    bounds: RoadCellBounds,
    ids: &mut BTreeSet<RoadSpanId>,
) -> Result<Vec<RoadJunction>, WorldDbError> {
    let mut found = BTreeSet::new();
    let mut q=c.prepare_cached("SELECT junction_id FROM road_junction_cells WHERE world_space_id=?1 AND cell_x=?2 AND cell_z=?3 ORDER BY junction_id LIMIT 65")?;
    for x in bounds.minimum.x..=bounds.maximum.x {
        for z in bounds.minimum.z..=bounds.maximum.z {
            for id in q.query_map(params![space.0, x, z], |r| {
                Ok(RoadJunctionId(crate::storage::blob_array(
                    r.get_ref(0)?.as_blob()?,
                    "junction id",
                )?))
            })? {
                found.insert(id?);
                if found.len() > MAX_ROAD_RECORDS {
                    return Err(invalid("junction query budget"));
                }
            }
        }
    }
    for id in ids.iter() {
        let s = span(c, *id)?;
        for k in [s.start.id, s.end.id] {
            if let Some(j) = for_knot(c, k)? {
                found.insert(j);
            }
        }
        if found.len() > MAX_ROAD_RECORDS {
            return Err(invalid("junction query budget"));
        }
    }
    let mut junctions = vec![];
    for id in found {
        let j = get(c, id)?;
        if j.space != space {
            return Err(invalid("foreign junction"));
        }
        for k in &j.junction.knots {
            ids.extend(adjacent(c, *k)?);
        }
        if ids.len() > MAX_ROAD_SPANS {
            return Err(invalid("junction incident-span budget"));
        }
        junctions.push(j.junction);
    }
    Ok(junctions)
}
pub(super) fn touching(
    c: &Connection,
    key: RoadRecordKey,
) -> Result<BTreeSet<RoadJunctionId>, WorldDbError> {
    let mut ids = BTreeSet::new();
    match key {
        RoadRecordKey::Junction(id) => {
            ids.insert(id);
        }
        RoadRecordKey::Knot(id) => {
            if let Some(j) = for_knot(c, id)? {
                ids.insert(j);
            }
        }
        RoadRecordKey::Span(id) => {
            if let Some(RoadSourceRecord::Span(s)) = read_state(c, RoadRecordKey::Span(id))?.record
            {
                for k in [s.start, s.end] {
                    if let Some(j) = for_knot(c, k)? {
                        ids.insert(j);
                    }
                }
            }
        }

        _ => {}
    }
    if ids.len() > MAX_ROAD_RECORDS {
        return Err(invalid("junction dependency budget"));
    }
    Ok(ids)
}
