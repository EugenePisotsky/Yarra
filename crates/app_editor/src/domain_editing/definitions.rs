//! Small environment catalogs share history, save intent and recovery with spatial masks.
use super::*;
use environment::EnvironmentDefinition;
use serde::{Deserialize, Serialize};
use vegetation::VegetationCatalog;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DirtyDefinitionSnapshot {
    pub(crate) base: EnvironmentDefinition,
    pub(crate) current: EnvironmentDefinition,
}
#[derive(Debug, Clone)]
pub(super) struct DefinitionEntry<T = EnvironmentDefinition> {
    pub(super) base: T,
    pub(super) current: T,
    pub(super) state: DefinitionSaveState<T>,
}
#[derive(Debug, Clone)]
pub(super) enum DefinitionSaveState<T = EnvironmentDefinition> {
    Idle,
    Saving(u64),
    Conflict(Option<T>),
    Failed(String),
}
impl<T: PartialEq> DefinitionEntry<T> {
    pub(super) fn dirty(&self) -> bool {
        self.base != self.current
    }
}
impl DenseDomainWorkingSets {
    pub(crate) fn definition(&self, space: WorldSpaceId) -> Option<&EnvironmentDefinition> {
        self.definitions.get(&space).map(|e| &e.current)
    }
    pub(crate) fn definitions(&self) -> impl Iterator<Item = &EnvironmentDefinition> {
        self.definitions.values().map(|e| &e.current)
    }
    pub(crate) fn definition_edit_revision(&self) -> u64 {
        self.definition_revision
    }
    pub(crate) fn definition_dirty_count(&self) -> usize {
        self.definitions.values().filter(|e| e.dirty()).count()
            + usize::from(self.presets.as_ref().is_some_and(|p| p.dirty()))
    }
    pub(super) fn definitions_saving(&self) -> bool {
        self.presets
            .as_ref()
            .is_some_and(|p| matches!(p.state, DefinitionSaveState::Saving(_)))
            || self
                .definitions
                .values()
                .any(|e| matches!(e.state, DefinitionSaveState::Saving(_)))
    }
    pub(super) fn definition_conflict_count(&self) -> usize {
        usize::from(
            self.presets
                .as_ref()
                .is_some_and(|p| matches!(p.state, DefinitionSaveState::Conflict(_))),
        ) + self
            .definitions
            .values()
            .filter(|e| matches!(e.state, DefinitionSaveState::Conflict(_)))
            .count()
    }
    pub(crate) fn definition_status(&self) -> Option<String> {
        if let Some(status) = self.preset_status() {
            return Some(status);
        }
        self.definitions.values().find_map(|e| match &e.state {
            DefinitionSaveState::Failed(error) => Some(format!("Environment save failed: {error}")),
            DefinitionSaveState::Conflict(_) => Some("The layer definitions changed in the database. Resolve the conflict before continuing.".into()),
            _ => None,
        })
    }
    pub(crate) fn dirty_definition_snapshots(&self) -> Vec<DirtyDefinitionSnapshot> {
        self.definitions
            .values()
            .filter(|e| e.dirty())
            .map(|e| DirtyDefinitionSnapshot {
                base: e.base.clone(),
                current: e.current.clone(),
            })
            .collect()
    }
    pub(crate) fn restore_definition_snapshot(
        &mut self,
        snapshot: DirtyDefinitionSnapshot,
        plants: &VegetationCatalog,
    ) -> bool {
        if snapshot.base.space != snapshot.current.space
            || snapshot.base.cell_size != snapshot.current.cell_size
            || snapshot.base.mask_resolution != snapshot.current.mask_resolution
            || snapshot.base == snapshot.current
            || self
                .presets()
                .is_none_or(|p| validate(&snapshot.current, plants, p).is_err())
        {
            return false;
        }
        self.definitions.insert(
            snapshot.current.space,
            DefinitionEntry {
                base: snapshot.base,
                current: snapshot.current,
                state: DefinitionSaveState::Idle,
            },
        );
        self.plants = Some(plants.clone());
        self.bump_definitions();
        true
    }
    pub(super) fn reconcile_definitions(
        &mut self,
        project: &ProjectEditorStore,
        plants: Option<&VegetationCatalog>,
    ) {
        if let Some(plants) = plants
            && self.plants.as_ref() != Some(plants)
        {
            self.plants = Some(plants.clone());
        }
        self.reconcile_presets(project);
        let mut changed = false;
        for definition in project.environments() {
            match self.definitions.get_mut(&definition.space) {
                Some(entry)
                    if matches!(entry.state, DefinitionSaveState::Idle)
                        && entry.base != *definition =>
                {
                    let pinned = self.entries.iter().any(|(key, cell)| {
                        let DenseSourceRecordKey::EnvironmentCoverage { space, .. } = key;
                        *space == definition.space && cell.pinned()
                    });
                    if entry.dirty() || pinned {
                        entry.state = DefinitionSaveState::Conflict(Some(definition.clone()));
                    } else {
                        entry.base = definition.clone();
                        entry.current = definition.clone();
                    }
                    changed = true;
                }
                None => {
                    self.definitions.insert(
                        definition.space,
                        DefinitionEntry {
                            base: definition.clone(),
                            current: definition.clone(),
                            state: DefinitionSaveState::Idle,
                        },
                    );
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.bump_definitions();
        }
    }
    pub(crate) fn apply_environment(
        &mut self,
        replacement: &EnvironmentDefinition,
        library: &environment::PresetLibrary,
    ) -> Result<(), String> {
        if self.presets.is_none() {
            return Err("Presets are still loading".into());
        }
        if self.saving() || self.has_any_conflict() || self.gesture_active {
            return Err("Finish the stroke or save before editing layers".into());
        }
        let entry = self
            .definitions
            .get(&replacement.space)
            .ok_or("Environment is still loading")?;
        if entry.current.cell_size != replacement.cell_size
            || entry.current.mask_resolution != replacement.mask_resolution
            || entry.current.surfaces != replacement.surfaces
        {
            return Err("Layer editing cannot change the world grid or terrain pack".into());
        }
        let plants = self
            .plants
            .as_ref()
            .ok_or("Vegetation assets are still loading")?;
        self.validate_preset_edit(library, Some(replacement))?;
        validate(replacement, plants, library)?;
        if !self.coverage_fits_definition(replacement) {
            return Err("Undo or erase this layer's local paint before removing the layer".into());
        }
        let mut current = replacement.clone();
        normalize_revisions(&mut current, &entry.base);
        let dirty_bytes = self
            .definitions
            .iter()
            .filter(|(space, e)| **space != replacement.space && e.dirty())
            .map(|(_, e)| definition_bytes(&e.current))
            .sum::<usize>()
            + if current != entry.base {
                definition_bytes(&current)
            } else {
                0
            };
        if dirty_bytes > 4 * 1024 * 1024 {
            return Err(
                "Save the current layer edits before editing more definitions (4 MiB limit)".into(),
            );
        }
        self.install_preset_edit(library);
        let entry = self.definitions.get_mut(&replacement.space).unwrap();
        if current != entry.current {
            entry.current = current;
            entry.state = DefinitionSaveState::Idle;
            self.bump_definitions();
        }
        Ok(())
    }
    pub(super) fn definition_writes(&self) -> Vec<world_db::EnvironmentDefinitionWrite> {
        self.definitions
            .values()
            .filter(|e| e.dirty())
            .map(|e| world_db::EnvironmentDefinitionWrite {
                expected_revision: Some(e.base.revision),
                definition: e.current.clone(),
            })
            .collect()
    }
    pub(super) fn mark_definitions_saving(&mut self, request: u64) {
        if let Some(p) = &mut self.presets
            && p.dirty()
        {
            p.state = DefinitionSaveState::Saving(request);
        }
        for entry in self.definitions.values_mut().filter(|e| e.dirty()) {
            entry.state = DefinitionSaveState::Saving(request);
        }
    }
    pub(super) fn finish_definitions(&mut self, request: u64, outcome: &DenseSaveOutcome) {
        self.finish_presets(request, outcome);
        for entry in self.definitions.values_mut() {
            if matches!(entry.state,DefinitionSaveState::Saving(id) if id==request) {
                entry.state = match outcome {
                    DenseSaveOutcome::Failed(error) => DefinitionSaveState::Failed(error.clone()),
                    _ => DefinitionSaveState::Idle,
                };
            }
        }
        match outcome {
            DenseSaveOutcome::Committed(commit) => {
                for definition in &commit.definitions {
                    if let Some(entry) = self.definitions.get_mut(&definition.space) {
                        entry.base = definition.clone();
                        entry.current = definition.clone();
                        entry.state = DefinitionSaveState::Idle;
                    }
                    self.retarget_definition(definition.space, definition.revision);
                }
                if !commit.definitions.is_empty() {
                    self.bump_definitions();
                }
            }
            DenseSaveOutcome::DefinitionConflict { space, actual } => {
                if let Some(entry) = self.definitions.get_mut(space) {
                    entry.state = DefinitionSaveState::Conflict(actual.clone());
                }
            }
            _ => {}
        }
    }
    pub(super) fn can_keep_definition_conflicts(&self) -> bool {
        self.can_resolve_presets(true) || self.definitions.values().any(|e|matches!(&e.state,DefinitionSaveState::Conflict(Some(actual)) if same_grid(actual,&e.current)))
    }
    pub(super) fn can_accept_definition_conflicts(&self) -> bool {
        self.can_resolve_presets(false) || self.definitions.values().any(|e|matches!(&e.state,DefinitionSaveState::Conflict(Some(actual)) if same_grid(actual,&e.current) && self.coverage_fits_definition(actual) && self.presets().is_some_and(|p|actual.validate_layers(p).is_ok())))
    }
    pub(super) fn resolve_definition_conflicts(&mut self, keep_local: bool) -> usize {
        let library_resolved = self.resolve_preset_conflict(keep_local);
        let spaces = self
            .definitions
            .iter()
            .filter_map(|(space, e)| match &e.state {
                DefinitionSaveState::Conflict(Some(actual))
                    if same_grid(actual, &e.current)
                        && (keep_local
                            || (self.coverage_fits_definition(actual)
                                && self
                                    .presets()
                                    .is_some_and(|p| actual.validate_layers(p).is_ok()))) =>
                {
                    Some(*space)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for space in &spaces {
            let entry = self.definitions.get_mut(space).unwrap();
            let DefinitionSaveState::Conflict(Some(actual)) = &entry.state else {
                unreachable!()
            };
            let actual = actual.clone();
            if keep_local {
                normalize_revisions(&mut entry.current, &actual);
            } else {
                entry.current = actual.clone();
            }
            entry.base = actual.clone();
            entry.state = DefinitionSaveState::Idle;
            self.retarget_definition(*space, actual.revision);
        }
        if !spaces.is_empty() {
            self.bump_definitions();
        }
        spaces.len() + library_resolved
    }
    fn retarget_definition(&mut self, space: WorldSpaceId, revision: u64) {
        for entry in self.entries.values_mut() {
            for record in entry
                .base
                .iter_mut()
                .chain(std::iter::once(&mut entry.current))
                .chain(entry.runtime.iter_mut())
            {
                let DenseSourceRecord::EnvironmentCoverage(record) = record;
                if record.space == space {
                    record.definition_revision = revision;
                }
            }
        }
    }
    fn coverage_fits_definition(&self, definition: &EnvironmentDefinition) -> bool {
        self.entries.values().all(|e| {
            let DenseSourceRecord::EnvironmentCoverage(record) = &e.current;
            record.space != definition.space
                || record.tiles.iter().all(|t| {
                    definition.layers.iter().any(|l| l.id == t.layer)
                        && t.samples.len() == usize::from(definition.mask_resolution).pow(2)
                })
        })
    }
    pub(super) fn bump_definitions(&mut self) {
        self.definition_revision = self.definition_revision.wrapping_add(1).max(1);
        self.bump_revision();
    }
}
fn same_grid(a: &EnvironmentDefinition, b: &EnvironmentDefinition) -> bool {
    a.cell_size == b.cell_size && a.mask_resolution == b.mask_resolution && a.surfaces == b.surfaces
}
fn validate(
    definition: &EnvironmentDefinition,
    plants: &VegetationCatalog,
    presets: &environment::PresetLibrary,
) -> Result<(), String> {
    if definition.layers.iter().any(|l| l.name.len() > 256) {
        return Err("Names must fit in 256 bytes".into());
    }
    environment_compile::CompilePlan::new(definition, plants, presets, Default::default())
        .map_err(|e| e.to_string())?;
    if definition_bytes(definition) > 1024 * 1024 {
        return Err("Environment definition exceeds the size budget".into());
    }
    Ok(())
}
pub(crate) fn definition_bytes(definition: &EnvironmentDefinition) -> usize {
    ron::ser::to_string(definition).map_or(usize::MAX, |s| s.len())
}
fn normalize_revisions(definition: &mut EnvironmentDefinition, base: &EnvironmentDefinition) {
    definition.revision = base.revision;
    for layer in &mut definition.layers {
        layer.revision = base
            .layers
            .iter()
            .find(|l| l.id == layer.id)
            .map_or(1, |l| l.revision);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::{EditorHistory, EditorObjectWorkingSet};
    use world_db::{EnvironmentSourceWriteResult, ProjectReader, ProjectWriter};

    fn save(dense: &mut DenseDomainWorkingSets, writer: &mut ProjectWriter) {
        let definitions = dense.definition_writes();
        let writes = dense
            .dirty_snapshots()
            .into_iter()
            .map(|s| {
                let DenseSourceRecord::EnvironmentCoverage(record) = s.current;
                let expected_source_revision = s.base.map(|r| dense_source_revision(&r));
                DenseSourceWrite::EnvironmentCoverage {
                    expected_source_revision,
                    record,
                }
            })
            .collect::<Vec<_>>();
        dense.mark_definitions_saving(1);
        for entry in dense.entries.values_mut().filter(|e| e.dirty()) {
            entry.save_state = DenseSaveState::Saving(1);
        }
        let EnvironmentSourceWriteResult::Committed(commit) = writer
            .apply_environment_source_transaction(
                dense.preset_base_revision(),
                dense.preset_write().as_ref(),
                &definitions,
                &writes,
            )
            .unwrap()
        else {
            panic!("expected a commit")
        };
        dense.finish_save(1, DenseSaveOutcome::Committed(commit));
        assert_eq!(dense.dirty_count(), 0);
    }

    #[test]
    fn layer_creation_and_paint_undo_redo_survive_multiple_atomic_saves() {
        let dir =
            std::env::temp_dir().join(format!("yarra-layer-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("project.sqlite");
        world_cook::create_demo_project(&path).unwrap();
        let reader = ProjectReader::open_read_only(&path).unwrap();
        let definition = reader
            .read_environment_definitions()
            .unwrap()
            .into_iter()
            .find(|d| !d.layers.is_empty())
            .unwrap();
        let halo = world_db::environment_dependency_cells(&[CellCoord::ZERO]).unwrap();
        let snapshot = reader
            .read_environment_snapshot(definition.space, &halo)
            .unwrap();
        let records = snapshot
            .coverage
            .cells
            .iter()
            .map(|c| world_db::SourceEnvironmentCellRecord {
                space: definition.space,
                cell: c.cell,
                definition_revision: definition.revision,
                source_revision: c.revision as i64,
                tiles: c.tiles.clone(),
            })
            .collect::<Vec<_>>();
        let mut dense = DenseDomainWorkingSets::from_environment_records(&records);
        dense.initialize_presets(&snapshot.presets);
        dense.plants = Some(snapshot.vegetation_catalog);
        dense.definitions.insert(
            definition.space,
            DefinitionEntry {
                base: definition.clone(),
                current: definition.clone(),
                state: DefinitionSaveState::Idle,
            },
        );
        let mut added = definition.clone();
        let mut layer = added.layers[0].clone();
        layer.id = environment::LayerId([91; 16]);
        layer.order = 50;
        let mut library = snapshot.presets.clone();
        let mut preset = library.get(layer.preset).unwrap().clone();
        preset.id = environment::PresetId([91; 16]);
        preset.name = "New shared meadow".into();
        layer.preset = preset.id;
        library.presets.push(preset);
        added.layers.push(layer.clone());
        let mut history = EditorHistory::default();
        let mut objects = EditorObjectWorkingSet::default();
        history
            .edit_environment(&mut dense, &added, &library)
            .unwrap();
        let before = dense
            .environment_record(definition.space, CellCoord::ZERO)
            .unwrap()
            .clone();
        let mut painted = before.clone();
        let mut samples = vec![0; usize::from(definition.mask_resolution).pow(2)];
        let center = samples.len() / 2;
        samples[center] = 255;
        painted.tiles.push(environment::CoverageTile {
            layer: layer.id,
            samples,
        });
        dense
            .apply_environment_records(std::slice::from_ref(&painted))
            .unwrap();
        history.record_environment_stroke(vec![before.clone()], vec![painted]);
        let mut writer = ProjectWriter::open(&path).unwrap();
        save(&mut dense, &mut writer);
        let first_saved = dense.definition(definition.space).unwrap().revision;
        assert!(first_saved > definition.revision);
        assert_eq!(dense.presets().unwrap().revision, 2);
        assert!(dense.presets().unwrap().get(layer.preset).is_some());
        assert!(history.undo(&mut objects, &mut dense));
        assert_eq!(
            dense
                .environment_record(definition.space, CellCoord::ZERO)
                .unwrap()
                .tiles,
            before.tiles
        );
        assert!(history.undo(&mut objects, &mut dense));
        assert_eq!(
            dense.definition(definition.space).unwrap().layers.len(),
            definition.layers.len()
        );
        assert!(dense.presets().unwrap().get(layer.preset).is_none());
        save(&mut dense, &mut writer);
        assert!(history.redo(&mut objects, &mut dense));
        assert!(history.redo(&mut objects, &mut dense));
        let revision = dense.definition(definition.space).unwrap().revision;
        assert!(revision > first_saved);
        assert_eq!(
            dense
                .environment_record(definition.space, CellCoord::ZERO)
                .unwrap()
                .definition_revision,
            revision
        );
        save(&mut dense, &mut writer);
        let reloaded = reader
            .read_environment_snapshot(definition.space, &halo)
            .unwrap();
        assert_eq!(
            &reloaded.definition,
            dense.definition(definition.space).unwrap()
        );
        assert!(
            reloaded
                .coverage
                .cells
                .iter()
                .any(|c| c.tiles.iter().any(|t| t.layer == layer.id))
        );
        assert_eq!(&reloaded.presets, dense.presets().unwrap());
        let plan = environment_compile::CompilePlan::new(
            &reloaded.definition,
            dense.plants.as_ref().unwrap(),
            &reloaded.presets,
            Default::default(),
        )
        .unwrap();
        let live = plan
            .compile_cells(&[CellCoord::ZERO], &reloaded.coverage)
            .unwrap();
        let runtime_path = dir.join("runtime.sqlite");
        world_cook::cook_project(&path, &runtime_path).unwrap();
        let document = world_db::read_project_database(&path).unwrap();
        let cooked = world_cook::build_runtime(document).unwrap();
        let page = cooked
            .pages
            .iter()
            .find(|p| {
                p.key.space == definition.space
                    && p.key.cell == CellCoord::ZERO
                    && p.key.domain == world::PageDomain::Vegetation
            })
            .unwrap()
            .clone()
            .decode()
            .unwrap();
        let world::PagePayload::Vegetation(vegetation) = page.payload else {
            panic!()
        };
        assert_eq!(live[0].vegetation, vegetation);
        // Definition-only saves are also real transactions and remain undoable.
        let mut renamed = reloaded.definition;
        renamed.layers.last_mut().unwrap().name = "Saved name".into();
        history.edit_definition(&mut dense, &renamed).unwrap();
        assert_eq!(dense.dirty_snapshots().len(), 0);
        save(&mut dense, &mut writer);
        assert!(history.undo(&mut objects, &mut dense));
        assert_eq!(dense.definition_dirty_count(), 1);
        // A layer-only undo never restores an old snapshot of unrelated shared presets.
        assert!(history.redo(&mut objects, &mut dense));
        let mut external = dense.presets().unwrap().clone();
        external.presets[0].name = "External shared name".into();
        dense.install_preset_edit(&external);
        assert!(history.undo(&mut objects, &mut dense));
        assert_eq!(
            dense.presets().unwrap().presets[0].name,
            "External shared name"
        );
        drop(writer);
        drop(reader);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
