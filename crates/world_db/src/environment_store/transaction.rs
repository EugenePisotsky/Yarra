//! Definitions and masks form one source commit, including creating a layer and painting it,
//! or undoing its creation after clearing its last masks in the same transaction.
use super::*;

#[derive(Debug, Clone)]
pub struct EnvironmentDefinitionWrite {
    pub expected_revision: Option<u64>,
    pub definition: EnvironmentDefinition,
}
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentSourceCommit {
    pub roads: Option<crate::road_store::RoadSourceCommit>,
    pub presets: Option<PresetLibrary>,
    pub definitions: Vec<EnvironmentDefinition>,
    pub coverage: Vec<crate::DenseSourceRecord>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum EnvironmentSourceWriteResult {
    RoadConflict {
        actual: crate::road_store::RoadRecordState,
    },
    Committed(EnvironmentSourceCommit),
    LibraryConflict {
        actual: PresetLibrary,
    },
    DefinitionConflict {
        space: WorldSpaceId,
        actual: Option<EnvironmentDefinition>,
    },
    CoverageConflict {
        key: DenseSourceRecordKey,
        actual: Option<DenseSourceRecord>,
    },
}

impl ProjectWriter {
    pub fn replace_environment_definition_if_revision(
        &mut self,
        expected: Option<u64>,
        replacement: &EnvironmentDefinition,
    ) -> Result<EnvironmentDefinitionWriteResult, WorldDbError> {
        match self.apply_environment_source_transaction(
            read_library(&self.connection)?.revision,
            None,
            &[EnvironmentDefinitionWrite {
                expected_revision: expected,
                definition: replacement.clone(),
            }],
            &[],
        )? {
            EnvironmentSourceWriteResult::Committed(mut commit) => Ok(
                EnvironmentDefinitionWriteResult::Committed(commit.definitions.remove(0)),
            ),
            EnvironmentSourceWriteResult::DefinitionConflict { actual, .. } => {
                Ok(EnvironmentDefinitionWriteResult::Conflict { actual })
            }
            EnvironmentSourceWriteResult::RoadConflict { .. } => unreachable!("no road writes"),
            EnvironmentSourceWriteResult::LibraryConflict { .. } => {
                Err(invalid("preset library changed; retry the transaction"))
            }
            EnvironmentSourceWriteResult::CoverageConflict { .. } => {
                unreachable!("no coverage writes")
            }
        }
    }
    pub fn apply_dense_source_transaction(
        &mut self,
        writes: &[DenseSourceWrite],
    ) -> Result<DenseSourceWriteTransactionResult, WorldDbError> {
        match self.apply_environment_source_transaction(
            read_library(&self.connection)?.revision,
            None,
            &[],
            writes,
        )? {
            EnvironmentSourceWriteResult::Committed(commit) => Ok(
                DenseSourceWriteTransactionResult::Committed(commit.coverage),
            ),
            EnvironmentSourceWriteResult::CoverageConflict { key, actual } => {
                Ok(DenseSourceWriteTransactionResult::Conflict { key, actual })
            }
            EnvironmentSourceWriteResult::RoadConflict { .. } => unreachable!("no road writes"),
            EnvironmentSourceWriteResult::LibraryConflict { .. } => {
                Err(invalid("preset library changed; retry the transaction"))
            }
            EnvironmentSourceWriteResult::DefinitionConflict { .. } => {
                unreachable!("no definition writes")
            }
        }
    }
    pub fn apply_environment_source_transaction(
        &mut self,
        expected_library_revision: u64,
        replacement_library: Option<&PresetLibrary>,
        definitions: &[EnvironmentDefinitionWrite],
        writes: &[DenseSourceWrite],
    ) -> Result<EnvironmentSourceWriteResult, WorldDbError> {
        self.apply_environment_and_roads_transaction(
            expected_library_revision,
            replacement_library,
            definitions,
            writes,
            &[],
            &[],
        )
    }
    pub fn apply_environment_and_roads_transaction(
        &mut self,
        expected_library_revision: u64,
        replacement_library: Option<&PresetLibrary>,
        definitions: &[EnvironmentDefinitionWrite],
        writes: &[DenseSourceWrite],
        roads: &[crate::road_store::RoadSourceWrite],
        dependencies: &[crate::road_store::RoadDependency],
    ) -> Result<EnvironmentSourceWriteResult, WorldDbError> {
        if (definitions.is_empty()
            && writes.is_empty()
            && replacement_library.is_none()
            && roads.is_empty())
            || definitions.len() > MAX_ENVIRONMENT_DEFINITIONS
            || definitions
                .iter()
                .map(|w| w.definition.space)
                .collect::<BTreeSet<_>>()
                .len()
                != definitions.len()
            || writes.len() > crate::MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION
            || writes
                .iter()
                .map(DenseSourceWrite::key)
                .collect::<std::collections::HashSet<_>>()
                .len()
                != writes.len()
        {
            return Err(WorldDbError::InvalidDenseSourceTransaction);
        }
        let mut bytes = 0usize;
        for write in writes {
            let DenseSourceWrite::EnvironmentCoverage {
                expected_source_revision,
                record,
            } = write;
            if expected_source_revision.is_some_and(|r| r < 0)
                || record.source_revision < 0
                || record.tiles.len() > 128
            {
                return Err(WorldDbError::InvalidDenseSourceRecord);
            }
            for tile in &record.tiles {
                bytes = bytes
                    .checked_add(tile.samples.len())
                    .ok_or_else(|| invalid("coverage size overflow"))?;
                if bytes > MAX_ENVIRONMENT_MASK_BYTES {
                    return Err(invalid("write exceeds the coverage byte budget"));
                }
            }
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let plants = read_vegetation_catalog(&tx)?.unwrap_or_else(|| VegetationCatalog {
            species: vec![],
            populations: vec![],
            assemblages: vec![],
        });
        let saved_library = read_library(&tx)?;
        if saved_library.revision != expected_library_revision {
            return Ok(EnvironmentSourceWriteResult::LibraryConflict {
                actual: saved_library,
            });
        }
        let changed_library = replacement_library
            .map(|next| revisioned_library(&saved_library, next))
            .transpose()?;
        let presets = changed_library.as_ref().unwrap_or(&saved_library);
        validate_library_references(&tx, presets, &plants)?;
        let mut previous = BTreeMap::new();
        let mut next = BTreeMap::new();
        let mut definition_bytes = 0;
        for write in definitions {
            let actual = read_definition(&tx, write.definition.space)?;
            if actual.as_ref().map(|d| d.revision) != write.expected_revision {
                return Ok(EnvironmentSourceWriteResult::DefinitionConflict {
                    space: write.definition.space,
                    actual,
                });
            }
            checked_definition(&write.definition, &plants, presets)?;
            validate_definition_references(&tx, &write.definition)?;
            definition_bytes +=
                bincode::serde::encode_to_vec(&write.definition, bincode::config::standard())?
                    .len();
            if definition_bytes > 4 * MAX_DEFINITION_BYTES {
                return Err(invalid("definition transaction exceeds the byte budget"));
            }
            let committed = revisioned_definition(actual.as_ref(), &write.definition)?;
            previous.insert(committed.space, actual);
            next.insert(committed.space, committed);
        }
        if read_definitions(&tx)?.len() + previous.values().filter(|d| d.is_none()).count()
            > MAX_ENVIRONMENT_DEFINITIONS
        {
            return Err(invalid("too many environment definitions"));
        }
        // A shared edit must leave all worlds valid, including layers outside the current viewport.
        for old in read_definitions(&tx)? {
            checked_definition(next.get(&old.space).unwrap_or(&old), &plants, presets)?;
        }
        let mut commits = Vec::new();
        let mut remaining = MAX_ENVIRONMENT_MASK_BYTES;
        for write in writes {
            let DenseSourceWrite::EnvironmentCoverage {
                expected_source_revision,
                record,
            } = write;
            let actual_definition = read_definition(&tx, record.space)?;
            let definition = next
                .get(&record.space)
                .or(actual_definition.as_ref())
                .ok_or_else(|| invalid("world has no environment definition"))?;
            let actual = if let Some(old) = &actual_definition {
                read_cell(&tx, old, record.cell, &mut remaining)?
            } else {
                None
            };
            if actual.as_ref().map(|r| r.source_revision) != *expected_source_revision
                || actual_definition.as_ref().map_or(0, |d| d.revision)
                    != record.definition_revision
            {
                return Ok(EnvironmentSourceWriteResult::CoverageConflict {
                    key: write.key(),
                    actual: actual.map(DenseSourceRecord::EnvironmentCoverage),
                });
            }
            let mut committed = record.clone();
            committed.definition_revision = definition.revision;
            validate_cell(definition, &committed)?;
            committed.source_revision =
                i64::try_from(next_revision(expected_source_revision.unwrap_or(0) as u64)?)
                    .map_err(|_| WorldDbError::IntegerOverflow)?;
            committed
                .tiles
                .retain(|t| t.samples.iter().any(|&v| v != 0));
            committed.tiles.sort_by_key(|t| t.layer);
            commits.push(committed);
        }
        for record in &commits {
            store_cell(&tx, record)?;
        }
        if changed_library.is_some() {
            store_library(&tx, presets)?;
        }
        for (space, definition) in &next {
            if let Some(old) = previous[space].as_ref() {
                validate_transition(&tx, old, definition)?;
            }
            store_definition(&tx, definition)?;
        }
        for space in commits.iter().map(|r| r.space).collect::<BTreeSet<_>>() {
            let definition =
                read_definition(&tx, space)?.ok_or_else(|| invalid("definition disappeared"))?;
            let cells = commits
                .iter()
                .filter(|r| r.space == space)
                .map(|r| r.cell)
                .collect::<Vec<_>>();
            let coverage = read_coverage(&tx, &definition, &environment_dependency_cells(&cells)?)?;
            CompilePlan::new(&definition, &plants, presets, CompileProfile::default())?
                .validate_coverage(&cells, &coverage)?;
        }
        let roads = if roads.is_empty() {
            None
        } else {
            match crate::road_store::apply_transaction(&tx, presets.revision, roads, dependencies)?
            {
                crate::road_store::RoadSourceWriteResult::Committed(commit) => Some(commit),
                crate::road_store::RoadSourceWriteResult::Conflict(actual) => {
                    return Ok(EnvironmentSourceWriteResult::RoadConflict { actual });
                }
                crate::road_store::RoadSourceWriteResult::LibraryConflict { .. } => {
                    unreachable!("library checked in the same transaction")
                }
            }
        };
        crate::road_store::validate_shared(&tx, presets)?;
        tx.commit()?;
        Ok(EnvironmentSourceWriteResult::Committed(
            EnvironmentSourceCommit {
                roads,
                presets: changed_library,
                definitions: next.into_values().collect(),
                coverage: commits
                    .into_iter()
                    .map(DenseSourceRecord::EnvironmentCoverage)
                    .collect(),
            },
        ))
    }
}
fn validate_transition(
    tx: &Transaction<'_>,
    old: &EnvironmentDefinition,
    next: &EnvironmentDefinition,
) -> Result<(), WorldDbError> {
    let has_masks: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM environment_coverage WHERE world_space_id = ?1)",
        [next.space.0],
        |row| row.get(0),
    )?;
    if has_masks && (old.cell_size != next.cell_size || old.mask_resolution != next.mask_resolution)
    {
        return Err(invalid("clear coverage before changing the source grid"));
    }
    for layer in old
        .layers
        .iter()
        .filter(|l| !next.layers.iter().any(|n| n.id == l.id))
    {
        let used:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM environment_coverage WHERE world_space_id = ?1 AND layer_id = ?2)",params![next.space.0,layer.id.0.as_slice()],|row|row.get(0))?;
        if used {
            return Err(invalid(
                "clear a layer's coverage before deleting its definition",
            ));
        }
    }
    Ok(())
}
fn revisioned_definition(
    old: Option<&EnvironmentDefinition>,
    replacement: &EnvironmentDefinition,
) -> Result<EnvironmentDefinition, WorldDbError> {
    let mut next = replacement.clone();
    next.revision = next_revision(old.map_or(0, |d| d.revision))?;
    for layer in &mut next.layers {
        layer.revision =
            if let Some(previous) = old.and_then(|d| d.layers.iter().find(|l| l.id == layer.id)) {
                layer.revision = previous.revision;
                if layer == previous {
                    previous.revision
                } else {
                    next_revision(previous.revision)?
                }
            } else {
                1
            };
    }

    Ok(next)
}
