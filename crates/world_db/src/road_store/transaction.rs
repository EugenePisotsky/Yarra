use super::*;
fn same_owner(a: &RoadSourceRecord, b: &RoadSourceRecord) -> bool {
    match (a, b) {
        (RoadSourceRecord::Road(a), RoadSourceRecord::Road(b)) => a.space == b.space,
        (RoadSourceRecord::Knot(a), RoadSourceRecord::Knot(b)) => a.road == b.road,
        (RoadSourceRecord::Span(a), RoadSourceRecord::Span(b)) => a.road == b.road,
        (RoadSourceRecord::Profile(_), RoadSourceRecord::Profile(_)) => true,
        (RoadSourceRecord::Junction(a), RoadSourceRecord::Junction(b)) => a.space == b.space,
        _ => false,
    }
}
impl ProjectWriter {
    /// One gesture: CAS all writes/dependencies, validate the final graph, update incident indexes,
    /// and publish old/new bounds together. No partial writes escape a conflict or validation error.
    pub fn apply_road_source_transaction(
        &mut self,
        expected_library_revision: u64,
        writes: &[RoadSourceWrite],
        dependencies: &[RoadDependency],
    ) -> Result<RoadSourceWriteResult, WorldDbError> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = apply_transaction(&tx, expected_library_revision, writes, dependencies)?;
        if matches!(result, RoadSourceWriteResult::Committed(_)) {
            tx.commit()?;
        }
        Ok(result)
    }
}
/// Caller owns the encompassing transaction, including rollback on a conflict.
pub(crate) fn apply_transaction(
    tx: &Connection,
    expected_library_revision: u64,
    writes: &[RoadSourceWrite],
    dependencies: &[RoadDependency],
) -> Result<RoadSourceWriteResult, WorldDbError> {
    if writes.is_empty()
        || writes.len() > MAX_ROAD_WRITES
        || dependencies.len() > MAX_ROAD_DEPENDENCIES
    {
        return Err(invalid("transaction record budget"));
    }
    let mut replacements = BTreeMap::new();
    let mut expected = BTreeMap::new();
    for w in writes {
        if replacements.insert(w.key, w).is_some()
            || w.record.as_ref().is_some_and(|r| r.key() != w.key)
        {
            return Err(invalid("duplicate or mismatched write identity"));
        }
        expected.insert(w.key, w.expected_revision);
    }
    let mut unique = BTreeSet::new();
    for d in dependencies {
        if d.revision == 0 || !unique.insert(d.key) {
            return Err(invalid("duplicate or invalid dependency"));
        }
        if let Some(old) = expected.insert(d.key, Some(d.revision))
            && old != Some(d.revision)
        {
            return Err(invalid("write/dependency revision disagreement"));
        }
    }
    let library = read_library(tx)?;
    if library.revision != expected_library_revision {
        return Ok(RoadSourceWriteResult::LibraryConflict {
            actual_revision: library.revision,
        });
    }
    let mut actual = BTreeMap::new();
    for (&key, &revision) in &expected {
        let state = read_state(tx, key)?;
        if state.revision != revision {
            return Ok(RoadSourceWriteResult::Conflict(state));
        }
        actual.insert(key, state);
    }
    // Every unchanged record used to interpret new geometry must be covered by the caller's CAS.
    let mut queue = writes
        .iter()
        .filter_map(|w| w.record.as_ref())
        .flat_map(RoadSourceRecord::references)
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some(key) = queue.pop() {
        if !visited.insert(key) {
            continue;
        }
        let record = if let Some(w) = replacements.get(&key) {
            w.record.as_ref()
        } else {
            actual
                .get(&key)
                .ok_or_else(|| invalid(format!("missing dependency revision for {key:?}")))?
                .record
                .as_ref()
        }
        .ok_or_else(|| invalid("replacement references a deleted record"))?;
        queue.extend(record.references());
    }
    let mut junction_ids = BTreeSet::new();
    for w in writes {
        junction_ids.extend(junctions::touching(tx, w.key)?);
        if let Some(record) = &w.record {
            for key in record.references() {
                if let RoadRecordKey::Knot(id) = key
                    && let Some(junction) = junctions::for_knot(tx, id)?
                {
                    junction_ids.insert(junction);
                }
            }
        }
    }
    for id in &junction_ids {
        let key = RoadRecordKey::Junction(*id);
        if !replacements.contains_key(&key) && !actual.contains_key(&key) {
            return Err(invalid("missing junction dependency revision"));
        }
    }
    let mut junction_bounds = vec![];
    for id in &junction_ids {
        if let Some(RoadSourceRecord::Junction(j)) =
            read_state(tx, RoadRecordKey::Junction(*id))?.record
        {
            junction_bounds.push(RoadAffectedBounds {
                space: j.space,
                bounds: j
                    .junction
                    .bounds(definition(tx, j.space)?.cell_size)
                    .map_err(|e| invalid(e.to_string()))?,
            });
        }
    }
    let mut affected = BTreeSet::new();
    for w in writes {
        if let RoadRecordKey::Span(id) = w.key {
            affected.insert(id);
        }
        if let RoadRecordKey::Knot(id) = w.key {
            affected.extend(adjacent(tx, id)?);
        }
    }
    let mut bounds = affected
        .iter()
        .map(|&id| saved_bounds(tx, id))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let old_index_rows = bounds
        .iter()
        .chain(junction_bounds.iter())
        .try_fold(0usize, |total, b| {
            b.bounds.cell_count().and_then(|n| total.checked_add(n))
        })
        .ok_or_else(|| invalid("invalid saved road index bounds"))?;
    let mut remaining_index_writes = MAX_ROAD_INDEX_WRITES_PER_TRANSACTION
        .checked_sub(old_index_rows)
        .ok_or_else(|| invalid("gesture exceeds the road spatial-index write budget"))?;
    let mut committed = vec![];
    let mut selectors = vec![];
    for w in writes {
        let old = &actual[&w.key];
        if w.record.is_none() && old.record.is_none() {
            return Err(invalid("cannot delete a missing record"));
        }
        let revision = next_revision(old.revision.unwrap_or(0))?;
        let mut record = w.record.clone();
        if let Some(r) = &mut record {
            if let Some(old) = &old.record
                && !same_owner(old, r)
            {
                return Err(invalid(
                    "record ownership is immutable; create a new identity",
                ));
            }
            r.set_revision(revision);
        }
        match w.key {
            RoadRecordKey::Profile(id) => selectors.push(RoadDependencySelector::Profile(id)),
            RoadRecordKey::Road(id) => selectors.push(RoadDependencySelector::Road(id)),
            _ => {}
        }
        committed.push(RoadRecordState {
            key: w.key,
            revision: Some(revision),
            record,
        });
    }
    // Replacing all edited spans first avoids transient UNIQUE conflicts when splitting/relinking.
    for state in &committed {
        if let RoadRecordKey::Span(id) = state.key {
            tx.execute("DELETE FROM road_spans WHERE id=?1", [id.0.as_slice()])?;
        }
    }
    for state in &committed {
        if let RoadRecordKey::Junction(id) = state.key {
            tx.execute(
                "DELETE FROM road_junction_knots WHERE junction_id=?1",
                [id.0.as_slice()],
            )?;
        }
    }
    for state in &committed {
        if let Some(record) = &state.record {
            if let RoadSourceRecord::Junction(j) = record {
                let count = j
                    .junction
                    .bounds(definition(tx, j.space)?.cell_size)
                    .map_err(|e| invalid(e.to_string()))?
                    .cell_count()
                    .ok_or_else(|| invalid("junction bounds"))?;
                remaining_index_writes =
                    remaining_index_writes.checked_sub(count).ok_or_else(|| {
                        invalid("gesture exceeds the road spatial-index write budget")
                    })?;
            }
            store(tx, record)?;
        } else {
            delete(tx, state.key, state.revision.unwrap())?;
        }
    }
    validate_shared(tx, &library)?;
    for state in &committed {
        if let Some(record) = &state.record {
            validate_record(tx, record)?;
        }
    }
    for state in &committed {
        if let RoadRecordKey::Knot(id) = state.key {
            affected.extend(adjacent(tx, id)?);
        }
    }
    for id in affected {
        if read_state(tx, RoadRecordKey::Span(id))?.record.is_some() {
            bounds.push(reindex(tx, id, Some(&mut remaining_index_writes))?);
        }
    }
    for w in writes {
        junction_ids.extend(junctions::touching(tx, w.key)?);
    }
    for id in junction_ids {
        if let Some(RoadSourceRecord::Junction(j)) =
            read_state(tx, RoadRecordKey::Junction(id))?.record
        {
            junctions::validate(tx, &j)?;
            junction_bounds.push(RoadAffectedBounds {
                space: j.space,
                bounds: j
                    .junction
                    .bounds(definition(tx, j.space)?.cell_size)
                    .map_err(|e| invalid(e.to_string()))?,
            });
        }
    }
    bounds.extend(junction_bounds);
    let revision = next_revision(epoch(tx)?)?;
    tx.execute(
        "UPDATE road_state SET revision=?1 WHERE singleton=1",
        [revision as i64],
    )?;
    Ok(RoadSourceWriteResult::Committed(RoadSourceCommit {
        revision,
        records: committed,
        bounds,
        dependencies: selectors,
    }))
}

pub(super) fn validate_record(
    c: &Connection,
    record: &RoadSourceRecord,
) -> Result<(), WorldDbError> {
    match record {
        RoadSourceRecord::Junction(j) => junctions::validate(c, j)?,
        RoadSourceRecord::Profile(p) => p.validate().map_err(|e| invalid(e.to_string()))?,
        RoadSourceRecord::Road(r) => {
            let mismatch:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM road_junction_knots m JOIN road_knots k ON k.id=m.knot_id JOIN road_junctions j ON j.id=m.junction_id WHERE k.road_id=?1 AND (j.profile_id!=?2 OR j.world_space_id!=?3))",params![r.road.id.0.as_slice(),r.road.profile.0.as_slice(),r.space.0],|row|row.get(0))?;
            if mismatch {
                return Err(invalid(
                    "disconnect a road from its junctions before changing its style",
                ));
            }
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
                roads: vec![r.road.clone()],
                profiles: vec![p],
                spans: vec![],
            }
            .validate()
            .map_err(|e| invalid(e.to_string()))?;
        }
        RoadSourceRecord::Knot(k) => {
            let r = route(c, k.road)?;
            let p = profile(c, r.road.profile)?;
            let d = definition(c, r.space)?;
            k.knot
                .validate(d.cell_size, &p)
                .map_err(|e| invalid(e.to_string()))?;
        }
        RoadSourceRecord::Span(_) => {} // Endpoint consistency and bounds checked by reindex; topology by FK/UNIQUE.
    }
    Ok(())
}
/// Validate shared references against the final library/world definitions, including offscreen routes.
/// Reads at most 64 profile payloads and 32 world definitions; never deserializes all road controls.
pub(crate) fn validate_shared(c: &Connection, library: &PresetLibrary) -> Result<(), WorldDbError> {
    let mut q = c.prepare("SELECT id FROM road_profiles ORDER BY id LIMIT 65")?;
    let ids = q
        .query_map([], |r| {
            Ok(RoadProfileId(crate::blob_array(
                r.get_ref(0)?.as_blob()?,
                "profile id",
            )?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > MAX_ROAD_RECORDS {
        return Err(invalid("project road profile limit is 64"));
    }
    for id in ids {
        let p = profile(c, id)?;
        p.validate().map_err(|e| invalid(e.to_string()))?;
        let Some(preset) = library.get(p.ground) else {
            return Err(invalid("missing road Ground preset"));
        };
        let PresetKind::Ground(g) = &preset.kind else {
            return Err(invalid("road surface preset must remain Ground"));
        };
        let undersized:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM roads r JOIN road_knots k ON k.road_id=r.id WHERE r.profile_id=?1 AND k.width<?2)",params![id.0.as_slice(),p.minimum_width()],|r|r.get(0))?;
        if undersized {
            return Err(invalid(
                "profile wheel/edge widths exceed a dependent road corridor",
            ));
        }
        let mut query=c.prepare("SELECT DISTINCT world_space_id FROM roads WHERE profile_id=?1 ORDER BY world_space_id LIMIT 33")?;
        let worlds = query
            .query_map([id.0.as_slice()], |r| Ok(WorldSpaceId(r.get(0)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        if worlds.len() > crate::MAX_ENVIRONMENT_DEFINITIONS {
            return Err(invalid("road world limit"));
        }
        for space in worlds {
            let d = definition(c, space)?;
            if g.surfaces.iter().any(|s| !d.surfaces.contains(&s.surface)) {
                return Err(invalid(
                    "road Ground preset uses a surface absent from a dependent world",
                ));
            }
        }
    }
    Ok(())
}
