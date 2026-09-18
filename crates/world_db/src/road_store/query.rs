use super::*;
pub(crate) fn read_snapshot(
    c: &Connection,
    space: WorldSpaceId,
    bounds: RoadCellBounds,
) -> Result<RoadReadSnapshot, WorldDbError> {
    if bounds.cell_count().is_none_or(|n| n > MAX_ROAD_QUERY_CELLS) {
        return Err(invalid("road query exceeds 576 cells"));
    }
    let d = definition(c, space)?;
    let mut ids = BTreeSet::new();
    let mut rows_read = 0;
    let mut truncated = false;
    let mut q=c.prepare_cached("SELECT span_id FROM road_span_cells WHERE world_space_id=?1 AND cell_x=?2 AND cell_z=?3 ORDER BY span_id LIMIT ?4")?;
    'cells: for x in bounds.minimum.x..=bounds.maximum.x {
        for z in bounds.minimum.z..=bounds.maximum.z {
            let mut rows = q.query(params![
                space.0,
                x,
                z,
                (MAX_ROAD_QUERY_MEMBERSHIPS - rows_read + 1) as i64
            ])?;
            while let Some(row) = rows.next()? {
                rows_read += 1;
                if rows_read > MAX_ROAD_QUERY_MEMBERSHIPS {
                    truncated = true;
                    break 'cells;
                }
                ids.insert(RoadSpanId(crate::blob_array(
                    row.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?,
                    "span id",
                )?));
                if ids.len() > MAX_ROAD_SPANS {
                    truncated = true;
                    break 'cells;
                }
            }
        }
    }
    let junctions = if truncated {
        vec![]
    } else {
        junctions::expand(c, space, bounds, &mut ids)?
    };
    let mut roads: BTreeMap<RoadId, Road> = BTreeMap::new();
    let mut profiles: BTreeMap<RoadProfileId, CartTrackProfile> = BTreeMap::new();
    let mut spans = vec![];
    for id in ids.into_iter().take(MAX_ROAD_SPANS) {
        let s = span(c, id)?;
        let r = if let Some(r) = roads.get(&s.road) {
            r.clone()
        } else {
            let source = route(c, s.road)?;
            if source.space != space {
                return Err(invalid("spatial index references another world"));
            }
            source.road
        };
        if roads.len() >= MAX_ROAD_RECORDS && !roads.contains_key(&r.id) {
            truncated = true;
            break;
        }
        let p = if let Some(p) = profiles.get(&r.profile) {
            p.clone()
        } else {
            profile(c, r.profile)?
        };
        if profiles.len() >= MAX_ROAD_RECORDS && !profiles.contains_key(&p.id) {
            truncated = true;
            break;
        }
        roads.insert(r.id, r);
        profiles.insert(p.id, p);
        spans.push(s);
    }
    let roads = RoadSnapshot {
        space,
        cell_size: d.cell_size,
        loaded_bounds: bounds,
        truncated,
        junctions,
        roads: roads.into_values().collect(),
        profiles: profiles.into_values().collect(),
        spans,
    };
    if !truncated {
        roads.validate().map_err(|e| invalid(e.to_string()))?;
    }
    Ok(RoadReadSnapshot {
        revision: epoch(c)?,
        roads,
    })
}
impl ProjectReader {
    pub fn read_roads_in_cells(
        &self,
        space: WorldSpaceId,
        bounds: RoadCellBounds,
    ) -> Result<RoadReadSnapshot, WorldDbError> {
        let tx = self.connection.unchecked_transaction()?;
        read_snapshot(&tx, space, bounds)
    }
    pub fn read_road_records(
        &self,
        keys: &[RoadRecordKey],
    ) -> Result<Vec<RoadRecordState>, WorldDbError> {
        if keys.len() > MAX_ROAD_DEPENDENCIES
            || keys.iter().collect::<BTreeSet<_>>().len() != keys.len()
        {
            return Err(invalid("record read budget or duplicate identity"));
        }
        let tx = self.connection.unchecked_transaction()?;
        keys.iter().map(|&key| read_state(&tx, key)).collect()
    }
    /// Both source domains and all referenced presets come from one SQLite read transaction.
    /// `coverage_cells` includes the painter's halo; road bounds certify the target cells.
    pub fn read_environment_with_roads(
        &self,
        space: WorldSpaceId,
        coverage_cells: &[CellCoord],
        bounds: RoadCellBounds,
    ) -> Result<RoadEnvironmentSnapshot, WorldDbError> {
        let tx = self.connection.unchecked_transaction()?;
        let environment = crate::environment_store::read_snapshot(&tx, space, coverage_cells)?;
        let roads = read_snapshot(&tx, space, bounds)?;
        Ok(RoadEnvironmentSnapshot { environment, roads })
    }
    pub fn read_road_dependency_spans(
        &self,
        selector: RoadDependencySelector,
        after: Option<RoadSpanId>,
        limit: usize,
    ) -> Result<RoadDependencyPage, WorldDbError> {
        if limit == 0 || limit > MAX_ROAD_SPANS {
            return Err(invalid("dependency page budget"));
        }
        let tx = self.connection.unchecked_transaction()?;
        let (filter, id) = match selector {
            RoadDependencySelector::Road(id) => ("r.id", id.0),
            RoadDependencySelector::Profile(id) => ("r.profile_id", id.0),
            RoadDependencySelector::GroundPreset(id) => ("p.ground_preset", id.0),
        };
        let mut q=tx.prepare(&format!("SELECT s.id,r.world_space_id,s.min_x,s.max_x,s.min_z,s.max_z FROM road_spans s JOIN roads r ON r.id=s.road_id JOIN road_profiles p ON p.id=r.profile_id WHERE {filter}=?1 AND s.id>=?2 ORDER BY s.id LIMIT ?3"))?;
        let minimum = after.map_or([0; 16], |id| id.0);
        let mut spans = q
            .query_map(
                params![id.as_slice(), minimum.as_slice(), limit as i64 + 2],
                |row| {
                    Ok((
                        RoadSpanId(crate::blob_array(
                            row.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?,
                            "span id",
                        )?),
                        RoadAffectedBounds {
                            space: WorldSpaceId(row.get(1)?),
                            bounds: RoadCellBounds {
                                minimum: CellCoord {
                                    x: row.get(2)?,
                                    z: row.get(4)?,
                                },
                                maximum: CellCoord {
                                    x: row.get(3)?,
                                    z: row.get(5)?,
                                },
                            },
                        },
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(after) = after {
            spans.retain(|(id, _)| *id != after);
        }
        let more = spans.len() > limit;
        spans.truncate(limit);
        // Route/style changes also affect their junction patches, which can extend
        // beyond the ordinary corridor's index padding. Keep pagination by span ID.
        let mut junction_bounds = BTreeMap::new();
        for (id, affected) in &mut spans {
            let s = span(&tx, *id)?;
            for knot in [s.start.id, s.end.id] {
                if let Some(id) = junctions::for_knot(&tx, knot)? {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        junction_bounds.entry(id)
                    {
                        let j = junctions::get(&tx, id)?;
                        let size = definition(&tx, j.space)?.cell_size;
                        entry.insert(
                            j.junction
                                .bounds(size)
                                .map_err(|e| invalid(e.to_string()))?,
                        );
                    }
                    let b = junction_bounds[&id];
                    affected.bounds.minimum.x = affected.bounds.minimum.x.min(b.minimum.x);
                    affected.bounds.minimum.z = affected.bounds.minimum.z.min(b.minimum.z);
                    affected.bounds.maximum.x = affected.bounds.maximum.x.max(b.maximum.x);
                    affected.bounds.maximum.z = affected.bounds.maximum.z.max(b.maximum.z);
                }
            }
        }
        let next_cursor = if more {
            spans.last().map(|(id, _)| *id)
        } else {
            None
        };
        let library_revision = tx.query_row(
            "SELECT revision FROM environment_preset_state WHERE singleton=1",
            [],
            |row| row.get::<_, i64>(0),
        )? as u64;
        Ok(RoadDependencyPage {
            road_revision: epoch(&tx)?,
            library_revision,
            spans,
            next_cursor,
        })
    }
}
