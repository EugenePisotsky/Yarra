//! Bounded normalized snapshots for editor gestures. One-hop incident spans protect shared knots
//! without expanding a long road into the viewport working set.
use super::*;

pub fn road_snapshot_records(snapshot: &RoadSnapshot) -> BTreeMap<RoadRecordKey, RoadSourceRecord> {
    let mut records = BTreeMap::new();
    for j in &snapshot.junctions {
        let r = RoadSourceRecord::Junction(SourceRoadJunction {
            space: snapshot.space,
            junction: j.clone(),
        });
        records.insert(r.key(), r);
    }
    for p in &snapshot.profiles {
        let r = RoadSourceRecord::Profile(p.clone());
        records.insert(r.key(), r);
    }
    for r in &snapshot.roads {
        let r = RoadSourceRecord::Road(SourceRoad {
            space: snapshot.space,
            road: r.clone(),
        });
        records.insert(r.key(), r);
    }
    for s in &snapshot.spans {
        for k in [&s.start, &s.end] {
            let r = RoadSourceRecord::Knot(SourceRoadKnot {
                road: s.road,
                knot: k.clone(),
            });
            records.insert(r.key(), r);
        }
        let r = RoadSourceRecord::Span(SourceRoadSpan {
            id: s.id,
            revision: s.revision,
            road: s.road,
            start: s.start.id,
            end: s.end.id,
        });
        records.insert(r.key(), r);
    }
    records
}

/// Normalize draft records into a compiler window without enumerating their occupied cells.
/// The caller supplies complete references and a complete database query for this window.
pub fn road_snapshot_from_records(
    space: WorldSpaceId,
    cell_size: f32,
    bounds: RoadCellBounds,
    records: &BTreeMap<RoadRecordKey, RoadSourceRecord>,
) -> Result<RoadSnapshot, WorldDbError> {
    if records.len() > MAX_ROAD_DEPENDENCIES {
        return Err(invalid("road preview record budget"));
    }
    let mut roads = BTreeMap::new();
    let mut profiles = BTreeMap::new();
    let junctions = records
        .values()
        .filter_map(|r| match r {
            RoadSourceRecord::Junction(j) if j.space == space => Some(j.junction.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let junction_knots = junctions
        .iter()
        .flat_map(|j| j.knots.iter().copied())
        .collect::<BTreeSet<_>>();
    let mut spans = vec![];
    for record in records.values() {
        let RoadSourceRecord::Span(s) = record else {
            continue;
        };
        let Some(RoadSourceRecord::Road(r)) = records.get(&RoadRecordKey::Road(s.road)) else {
            return Err(invalid("missing draft road"));
        };
        if r.space != space {
            continue;
        }
        let Some(RoadSourceRecord::Profile(p)) =
            records.get(&RoadRecordKey::Profile(r.road.profile))
        else {
            return Err(invalid("missing draft road profile"));
        };
        let get = |id| match records.get(&RoadRecordKey::Knot(id)) {
            Some(RoadSourceRecord::Knot(k)) if k.road == s.road => Ok(k.knot.clone()),
            _ => Err(invalid("missing or foreign draft knot")),
        };
        let span = RoadSpan {
            id: s.id,
            revision: s.revision,
            road: s.road,
            start: get(s.start)?,
            end: get(s.end)?,
        };
        if !junction_knots.contains(&s.start)
            && !junction_knots.contains(&s.end)
            && !influence_bounds(&span, p, cell_size)
                .map_err(|e| invalid(e.to_string()))?
                .intersects(bounds)
        {
            continue;
        }
        roads.insert(r.road.id, r.road.clone());
        profiles.insert(p.id, p.clone());
        spans.push(span);
    }
    let snapshot = RoadSnapshot {
        space,
        cell_size,
        loaded_bounds: bounds,
        truncated: false,
        junctions,
        roads: roads.into_values().collect(),
        profiles: profiles.into_values().collect(),
        spans,
    };
    snapshot.validate().map_err(|e| invalid(e.to_string()))?;
    Ok(snapshot)
}
/// Aggregate dependency information, including roads outside the loaded window.
#[derive(Debug, Clone)]
pub struct RoadStyleUsage {
    pub space: WorldSpaceId,
    pub road_count: u64,
    pub minimum_width: Option<f32>,
}
#[derive(Debug, Clone)]
pub struct RoadAuthoringSnapshot {
    pub revision: u64,
    pub records: Vec<RoadRecordState>,
    pub style_usage: BTreeMap<RoadProfileId, Vec<RoadStyleUsage>>,
    pub route_minimum_widths: BTreeMap<RoadId, Option<f32>>,
    /// Only these knots have all incident spans in this snapshot.
    pub complete_knots: BTreeSet<RoadKnotId>,
}
impl ProjectReader {
    pub fn read_road_authoring_snapshot(
        &self,
        space: WorldSpaceId,
        bounds: RoadCellBounds,
        pins: &[RoadRecordKey],
    ) -> Result<RoadAuthoringSnapshot, WorldDbError> {
        if pins.len() > MAX_ROAD_DEPENDENCIES {
            return Err(invalid("road authoring pin budget"));
        }
        let tx = self.connection.unchecked_transaction()?;
        let near = query::read_snapshot(&tx, space, bounds)?;
        if near.roads.truncated {
            return Err(invalid("too many nearby roads; reduce the query area"));
        }
        let mut keys = road_snapshot_records(&near.roads)
            .into_keys()
            .chain(pins.iter().copied())
            .collect::<BTreeSet<_>>();
        // Styles are a bounded project library, independent of nearby route membership.
        // Reusing them prevents an identical new profile for every distant road.
        let mut q = tx.prepare("SELECT id FROM road_profiles ORDER BY id LIMIT 65")?;
        let ids = q
            .query_map([], |row| {
                Ok(RoadProfileId(crate::blob_array(
                    row.get_ref(0)?.as_blob()?,
                    "profile id",
                )?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if ids.len() > MAX_ROAD_RECORDS {
            return Err(invalid("project road profile limit is 64"));
        }
        // Result size is bounded by 64 styles × 32 world definitions. Aggregate in SQLite;
        // distant control points are never materialized in the editor.
        let mut style_usage = BTreeMap::new();
        for id in &ids {
            let mut q = tx.prepare("SELECT r.world_space_id, COUNT(DISTINCT r.id), MIN(k.width) FROM roads r LEFT JOIN road_knots k ON k.road_id=r.id WHERE r.profile_id=?1 GROUP BY r.world_space_id ORDER BY r.world_space_id LIMIT 33")?;
            let usages = q
                .query_map([id.0.as_slice()], |row| {
                    Ok(RoadStyleUsage {
                        space: WorldSpaceId(row.get(0)?),
                        road_count: row.get::<_, i64>(1)? as u64,
                        minimum_width: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            if usages.len() > crate::MAX_ENVIRONMENT_DEFINITIONS {
                return Err(invalid("road world limit"));
            }
            style_usage.insert(*id, usages);
        }
        keys.extend(ids.into_iter().map(RoadRecordKey::Profile));
        let connected = keys
            .iter()
            .filter_map(|key| match key {
                RoadRecordKey::Knot(k) => Some(*k),
                _ => None,
            })
            .collect::<Vec<_>>();
        for k in connected {
            if let Some(id) = junctions::for_knot(&tx, k)? {
                keys.insert(RoadRecordKey::Junction(id));
            }
        }
        let junction_keys = keys
            .iter()
            .filter_map(|key| match key {
                RoadRecordKey::Junction(j) => Some(*j),
                _ => None,
            })
            .collect::<Vec<_>>();
        for id in junction_keys {
            if let Some(RoadSourceRecord::Junction(j)) =
                read_state(&tx, RoadRecordKey::Junction(id))?.record
            {
                keys.extend(j.junction.knots.iter().copied().map(RoadRecordKey::Knot));
            }
        }
        let complete_knots = keys
            .iter()
            .filter_map(|k| {
                if let RoadRecordKey::Knot(id) = k {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>();
        for id in &complete_knots {
            keys.extend(adjacent(&tx, *id)?.into_iter().map(RoadRecordKey::Span));
        }
        let mut records = BTreeMap::new();
        let mut pending = keys.into_iter().collect::<Vec<_>>();
        while let Some(key) = pending.pop() {
            if records.contains_key(&key) {
                continue;
            }
            if records.len() >= MAX_ROAD_DEPENDENCIES {
                return Err(invalid("road authoring reference budget"));
            }
            let state = read_state(&tx, key)?;
            if let Some(r) = &state.record {
                pending.extend(r.references());
            }
            records.insert(key, state);
        }
        let mut route_minimum_widths = BTreeMap::new();
        for state in records.values() {
            if let Some(RoadSourceRecord::Road(r)) = &state.record {
                let width = tx.query_row(
                    "SELECT MIN(width) FROM road_knots WHERE road_id=?1",
                    [r.road.id.0.as_slice()],
                    |row| row.get(0),
                )?;
                route_minimum_widths.insert(r.road.id, width);
            }
        }
        Ok(RoadAuthoringSnapshot {
            style_usage,
            route_minimum_widths,
            revision: near.revision,
            records: records.into_values().collect(),
            complete_knots,
        })
    }
}
