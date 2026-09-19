//! Bounded environment source working sets.
//!
//! Dense records are keyed by domain/cell, keep database checkpoints, local values, and the cooked
//! runtime baseline separate, and survive spatial-query eviction while dirty, saving, or newer
//! than the active runtime generation.

use std::collections::{BTreeMap, HashMap, HashSet};
mod definitions;
mod presets;
use definitions::DefinitionEntry;
pub(crate) use definitions::{DirtyDefinitionSnapshot, definition_bytes};
pub(crate) use presets::{DirtyPresetSnapshot, library_bytes};

use bevy::prelude::*;
use world::{CellCoord, WorldSpaceId};
use world_db::{
    DenseSourceRecord, DenseSourceRecordKey, DenseSourceWrite,
    MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION,
};

use crate::{
    project_store::{DenseSaveOutcome, ProjectEditorStore},
    saving::EditorSaveCoordinator,
};

#[derive(Debug, Clone)]
enum DenseSaveState {
    Idle,
    Saving(u64),
    Conflict(Option<DenseSourceRecord>),
    Failed(String),
}

#[derive(Debug, Clone)]
struct DenseEditEntry {
    base: Option<DenseSourceRecord>,
    current: DenseSourceRecord,
    runtime: Option<DenseSourceRecord>,
    save_state: DenseSaveState,
}

impl DenseEditEntry {
    fn dirty(&self) -> bool {
        !same_dense_content(self.base.as_ref(), &self.current)
    }

    fn pinned(&self) -> bool {
        self.dirty()
            || self.runtime_diverged()
            || matches!(self.save_state, DenseSaveState::Saving(_))
    }

    fn runtime_diverged(&self) -> bool {
        !same_dense_content(self.runtime.as_ref(), &self.current)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirtyDenseSnapshot {
    pub(crate) base: Option<DenseSourceRecord>,
    pub(crate) current: DenseSourceRecord,
    pub(crate) runtime: Option<DenseSourceRecord>,
}

#[derive(Resource, Default)]
pub(crate) struct DenseDomainWorkingSets {
    pub(crate) atmospheres: crate::atmosphere_authoring::working::WorkingSet,
    pub(crate) roads: crate::road_authoring::working::RoadWorkingSet,
    definitions: BTreeMap<WorldSpaceId, DefinitionEntry>,
    presets: Option<DefinitionEntry<environment::PresetLibrary>>,
    definition_revision: u64,
    plants: Option<vegetation::VegetationCatalog>,
    entries: HashMap<DenseSourceRecordKey, DenseEditEntry>,
    last_completed_query: u64,
    edit_revision: u64,
    pub(crate) gesture_active: bool,
}

impl DenseDomainWorkingSets {
    #[cfg(test)]
    pub(crate) fn from_environment_records(
        records: &[world_db::SourceEnvironmentCellRecord],
    ) -> Self {
        let mut result = Self::default();
        for record in records {
            let current = DenseSourceRecord::EnvironmentCoverage(record.clone());
            let base = (record.source_revision > 0).then(|| current.clone());
            result.entries.insert(
                dense_record_key(&current),
                DenseEditEntry {
                    base: base.clone(),
                    current,
                    runtime: base,
                    save_state: DenseSaveState::Idle,
                },
            );
        }
        result
    }

    pub(crate) fn environment_record(
        &self,
        space: WorldSpaceId,
        cell: CellCoord,
    ) -> Option<&world_db::SourceEnvironmentCellRecord> {
        self.entries
            .get(&DenseSourceRecordKey::EnvironmentCoverage { space, cell })
            .map(|entry| {
                let DenseSourceRecord::EnvironmentCoverage(record) = &entry.current;
                record
            })
    }

    /// Applies every endpoint owner together. The unsaved working set is one bounded atomic
    /// save unit; it cannot grow into a gesture that the database would have to split.
    pub(crate) fn apply_environment_records(
        &mut self,
        records: &[world_db::SourceEnvironmentCellRecord],
    ) -> Result<(), String> {
        let mut replacements = HashMap::new();
        for record in records {
            let key = DenseSourceRecordKey::EnvironmentCoverage {
                space: record.space,
                cell: record.cell,
            };
            let entry = self
                .entries
                .get(&key)
                .ok_or("Wait for this area to load before editing")?;
            let mut record = record.clone();
            if let Some(definition) = self.definition(record.space) {
                if record.tiles.iter().any(|tile| {
                    !definition.layers.iter().any(|l| l.id == tile.layer)
                        || tile.samples.len() != usize::from(definition.mask_resolution).pow(2)
                }) {
                    return Err("Coverage command does not match the current layers".into());
                }
                record.definition_revision = definition.revision;
            }
            let replacement = DenseSourceRecord::EnvironmentCoverage(record);
            if !same_dense_shape(&entry.current, &replacement)
                || matches!(
                    entry.save_state,
                    DenseSaveState::Saving(_) | DenseSaveState::Conflict(_)
                )
            {
                return Err("Resolve the pending save or changed layer definition first".into());
            }
            let mut replacement = replacement;
            set_dense_revision(&mut replacement, dense_source_revision(&entry.current));
            if replacements.insert(key, replacement).is_some() {
                return Err("Duplicate cell in coverage command".into());
            }
        }
        let mut dirty = 0;
        let mut dirty_bytes = 0;
        let mut retained = 0;
        for (key, entry) in &self.entries {
            let next = replacements.get(key).unwrap_or(&entry.current);
            retained += dense_record_bytes(next)
                + entry.base.as_ref().map_or(0, dense_record_bytes)
                + entry.runtime.as_ref().map_or(0, dense_record_bytes);
            if !same_dense_content(entry.base.as_ref(), next) {
                dirty += 1;
                dirty_bytes += dense_record_bytes(next);
            }
        }
        if dirty > MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION || dirty_bytes > 3 * 1024 * 1024 {
            return Err("Save these changes before painting more area".into());
        }
        if retained > 32 * 1024 * 1024 || self.entries.len() > 512 {
            return Err(
                "Publish changes and clear undo history to release the retained painting area"
                    .into(),
            );
        }
        let mut changed = false;
        for (key, next) in replacements {
            let entry = self.entries.get_mut(&key).unwrap();
            if !same_dense_content(Some(&entry.current), &next) {
                entry.current = next;
                entry.save_state = DenseSaveState::Idle;
                changed = true;
            }
        }
        if changed {
            self.bump_revision();
        }
        Ok(())
    }

    pub(crate) fn dirty_environment_cells(
        &self,
    ) -> impl Iterator<Item = &world_db::SourceEnvironmentCellRecord> {
        self.entries
            .values()
            .filter(|entry| entry.dirty())
            .map(|entry| {
                let DenseSourceRecord::EnvironmentCoverage(record) = &entry.current;
                record
            })
    }

    pub(crate) fn dirty_snapshots(&self) -> Vec<DirtyDenseSnapshot> {
        let mut snapshots = self
            .entries
            .values()
            .filter(|entry| entry.dirty())
            .map(|entry| DirtyDenseSnapshot {
                base: entry.base.clone(),
                current: entry.current.clone(),
                runtime: entry.runtime.clone(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| format!("{:?}", dense_record_key(&snapshot.current)));
        snapshots
    }

    pub(crate) fn restore_dirty_snapshot(&mut self, snapshot: DirtyDenseSnapshot) -> bool {
        let key = dense_record_key(&snapshot.current);
        if snapshot.base.as_ref() == Some(&snapshot.current)
            || !valid_dense_record(&snapshot.current)
            || snapshot.base.as_ref().is_some_and(|record| {
                dense_record_key(record) != key || !valid_dense_record(record)
            })
            || snapshot.runtime.as_ref().is_some_and(|record| {
                dense_record_key(record) != key || !valid_dense_record(record)
            })
        {
            return false;
        }
        self.entries.insert(
            key,
            DenseEditEntry {
                base: snapshot.base,
                current: snapshot.current,
                runtime: snapshot.runtime,
                save_state: DenseSaveState::Idle,
            },
        );
        self.bump_revision();
        true
    }

    pub(crate) fn environment_record_count(&self) -> usize {
        self.entries
            .keys()
            .filter(|key| matches!(key, DenseSourceRecordKey::EnvironmentCoverage { .. }))
            .count()
    }

    pub(crate) fn dirty_count(&self) -> usize {
        self.atmospheres.dirty_count()
            + self.roads.dirty_count()
            + self.definition_dirty_count()
            + self.entries.values().filter(|entry| entry.dirty()).count()
    }

    pub(crate) fn saving(&self) -> bool {
        self.atmospheres.saving.is_some()
            || self.roads.saving.is_some()
            || self.definitions_saving()
            || self
                .entries
                .values()
                .any(|entry| matches!(entry.save_state, DenseSaveState::Saving(_)))
    }

    pub(crate) fn has_any_conflict(&self) -> bool {
        self.atmospheres.conflict.is_some()
            || self.roads.conflict
            || self.definition_conflict_count() > 0
            || self
                .entries
                .values()
                .any(|entry| matches!(entry.save_state, DenseSaveState::Conflict(_)))
    }

    pub(crate) fn conflict_count(&self) -> usize {
        usize::from(self.atmospheres.conflict.is_some())
            + usize::from(self.roads.conflict)
            + self.definition_conflict_count()
            + self
                .entries
                .values()
                .filter(|entry| matches!(entry.save_state, DenseSaveState::Conflict(_)))
                .count()
    }

    pub(crate) fn can_accept_database_conflicts(&self) -> bool {
        self.can_accept_definition_conflicts()
            || self.entries.values().any(|entry| {
                matches!(
                    &entry.save_state,
                    DenseSaveState::Conflict(Some(actual))
                        if same_dense_shape(actual, &entry.current)
                )
            })
    }

    pub(crate) fn can_keep_local_conflicts(&self) -> bool {
        self.can_keep_definition_conflicts()
            || self.entries.values().any(|entry| match &entry.save_state {
                DenseSaveState::Conflict(None) => true,
                DenseSaveState::Conflict(Some(actual)) => same_dense_shape(actual, &entry.current),
                _ => false,
            })
    }

    /// Advances conflicted checkpoints to the latest database revisions while retaining local
    /// coverage bytes for a retry. Conflicts with an incompatible record shape remain unresolved.
    pub(crate) fn keep_local_conflicts(&mut self) -> usize {
        let mut resolved = self.resolve_definition_conflicts(true);
        for entry in self.entries.values_mut() {
            let DenseSaveState::Conflict(actual) = &entry.save_state else {
                continue;
            };
            let actual = actual.clone();
            if let Some(actual) = actual.as_ref() {
                if !same_dense_shape(actual, &entry.current) {
                    continue;
                }
                set_dense_revision(&mut entry.current, dense_source_revision(actual));
            } else {
                set_dense_revision(&mut entry.current, 0);
            }
            entry.base = actual;
            entry.save_state = DenseSaveState::Idle;
            resolved += 1;
        }
        if resolved > 0 {
            self.bump_revision();
        }
        resolved
    }

    /// Discards local bytes in favor of concrete database records. Normal erase retains an empty
    /// record; an externally removed record or changed definition requires a project reload.
    pub(crate) fn accept_database_conflicts(&mut self) -> usize {
        let mut resolved = self.resolve_definition_conflicts(false);
        for entry in self.entries.values_mut() {
            let DenseSaveState::Conflict(Some(actual)) = &entry.save_state else {
                continue;
            };
            if !same_dense_shape(actual, &entry.current) {
                continue;
            }
            entry.base = Some(actual.clone());
            entry.current = actual.clone();
            entry.save_state = DenseSaveState::Idle;
            resolved += 1;
        }
        if resolved > 0 {
            self.bump_revision();
        }
        resolved
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.presets
            .as_ref()
            .map_or(0, |p| library_bytes(&p.base) + library_bytes(&p.current))
            + self
                .definitions
                .values()
                .map(|entry| definition_bytes(&entry.base) + definition_bytes(&entry.current))
                .sum::<usize>()
            + self
                .entries
                .values()
                .map(|entry| {
                    entry.base.as_ref().map_or(0, dense_record_bytes)
                        + dense_record_bytes(&entry.current)
                        + entry.runtime.as_ref().map_or(0, dense_record_bytes)
                })
                .sum::<usize>()
    }

    pub(crate) fn edit_revision(&self) -> u64 {
        self.edit_revision
    }

    /// Advances every tracked cooked baseline to the project checkpoint represented by a newly
    /// adopted immutable generation. Unsaved current values remain divergent from `base`.
    pub(crate) fn adopt_runtime_generation(&mut self) -> usize {
        let mut adopted = 0;
        for entry in self.entries.values_mut() {
            if entry.runtime != entry.base {
                entry.runtime.clone_from(&entry.base);
                adopted += 1;
            }
        }
        if adopted > 0 {
            self.bump_revision();
        }
        adopted
    }

    pub(crate) fn current_records(&self) -> Vec<DenseSourceRecord> {
        self.entries
            .values()
            .map(|entry| entry.current.clone())
            .collect()
    }

    /// Preview overlays must also include an unsaved undo back to the runtime baseline: in that
    /// case the runtime matches the draft but the saved project database no longer does.
    pub(crate) fn preview_records(&self) -> Vec<DenseSourceRecord> {
        self.entries
            .values()
            .filter(|entry| entry.dirty() || entry.runtime_diverged())
            .map(|entry| entry.current.clone())
            .collect()
    }

    pub(crate) fn runtime_divergent_records(&self) -> Vec<DenseSourceRecord> {
        self.entries
            .values()
            .filter(|entry| entry.runtime_diverged())
            .map(|entry| entry.current.clone())
            .collect()
    }

    pub(crate) fn queue_save(&mut self, project: &mut ProjectEditorStore) -> bool {
        if self.saving() || self.has_any_conflict() {
            return false;
        }
        if self
            .entries
            .values()
            .any(|entry| matches!(entry.save_state, DenseSaveState::Saving(_)))
        {
            return false;
        }
        let mut dirty = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.dirty())
            .collect::<Vec<_>>();
        dirty.sort_by_key(|(key, _)| format!("{key:?}"));
        let selected = dirty
            .into_iter()
            .map(|(key, entry)| {
                (
                    key.clone(),
                    dense_write(entry.base.as_ref(), &entry.current),
                )
            })
            .collect::<Vec<_>>();
        if selected.len() > MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION || self.gesture_active {
            return false;
        }
        let writes = selected
            .iter()
            .map(|(_, write)| write.clone())
            .collect::<Vec<_>>();
        let Some(request_id) = project.queue_dense_transaction(
            self.preset_base_revision(),
            self.preset_write(),
            self.definition_writes(),
            writes,
            self.roads.writes(),
            self.roads.dependencies(),
        ) else {
            return false;
        };
        self.mark_definitions_saving(request_id);
        self.roads.saving = Some(request_id);
        for (key, _) in selected {
            self.entries
                .get_mut(&key)
                .expect("selected dense write remains in the working set")
                .save_state = DenseSaveState::Saving(request_id);
        }
        true
    }

    pub(crate) fn status(&self) -> Option<String> {
        if let Some(error) = &self.roads.error {
            return Some(error.clone());
        }
        if let Some(status) = self.definition_status() {
            return Some(status);
        }
        self.entries
            .values()
            .find_map(|entry| match &entry.save_state {
                DenseSaveState::Idle => None,
                DenseSaveState::Saving(request) => {
                    Some(format!("saving dense transaction {request}"))
                }
                DenseSaveState::Conflict(actual) => Some(if actual.is_some() {
                    "dense source conflict; local patch retained".into()
                } else {
                    "dense source record disappeared; local patch retained".into()
                }),
                DenseSaveState::Failed(error) => Some(format!("dense save failed: {error}")),
            })
    }

    fn reconcile(&mut self, project: &ProjectEditorStore, history: &crate::editing::EditorHistory) {
        if self.last_completed_query == project.completed_queries() {
            return;
        }
        self.last_completed_query = project.completed_queries();

        let fresh = project
            .environment_cells()
            .iter()
            .cloned()
            .map(DenseSourceRecord::EnvironmentCoverage)
            .collect::<Vec<_>>();
        let fresh_keys = fresh.iter().map(dense_record_key).collect::<HashSet<_>>();
        let history_cells = history.environment_cells();
        self.entries.retain(|key, entry| {
            let DenseSourceRecordKey::EnvironmentCoverage { space, cell } = key;
            entry.pinned() || fresh_keys.contains(key) || history_cells.contains(&(*space, *cell))
        });
        for record in fresh {
            let key = dense_record_key(&record);
            let DenseSourceRecordKey::EnvironmentCoverage { space, .. } = &key;
            if self
                .definitions
                .get(space)
                .is_some_and(|e| matches!(e.state, definitions::DefinitionSaveState::Conflict(_)))
            {
                continue;
            }
            match self.entries.get_mut(&key) {
                Some(entry) if !entry.pinned() => {
                    entry.base = (dense_source_revision(&record) > 0).then(|| record.clone());
                    entry.current = record.clone();
                    entry.runtime = (dense_source_revision(&record) > 0).then_some(record);
                    entry.save_state = DenseSaveState::Idle;
                }
                Some(_) => {}
                None => {
                    self.entries.insert(
                        key,
                        DenseEditEntry {
                            base: (dense_source_revision(&record) > 0).then(|| record.clone()),
                            current: record.clone(),
                            runtime: (dense_source_revision(&record) > 0).then_some(record),
                            save_state: DenseSaveState::Idle,
                        },
                    );
                }
            }
        }
    }

    fn finish_save(&mut self, request_id: u64, outcome: DenseSaveOutcome) {
        let matching_keys = self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                matches!(entry.save_state, DenseSaveState::Saving(id) if id == request_id)
                    .then_some(key.clone())
            })
            .collect::<Vec<_>>();
        self.finish_definitions(request_id, &outcome);
        self.roads.finish_save(request_id, &outcome);
        match outcome {
            DenseSaveOutcome::Committed(commits) => {
                for key in &matching_keys {
                    if let Some(entry) = self.entries.get_mut(key) {
                        entry.save_state = DenseSaveState::Idle;
                    }
                }
                for commit in commits.coverage {
                    let key = dense_record_key(&commit);
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.base = Some(commit.clone());
                        entry.current = commit;
                        entry.save_state = DenseSaveState::Idle;
                    }
                }
            }
            DenseSaveOutcome::RoadConflict { .. }
            | DenseSaveOutcome::DefinitionConflict { .. }
            | DenseSaveOutcome::LibraryConflict { .. } => {
                for key in matching_keys {
                    self.entries.get_mut(&key).unwrap().save_state = DenseSaveState::Idle;
                }
            }
            DenseSaveOutcome::Conflict { key, actual } => {
                for matching in matching_keys {
                    if let Some(entry) = self.entries.get_mut(&matching) {
                        entry.save_state = if matching == key {
                            DenseSaveState::Conflict(actual.clone())
                        } else {
                            DenseSaveState::Idle
                        };
                    }
                }
            }
            DenseSaveOutcome::Failed(error) => {
                for key in matching_keys {
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.save_state = DenseSaveState::Failed(error.clone());
                    }
                }
            }
        }
        self.bump_revision();
    }

    fn bump_revision(&mut self) {
        self.edit_revision = self.edit_revision.wrapping_add(1).max(1);
    }
}

pub(crate) fn reconcile_dense_working_sets(
    mut working_sets: ResMut<DenseDomainWorkingSets>,
    project: Res<ProjectEditorStore>,
    history: Res<crate::editing::EditorHistory>,
    plants: Res<crate::vegetation_authoring::VegetationAuthoringState>,
) {
    working_sets.reconcile_definitions(
        &project,
        plants
            .study_source()
            .map(|(catalog, _, _)| catalog)
            .or_else(|| project.vegetation_catalog()),
    );
    working_sets.reconcile(&project, &history);
}

pub(crate) fn process_dense_save_completion(
    mut working_sets: ResMut<DenseDomainWorkingSets>,
    mut project: ResMut<ProjectEditorStore>,
    mut coordinator: ResMut<EditorSaveCoordinator>,
) {
    let Some(completion) = project.take_dense_save_completion() else {
        return;
    };
    let committed = matches!(&completion.outcome, DenseSaveOutcome::Committed(_));
    working_sets.finish_save(completion.request_id, completion.outcome);
    coordinator.transaction_finished(committed);
}

fn dense_record_key(record: &DenseSourceRecord) -> DenseSourceRecordKey {
    match record {
        DenseSourceRecord::EnvironmentCoverage(record) => {
            DenseSourceRecordKey::EnvironmentCoverage {
                space: record.space,
                cell: record.cell,
            }
        }
    }
}

fn dense_record_bytes(record: &DenseSourceRecord) -> usize {
    std::mem::size_of::<DenseSourceRecord>()
        + match record {
            DenseSourceRecord::EnvironmentCoverage(record) => record.sample_bytes(),
        }
}

fn valid_dense_record(record: &DenseSourceRecord) -> bool {
    match record {
        DenseSourceRecord::EnvironmentCoverage(record) => {
            record.source_revision >= 0
                && record.definition_revision > 0
                && record
                    .tiles
                    .iter()
                    .all(|t| (4..=257 * 257).contains(&t.samples.len()))
        }
    }
}

fn dense_source_revision(record: &DenseSourceRecord) -> i64 {
    match record {
        DenseSourceRecord::EnvironmentCoverage(record) => record.source_revision,
    }
}

fn set_dense_revision(record: &mut DenseSourceRecord, revision: i64) {
    match record {
        DenseSourceRecord::EnvironmentCoverage(record) => record.source_revision = revision,
    }
}

fn same_dense_shape(left: &DenseSourceRecord, right: &DenseSourceRecord) -> bool {
    match (left, right) {
        (
            DenseSourceRecord::EnvironmentCoverage(left),
            DenseSourceRecord::EnvironmentCoverage(right),
        ) => {
            left.space == right.space
                && left.cell == right.cell
                && left.definition_revision == right.definition_revision
        }
    }
}

fn dense_write(base: Option<&DenseSourceRecord>, current: &DenseSourceRecord) -> DenseSourceWrite {
    match (base, current) {
        (
            Some(DenseSourceRecord::EnvironmentCoverage(base)),
            DenseSourceRecord::EnvironmentCoverage(current),
        ) => DenseSourceWrite::EnvironmentCoverage {
            expected_source_revision: Some(base.source_revision),
            record: current.clone(),
        },
        (None, DenseSourceRecord::EnvironmentCoverage(current)) => {
            DenseSourceWrite::EnvironmentCoverage {
                expected_source_revision: None,
                record: current.clone(),
            }
        }
    }
}

fn same_dense_content(base: Option<&DenseSourceRecord>, current: &DenseSourceRecord) -> bool {
    let DenseSourceRecord::EnvironmentCoverage(current) = current;
    match base {
        Some(DenseSourceRecord::EnvironmentCoverage(base)) => {
            base.space == current.space
                && base.cell == current.cell
                && base.definition_revision == current.definition_revision
                && base.tiles == current.tiles
        }
        None => current.tiles.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::{EditorHistory, EditorObjectWorkingSet};
    fn record(x: i32, value: u8) -> world_db::SourceEnvironmentCellRecord {
        world_db::SourceEnvironmentCellRecord {
            space: WorldSpaceId(1),
            cell: CellCoord { x, z: 0 },
            source_revision: 1,
            definition_revision: 1,
            tiles: vec![environment::CoverageTile {
                layer: environment::LayerId([1; 16]),
                samples: vec![value; 9],
            }],
        }
    }
    #[test]
    fn environment_undo_redo_rebases_on_saved_revision_and_retains_runtime_delta() {
        let before = record(0, 60);
        let mut after = record(0, 180);
        let mut dense =
            DenseDomainWorkingSets::from_environment_records(std::slice::from_ref(&before));
        let mut history = EditorHistory::default();
        let mut objects = EditorObjectWorkingSet::default();
        dense
            .apply_environment_records(std::slice::from_ref(&after))
            .unwrap();
        history.record_environment_stroke(vec![before.clone()], vec![after.clone()]);
        let key = DenseSourceRecordKey::EnvironmentCoverage {
            space: before.space,
            cell: before.cell,
        };
        dense.entries.get_mut(&key).unwrap().save_state = DenseSaveState::Saving(1);
        after.source_revision = 2;
        dense.finish_save(
            1,
            DenseSaveOutcome::Committed(world_db::EnvironmentSourceCommit {
                roads: None,
                presets: None,
                definitions: vec![],
                coverage: vec![DenseSourceRecord::EnvironmentCoverage(after.clone())],
            }),
        );
        assert_eq!(dense.dirty_count(), 0);
        assert_eq!(dense.runtime_divergent_records().len(), 1);
        assert!(history.undo(&mut objects, &mut dense));
        let undone = dense.environment_record(before.space, before.cell).unwrap();
        assert_eq!(undone.tiles, before.tiles);
        assert_eq!(undone.source_revision, 2);
        assert_eq!(dense.dirty_count(), 1);
        assert!(dense.runtime_divergent_records().is_empty());
        assert_eq!(dense.preview_records().len(), 1);
        assert!(history.redo(&mut objects, &mut dense));
        assert_eq!(
            dense.environment_record(before.space, before.cell),
            Some(&after)
        );
        assert_eq!(dense.dirty_count(), 0);
        assert_eq!(dense.runtime_divergent_records().len(), 1);
        dense.adopt_runtime_generation();
        assert!(dense.runtime_divergent_records().is_empty());
        assert!(history.undo(&mut objects, &mut dense));
        assert_eq!(dense.runtime_divergent_records().len(), 1);
    }
    #[test]
    fn rejected_large_gesture_does_not_apply_any_cells() {
        let records = (0..65).map(|x| record(x, 60)).collect::<Vec<_>>();
        let mut dense = DenseDomainWorkingSets::from_environment_records(&records);
        let changed = (0..65).map(|x| record(x, 180)).collect::<Vec<_>>();
        assert!(dense.apply_environment_records(&changed).is_err());
        assert_eq!(dense.dirty_count(), 0);
        for r in &records {
            assert_eq!(dense.environment_record(r.space, r.cell), Some(r));
        }
        dense.apply_environment_records(&changed[..64]).unwrap();
        assert_eq!(dense.dirty_count(), 64);
    }
    #[test]
    fn absent_empty_cell_is_clean_and_cancel_restores_it() {
        let mut before = record(0, 0);
        before.source_revision = 0;
        before.tiles.clear();
        let mut dense =
            DenseDomainWorkingSets::from_environment_records(std::slice::from_ref(&before));
        assert_eq!(dense.dirty_count(), 0);
        dense.apply_environment_records(&[record(0, 120)]).unwrap();
        assert_eq!(dense.dirty_count(), 1);
        dense.apply_environment_records(&[before]).unwrap();
        assert_eq!(dense.dirty_count(), 0);
        assert!(dense.runtime_divergent_records().is_empty());
    }
}
