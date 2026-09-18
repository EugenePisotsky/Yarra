use super::*;
pub(crate) fn write_document(c: &Connection, document: &RoadDocument) -> Result<(), WorldDbError> {
    let mut keys = BTreeSet::new();
    for record in &document.records {
        if !keys.insert(record.key()) {
            return Err(invalid("duplicate road document record"));
        }
        store(c, record)?;
    }
    let library = read_library(c)?;
    transaction::validate_shared(c, &library)?;
    for record in &document.records {
        transaction::validate_record(c, record)?;
    }
    for key in keys {
        if let RoadRecordKey::Span(id) = key {
            reindex(c, id, None)?;
        }
    }
    Ok(())
}
pub(crate) fn read_document(c: &Connection) -> Result<RoadDocument, WorldDbError> {
    let mut records = vec![];
    // Whole-project import/export/reference tests only. Production cooking uses a snapshot.
    for (table, kind) in [
        ("road_profiles", 0),
        ("roads", 1),
        ("road_knots", 2),
        ("road_spans", 3),
        ("road_junctions", 4),
    ] {
        let mut q = c.prepare(&format!("SELECT id FROM {table} ORDER BY id"))?;
        let mut rows = q.query([])?;
        while let Some(row) = rows.next()? {
            let id = crate::blob_array(
                row.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?,
                "road record id",
            )?;
            let key = match kind {
                0 => RoadRecordKey::Profile(RoadProfileId(id)),
                1 => RoadRecordKey::Road(RoadId(id)),
                2 => RoadRecordKey::Knot(RoadKnotId(id)),
                3 => RoadRecordKey::Span(RoadSpanId(id)),
                _ => RoadRecordKey::Junction(RoadJunctionId(id)),
            };
            records.push(required(c, key)?);
        }
    }
    Ok(RoadDocument { records })
}
/// Offline index used by small whole-project reference fixtures. Construction may read the whole
/// document; every subsequent cell snapshot has the same limits as a database viewport query.
pub struct RoadDocumentIndex {
    profiles: BTreeMap<RoadProfileId, CartTrackProfile>,
    routes: BTreeMap<RoadId, SourceRoad>,
    spans: BTreeMap<RoadSpanId, RoadSpan>,
    cells: BTreeMap<(WorldSpaceId, CellCoord), Vec<RoadSpanId>>,
    junctions: BTreeMap<RoadJunctionId, SourceRoadJunction>,
    membership: BTreeMap<RoadKnotId, RoadJunctionId>,
    incident: BTreeMap<RoadKnotId, Vec<RoadSpanId>>,
    junction_cells: BTreeMap<(WorldSpaceId, CellCoord), Vec<RoadJunctionId>>,
}
impl RoadDocumentIndex {
    pub fn new(
        document: &RoadDocument,
        definitions: &[environment::EnvironmentDefinition],
    ) -> Result<Self, WorldDbError> {
        let mut profiles = BTreeMap::new();
        let mut routes = BTreeMap::new();
        let mut knots = BTreeMap::new();
        let mut source_spans = vec![];
        let mut junctions = BTreeMap::new();
        let mut unique = BTreeSet::new();
        for record in &document.records {
            if !unique.insert(record.key()) {
                return Err(invalid("duplicate document record"));
            }
            match record {
                RoadSourceRecord::Profile(p) => {
                    p.validate().map_err(|e| invalid(e.to_string()))?;
                    profiles.insert(p.id, p.clone());
                }
                RoadSourceRecord::Road(r) => {
                    routes.insert(r.road.id, r.clone());
                }
                RoadSourceRecord::Knot(k) => {
                    knots.insert(k.knot.id, k);
                }
                RoadSourceRecord::Span(s) => source_spans.push(s),
                RoadSourceRecord::Junction(j) => {
                    junctions.insert(j.junction.id, j.clone());
                }
            }
        }
        if profiles.len() > MAX_ROAD_RECORDS {
            return Err(invalid("project road profile limit is 64"));
        }
        let world = |id| {
            definitions
                .iter()
                .find(|d| d.space == id)
                .ok_or_else(|| invalid("missing road world"))
        };
        for r in routes.values() {
            let d = world(r.space)?;
            let p = profiles
                .get(&r.road.profile)
                .ok_or_else(|| invalid("missing road profile"))?;
            RoadSnapshot {
                space: r.space,
                cell_size: d.cell_size,
                loaded_bounds: RoadCellBounds {
                    minimum: CellCoord::ZERO,
                    maximum: CellCoord::ZERO,
                },
                truncated: false,
                junctions: vec![],
                roads: vec![r.road.clone()],
                profiles: vec![p.clone()],
                spans: vec![],
            }
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        }
        for k in knots.values() {
            let r = routes
                .get(&k.road)
                .ok_or_else(|| invalid("missing knot road"))?;
            k.knot
                .validate(world(r.space)?.cell_size, &profiles[&r.road.profile])
                .map_err(|e| invalid(e.to_string()))?;
        }
        let mut spans = BTreeMap::new();
        let mut cells: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut directions = BTreeSet::new();
        let mut incident: BTreeMap<RoadKnotId, Vec<RoadSpanId>> = BTreeMap::new();
        for s in source_spans {
            let r = routes
                .get(&s.road)
                .ok_or_else(|| invalid("missing span road"))?;
            let d = world(r.space)?;
            let a = knots
                .get(&s.start)
                .ok_or_else(|| invalid("missing start knot"))?;
            let b = knots
                .get(&s.end)
                .ok_or_else(|| invalid("missing end knot"))?;
            if a.road != s.road
                || b.road != s.road
                || !directions.insert((s.start, true))
                || !directions.insert((s.end, false))
            {
                return Err(invalid("span ownership or branching"));
            }
            let span = RoadSpan {
                id: s.id,
                revision: s.revision,
                road: s.road,
                start: a.knot.clone(),
                end: b.knot.clone(),
            };
            RoadSnapshot {
                space: r.space,
                cell_size: d.cell_size,
                loaded_bounds: RoadCellBounds {
                    minimum: CellCoord::ZERO,
                    maximum: CellCoord::ZERO,
                },
                truncated: false,
                junctions: vec![],
                roads: vec![r.road.clone()],
                profiles: vec![profiles[&r.road.profile].clone()],
                spans: vec![span.clone()],
            }
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
            let bounds = index_bounds(&span, d.cell_size)?;
            for x in bounds.minimum.x..=bounds.maximum.x {
                for z in bounds.minimum.z..=bounds.maximum.z {
                    cells
                        .entry((r.space, CellCoord { x, z }))
                        .or_default()
                        .push(s.id);
                }
            }
            for k in [s.start, s.end] {
                incident.entry(k).or_default().push(s.id);
            }
            spans.insert(s.id, span);
        }
        let mut membership = BTreeMap::new();
        let mut junction_cells: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for j in junctions.values() {
            let d = world(j.space)?;
            let bounds = j
                .junction
                .bounds(d.cell_size)
                .map_err(|e| invalid(e.to_string()))?;
            if bounds
                .cell_count()
                .is_none_or(|n| n > MAX_ROAD_INDEX_CELLS_PER_SPAN)
            {
                return Err(invalid("junction index budget"));
            }
            let mut js = BTreeMap::new();
            let mut jr = BTreeMap::new();
            for k in &j.junction.knots {
                if membership.insert(*k, j.junction.id).is_some() {
                    return Err(invalid("point belongs to two junctions"));
                }
                let connected = incident
                    .get(k)
                    .ok_or_else(|| invalid("junction has an unused point"))?;
                for id in connected {
                    let s = &spans[id];
                    if routes[&s.road].space != j.space {
                        return Err(invalid("junction connects different worlds"));
                    }
                    jr.insert(s.road, routes[&s.road].road.clone());
                    js.insert(*id, s.clone());
                }
            }
            let p = profiles
                .get(&j.junction.profile)
                .ok_or_else(|| invalid("missing junction style"))?;
            RoadSnapshot {
                space: j.space,
                cell_size: d.cell_size,
                loaded_bounds: bounds,
                truncated: false,
                roads: jr.into_values().collect(),
                profiles: vec![p.clone()],
                spans: js.into_values().collect(),
                junctions: vec![j.junction.clone()],
            }
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
            for x in bounds.minimum.x..=bounds.maximum.x {
                for z in bounds.minimum.z..=bounds.maximum.z {
                    junction_cells
                        .entry((j.space, CellCoord { x, z }))
                        .or_default()
                        .push(j.junction.id);
                }
            }
        }
        Ok(Self {
            junctions,
            membership,
            incident,
            junction_cells,
            profiles,
            routes,
            spans,
            cells,
        })
    }
    pub fn cell_snapshot(
        &self,
        space: WorldSpaceId,
        cell: CellCoord,
        cell_size: f32,
    ) -> Result<RoadSnapshot, WorldDbError> {
        self.snapshot(
            space,
            RoadCellBounds {
                minimum: cell,
                maximum: cell,
            },
            cell_size,
        )
    }
    pub fn snapshot(
        &self,
        space: WorldSpaceId,
        bounds: RoadCellBounds,
        cell_size: f32,
    ) -> Result<RoadSnapshot, WorldDbError> {
        if bounds.cell_count().is_none_or(|n| n > MAX_ROAD_QUERY_CELLS) {
            return Err(invalid("road snapshot query budget"));
        }
        let mut ids = BTreeSet::new();
        for x in bounds.minimum.x..=bounds.maximum.x {
            for z in bounds.minimum.z..=bounds.maximum.z {
                if let Some(found) = self.cells.get(&(space, CellCoord { x, z })) {
                    ids.extend(found.iter().copied());
                }
                if ids.len() > MAX_ROAD_SPANS {
                    return Err(invalid("road terrain halo exceeds 256 spans"));
                }
            }
        }
        if ids.len() > MAX_ROAD_SPANS {
            return Err(invalid("road cell exceeds 256 spans"));
        }
        let mut junction_ids = BTreeSet::new();
        for x in bounds.minimum.x..=bounds.maximum.x {
            for z in bounds.minimum.z..=bounds.maximum.z {
                if let Some(js) = self.junction_cells.get(&(space, CellCoord { x, z })) {
                    junction_ids.extend(js);
                }
            }
        }
        for id in &ids {
            let s = &self.spans[id];
            for k in [s.start.id, s.end.id] {
                if let Some(j) = self.membership.get(&k) {
                    junction_ids.insert(j);
                }
            }
        }
        if junction_ids.len() > MAX_ROAD_RECORDS {
            return Err(invalid("junction query budget"));
        }
        let junctions = junction_ids
            .into_iter()
            .map(|id| self.junctions[id].junction.clone())
            .collect::<Vec<_>>();
        for j in &junctions {
            for k in &j.knots {
                ids.extend(&self.incident[k]);
            }
        }
        if ids.len() > MAX_ROAD_SPANS {
            return Err(invalid("junction incident-span budget"));
        }
        let mut spans = ids
            .iter()
            .map(|id| self.spans[id].clone())
            .collect::<Vec<_>>();
        spans.sort_by_key(|s| s.id);
        let routes = spans.iter().map(|s| s.road).collect::<BTreeSet<_>>();
        let roads = routes
            .into_iter()
            .map(|id| self.routes[&id].road.clone())
            .collect::<Vec<_>>();
        let profiles = roads
            .iter()
            .map(|r| r.profile)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| self.profiles[&id].clone())
            .collect();
        let snapshot = RoadSnapshot {
            space,
            cell_size,
            loaded_bounds: bounds,
            truncated: false,
            junctions,
            roads,
            profiles,
            spans,
        };
        snapshot.validate().map_err(|e| invalid(e.to_string()))?;
        Ok(snapshot)
    }
}

/// Stream validation over source records, including roads outside existing terrain.
/// No route-wide control arrays or world-wide spatial index are constructed.
pub(crate) fn validate_cook_source(
    c: &Connection,
    library: &PresetLibrary,
) -> Result<(), WorldDbError> {
    transaction::validate_shared(c, library)?;
    for (table, kind) in [
        ("road_profiles", 0),
        ("roads", 1),
        ("road_knots", 2),
        ("road_spans", 3),
        ("road_junctions", 4),
    ] {
        let mut q = c.prepare(&format!("SELECT id FROM {table} ORDER BY id"))?;
        let mut rows = q.query([])?;
        while let Some(row) = rows.next()? {
            let id = crate::blob_array(
                row.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?,
                "road id",
            )?;
            let key = match kind {
                0 => RoadRecordKey::Profile(RoadProfileId(id)),
                1 => RoadRecordKey::Road(RoadId(id)),
                2 => RoadRecordKey::Knot(RoadKnotId(id)),
                3 => RoadRecordKey::Span(RoadSpanId(id)),
                _ => RoadRecordKey::Junction(RoadJunctionId(id)),
            };
            let record = required(c, key)?;
            transaction::validate_record(c, &record)?;
            match record {
                RoadSourceRecord::Span(source) => {
                    let span = span(c, source.id)?;
                    let route = route(c, span.road)?;
                    let size = definition(c, route.space)?.cell_size;
                    let bounds = index_bounds(&span, size)?;
                    RoadSnapshot {
                        space: route.space,
                        cell_size: size,
                        loaded_bounds: bounds,
                        truncated: false,
                        junctions: vec![],
                        profiles: vec![profile(c, route.road.profile)?],
                        roads: vec![route.road],
                        spans: vec![span],
                    }
                    .validate()
                    .map_err(|e| invalid(e.to_string()))?;
                    validate_cook_index(c, "road_span_cells", "span_id", id, route.space, bounds)?;
                    let stored = c.query_row(
                        "SELECT min_x,min_z,max_x,max_z FROM road_spans WHERE id=?1",
                        [id.as_slice()],
                        |r| {
                            Ok(RoadCellBounds {
                                minimum: CellCoord {
                                    x: r.get(0)?,
                                    z: r.get(1)?,
                                },
                                maximum: CellCoord {
                                    x: r.get(2)?,
                                    z: r.get(3)?,
                                },
                            })
                        },
                    )?;
                    if stored != bounds {
                        return Err(invalid("road span bounds index disagrees with source"));
                    }
                }
                RoadSourceRecord::Junction(source) => {
                    let bounds = source
                        .junction
                        .bounds(definition(c, source.space)?.cell_size)
                        .map_err(|e| invalid(e.to_string()))?;
                    validate_cook_index(
                        c,
                        "road_junction_cells",
                        "junction_id",
                        id,
                        source.space,
                        bounds,
                    )?;
                    let count: i64 = c.query_row(
                        "SELECT count(*) FROM road_junction_knots WHERE junction_id=?1",
                        [id.as_slice()],
                        |r| r.get(0),
                    )?;
                    if count != source.junction.knots.len() as i64 {
                        return Err(invalid("junction membership count disagrees with source"));
                    }
                    for knot in source.junction.knots {
                        if junctions::for_knot(c, knot)? != Some(source.junction.id) {
                            return Err(invalid("junction membership disagrees with source"));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
fn validate_cook_index(
    c: &Connection,
    table: &str,
    id_column: &str,
    id: [u8; 16],
    space: WorldSpaceId,
    bounds: RoadCellBounds,
) -> Result<(), WorldDbError> {
    let expected = bounds
        .cell_count()
        .filter(|&n| n <= MAX_ROAD_INDEX_CELLS_PER_SPAN)
        .ok_or_else(|| invalid("road index budget"))?;
    let (total,matching):(i64,i64)=c.query_row(&format!("SELECT count(*),coalesce(sum(world_space_id=?2 AND cell_x BETWEEN ?3 AND ?4 AND cell_z BETWEEN ?5 AND ?6),0) FROM {table} WHERE {id_column}=?1"),params![id.as_slice(),space.0,bounds.minimum.x,bounds.maximum.x,bounds.minimum.z,bounds.maximum.z],|r|Ok((r.get(0)?,r.get(1)?)))?;
    if total != expected as i64 || matching != total {
        return Err(invalid("road spatial index disagrees with source coverage"));
    }
    Ok(())
}
