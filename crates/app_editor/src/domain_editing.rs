//! Bounded terrain source working sets.
//!
//! Dense records are keyed by domain/cell, keep database checkpoints, local values, and the cooked
//! runtime baseline separate, and survive spatial-query eviction while dirty, saving, or newer
//! than the active runtime generation.

use std::collections::{HashMap, HashSet};

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
        self.base.as_ref() != Some(&self.current)
    }

    fn pinned(&self) -> bool {
        self.dirty()
            || self.runtime_diverged()
            || matches!(self.save_state, DenseSaveState::Saving(_))
    }

    fn runtime_diverged(&self) -> bool {
        self.runtime.as_ref() != Some(&self.current)
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
    entries: HashMap<DenseSourceRecordKey, DenseEditEntry>,
    last_completed_query: u64,
    edit_revision: u64,
}

impl DenseDomainWorkingSets {
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

    pub(crate) fn terrain_record_count(&self) -> usize {
        self.entries
            .keys()
            .filter(|key| matches!(key, DenseSourceRecordKey::TerrainWeights { .. }))
            .count()
    }

    pub(crate) fn dirty_count(&self) -> usize {
        self.entries.values().filter(|entry| entry.dirty()).count()
    }

    pub(crate) fn saving(&self) -> bool {
        self.entries
            .values()
            .any(|entry| matches!(entry.save_state, DenseSaveState::Saving(_)))
    }

    pub(crate) fn has_any_conflict(&self) -> bool {
        self.entries
            .values()
            .any(|entry| matches!(entry.save_state, DenseSaveState::Conflict(_)))
    }

    pub(crate) fn conflict_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry.save_state, DenseSaveState::Conflict(_)))
            .count()
    }

    pub(crate) fn can_accept_database_conflicts(&self) -> bool {
        self.entries.values().any(|entry| {
            matches!(
                &entry.save_state,
                DenseSaveState::Conflict(Some(actual))
                    if same_dense_shape(actual, &entry.current)
            )
        })
    }

    pub(crate) fn can_keep_local_conflicts(&self) -> bool {
        self.entries.values().any(|entry| match &entry.save_state {
            DenseSaveState::Conflict(None) => true,
            DenseSaveState::Conflict(Some(actual)) => same_dense_shape(actual, &entry.current),
            _ => false,
        })
    }

    /// Advances conflicted checkpoints to the latest database revisions while retaining local
    /// coverage bytes for a retry. Conflicts with an incompatible record shape remain unresolved.
    pub(crate) fn keep_local_conflicts(&mut self) -> usize {
        let mut resolved = 0;
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

    /// Discards local bytes in favor of concrete database records. A record that disappeared
    /// cannot yet be accepted because dense tombstones are not part of the source writer.
    pub(crate) fn accept_database_conflicts(&mut self) -> usize {
        let mut resolved = 0;
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
        self.entries
            .values()
            .map(|entry| {
                entry.base.as_ref().map_or(0, dense_record_bytes)
                    + dense_record_bytes(&entry.current)
                    + entry.runtime.as_ref().map_or(0, dense_record_bytes)
            })
            .sum()
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

    pub(crate) fn runtime_divergent_records(&self) -> Vec<DenseSourceRecord> {
        self.entries
            .values()
            .filter(|entry| entry.runtime_diverged())
            .map(|entry| entry.current.clone())
            .collect()
    }

    pub(crate) fn contains_patch(&self, space: WorldSpaceId, cell: CellCoord) -> bool {
        self.entries.keys().any(|key| {
            matches!(
                key,
                DenseSourceRecordKey::TerrainWeights {
                    space: record_space,
                    cell: record_cell,
                    ..
                } if *record_space == space && *record_cell == cell
            )
        })
    }

    #[allow(
        dead_code,
        reason = "the terrain brush will use the typed whole-record mutation seam"
    )]
    pub(crate) fn replace_record(&mut self, replacement: DenseSourceRecord) -> bool {
        let key = dense_record_key(&replacement);
        let Some(entry) = self.entries.get_mut(&key) else {
            return false;
        };
        if matches!(entry.save_state, DenseSaveState::Saving(_))
            || !same_dense_shape(&entry.current, &replacement)
            || entry.current == replacement
        {
            return false;
        }
        entry.current = replacement;
        entry.save_state = DenseSaveState::Idle;
        self.bump_revision();
        true
    }

    pub(crate) fn queue_save(&mut self, project: &mut ProjectEditorStore) -> bool {
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
            .take(MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION)
            .map(|(key, entry)| {
                (
                    key.clone(),
                    dense_write(entry.base.as_ref(), &entry.current),
                )
            })
            .collect::<Vec<_>>();
        let writes = selected
            .iter()
            .map(|(_, write)| write.clone())
            .collect::<Vec<_>>();
        let Some(request_id) = project.queue_dense_transaction(writes) else {
            return false;
        };
        for (key, _) in selected {
            self.entries
                .get_mut(&key)
                .expect("selected dense write remains in the working set")
                .save_state = DenseSaveState::Saving(request_id);
        }
        true
    }

    pub(crate) fn status(&self) -> Option<String> {
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

    fn reconcile(&mut self, project: &ProjectEditorStore) {
        if self.last_completed_query == project.completed_queries() {
            return;
        }
        self.last_completed_query = project.completed_queries();

        let fresh = project
            .terrain_weight_pages()
            .iter()
            .cloned()
            .map(DenseSourceRecord::TerrainWeights)
            .collect::<Vec<_>>();
        let fresh_keys = fresh.iter().map(dense_record_key).collect::<HashSet<_>>();
        self.entries
            .retain(|key, entry| entry.pinned() || fresh_keys.contains(key));
        for record in fresh {
            let key = dense_record_key(&record);
            match self.entries.get_mut(&key) {
                Some(entry) if !entry.pinned() => {
                    entry.base = Some(record.clone());
                    entry.current = record.clone();
                    entry.runtime = Some(record);
                    entry.save_state = DenseSaveState::Idle;
                }
                Some(_) => {}
                None => {
                    self.entries.insert(
                        key,
                        DenseEditEntry {
                            base: Some(record.clone()),
                            current: record.clone(),
                            runtime: Some(record),
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
        if matching_keys.is_empty() {
            return;
        }
        match outcome {
            DenseSaveOutcome::Committed(commits) => {
                for key in &matching_keys {
                    if let Some(entry) = self.entries.get_mut(key) {
                        entry.save_state = DenseSaveState::Idle;
                    }
                }
                for commit in commits {
                    let key = dense_record_key(&commit);
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.base = Some(commit.clone());
                        entry.current = commit;
                        entry.save_state = DenseSaveState::Idle;
                    }
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
) {
    working_sets.reconcile(&project);
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
        DenseSourceRecord::TerrainWeights(record) => DenseSourceRecordKey::TerrainWeights {
            space: record.space,
            cell: record.cell,
            page: record.page,
        },
    }
}

fn dense_record_bytes(record: &DenseSourceRecord) -> usize {
    std::mem::size_of::<DenseSourceRecord>()
        + match record {
            DenseSourceRecord::TerrainWeights(record) => record.rgba.len(),
        }
}

fn valid_dense_record(record: &DenseSourceRecord) -> bool {
    match record {
        DenseSourceRecord::TerrainWeights(record) => {
            let resolution = usize::from(record.resolution);
            record.source_revision >= 0
                && resolution > 0
                && resolution
                    .checked_mul(resolution)
                    .and_then(|samples| samples.checked_mul(4))
                    == Some(record.rgba.len())
        }
    }
}

fn dense_source_revision(record: &DenseSourceRecord) -> i64 {
    match record {
        DenseSourceRecord::TerrainWeights(record) => record.source_revision,
    }
}

fn set_dense_revision(record: &mut DenseSourceRecord, revision: i64) {
    match record {
        DenseSourceRecord::TerrainWeights(record) => record.source_revision = revision,
    }
}

#[allow(
    dead_code,
    reason = "validation helper for the reserved terrain whole-record mutation seam"
)]
fn same_dense_shape(left: &DenseSourceRecord, right: &DenseSourceRecord) -> bool {
    match (left, right) {
        (DenseSourceRecord::TerrainWeights(left), DenseSourceRecord::TerrainWeights(right)) => {
            left.space == right.space
                && left.cell == right.cell
                && left.page == right.page
                && left.resolution == right.resolution
        }
    }
}

fn dense_write(base: Option<&DenseSourceRecord>, current: &DenseSourceRecord) -> DenseSourceWrite {
    match (base, current) {
        (
            Some(DenseSourceRecord::TerrainWeights(base)),
            DenseSourceRecord::TerrainWeights(current),
        ) => DenseSourceWrite::TerrainWeights {
            expected_source_revision: Some(base.source_revision),
            record: current.clone(),
        },
        (None, DenseSourceRecord::TerrainWeights(current)) => DenseSourceWrite::TerrainWeights {
            expected_source_revision: None,
            record: current.clone(),
        },
    }
}
