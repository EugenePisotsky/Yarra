//! Bounded terrain and ground-cover source working sets.
//!
//! Dense records are keyed by domain/cell, keep database checkpoints separate from local values,
//! and survive spatial-query eviction while dirty or saving. Brush tools can later add compact
//! reversible patches without changing this persistence boundary.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use world::{CellCoord, WorldSpaceId};
use world_db::{
    DenseSourceRecord, DenseSourceRecordKey, DenseSourceWrite,
    MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION,
};

use crate::project_store::{DenseSaveOutcome, ProjectEditorStore};

#[derive(Debug, Clone)]
enum DenseSaveState {
    Idle,
    Saving(u64),
    Conflict(Option<DenseSourceRecord>),
    Failed(String),
}

#[derive(Debug, Clone)]
struct DenseEditEntry {
    base: DenseSourceRecord,
    current: DenseSourceRecord,
    save_state: DenseSaveState,
}

impl DenseEditEntry {
    fn dirty(&self) -> bool {
        self.base != self.current
    }

    fn pinned(&self) -> bool {
        self.dirty() || matches!(self.save_state, DenseSaveState::Saving(_))
    }
}

#[derive(Resource, Default)]
pub(crate) struct DenseDomainWorkingSets {
    entries: HashMap<DenseSourceRecordKey, DenseEditEntry>,
    ground_cover_layer_count: usize,
    last_completed_query: u64,
    edit_revision: u64,
}

impl DenseDomainWorkingSets {
    pub(crate) fn terrain_record_count(&self) -> usize {
        self.entries
            .keys()
            .filter(|key| matches!(key, DenseSourceRecordKey::TerrainWeights { .. }))
            .count()
    }

    pub(crate) fn ground_cover_record_count(&self) -> usize {
        self.entries
            .keys()
            .filter(|key| matches!(key, DenseSourceRecordKey::GroundCoverMask { .. }))
            .count()
    }

    pub(crate) fn ground_cover_layer_count(&self) -> usize {
        self.ground_cover_layer_count
    }

    pub(crate) fn dirty_count(&self) -> usize {
        self.entries.values().filter(|entry| entry.dirty()).count()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.entries
            .values()
            .map(|entry| dense_record_bytes(&entry.base) + dense_record_bytes(&entry.current))
            .sum()
    }

    pub(crate) fn edit_revision(&self) -> u64 {
        self.edit_revision
    }

    pub(crate) fn current_records(&self) -> Vec<DenseSourceRecord> {
        self.entries
            .values()
            .map(|entry| entry.current.clone())
            .collect()
    }

    pub(crate) fn dirty_records(&self) -> Vec<DenseSourceRecord> {
        self.entries
            .values()
            .filter(|entry| entry.dirty())
            .map(|entry| entry.current.clone())
            .collect()
    }

    pub(crate) fn contains_patch(
        &self,
        space: WorldSpaceId,
        cell: CellCoord,
        terrain: bool,
    ) -> bool {
        self.entries.keys().any(|key| match key {
            DenseSourceRecordKey::TerrainWeights {
                space: record_space,
                cell: record_cell,
                ..
            } => terrain && *record_space == space && *record_cell == cell,
            DenseSourceRecordKey::GroundCoverMask {
                space: record_space,
                cell: record_cell,
                ..
            } => !terrain && *record_space == space && *record_cell == cell,
        })
    }

    #[allow(
        dead_code,
        reason = "brush tools consume this mutation seam in the next UI slice"
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

    #[allow(
        dead_code,
        reason = "save controls are enabled with the first paint gesture"
    )]
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
            .map(|(key, entry)| (key.clone(), dense_write(&entry.base, &entry.current)))
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
        self.ground_cover_layer_count = project.ground_cover_layers().len();

        let fresh = project
            .terrain_weight_pages()
            .iter()
            .cloned()
            .map(DenseSourceRecord::TerrainWeights)
            .chain(
                project
                    .ground_cover_masks()
                    .iter()
                    .cloned()
                    .map(DenseSourceRecord::GroundCoverMask),
            )
            .collect::<Vec<_>>();
        let fresh_keys = fresh.iter().map(dense_record_key).collect::<HashSet<_>>();
        self.entries
            .retain(|key, entry| entry.pinned() || fresh_keys.contains(key));
        for record in fresh {
            let key = dense_record_key(&record);
            match self.entries.get_mut(&key) {
                Some(entry) if !entry.pinned() => {
                    entry.base = record.clone();
                    entry.current = record;
                    entry.save_state = DenseSaveState::Idle;
                }
                Some(_) => {}
                None => {
                    self.entries.insert(
                        key,
                        DenseEditEntry {
                            base: record.clone(),
                            current: record,
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
                        entry.base = commit.clone();
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
) {
    let Some(completion) = project.take_dense_save_completion() else {
        return;
    };
    working_sets.finish_save(completion.request_id, completion.outcome);
}

fn dense_record_key(record: &DenseSourceRecord) -> DenseSourceRecordKey {
    match record {
        DenseSourceRecord::TerrainWeights(record) => DenseSourceRecordKey::TerrainWeights {
            space: record.space,
            cell: record.cell,
            page: record.page,
        },
        DenseSourceRecord::GroundCoverMask(record) => DenseSourceRecordKey::GroundCoverMask {
            layer: record.layer,
            space: record.space,
            cell: record.cell,
        },
    }
}

fn dense_record_bytes(record: &DenseSourceRecord) -> usize {
    std::mem::size_of::<DenseSourceRecord>()
        + match record {
            DenseSourceRecord::TerrainWeights(record) => record.rgba.len(),
            DenseSourceRecord::GroundCoverMask(record) => record.coverage.len(),
        }
}

fn same_dense_shape(left: &DenseSourceRecord, right: &DenseSourceRecord) -> bool {
    match (left, right) {
        (DenseSourceRecord::TerrainWeights(left), DenseSourceRecord::TerrainWeights(right)) => {
            left.space == right.space
                && left.cell == right.cell
                && left.page == right.page
                && left.resolution == right.resolution
        }
        (DenseSourceRecord::GroundCoverMask(left), DenseSourceRecord::GroundCoverMask(right)) => {
            left.layer == right.layer
                && left.space == right.space
                && left.cell == right.cell
                && left.resolution == right.resolution
        }
        _ => false,
    }
}

fn dense_write(base: &DenseSourceRecord, current: &DenseSourceRecord) -> DenseSourceWrite {
    match (base, current) {
        (DenseSourceRecord::TerrainWeights(base), DenseSourceRecord::TerrainWeights(current)) => {
            DenseSourceWrite::TerrainWeights {
                expected_source_revision: Some(base.source_revision),
                record: current.clone(),
            }
        }
        (DenseSourceRecord::GroundCoverMask(base), DenseSourceRecord::GroundCoverMask(current)) => {
            DenseSourceWrite::GroundCoverMask {
                expected_source_revision: Some(base.source_revision),
                record: current.clone(),
            }
        }
        _ => unreachable!("a dense working-set entry cannot change domain identity"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use world_db::SourceTerrainCellWeightPageRecord;

    #[test]
    fn dense_working_set_rejects_shape_changes_and_tracks_payload_bytes() {
        let base = DenseSourceRecord::TerrainWeights(SourceTerrainCellWeightPageRecord {
            space: WorldSpaceId(1),
            cell: CellCoord::ZERO,
            page: 0,
            resolution: 2,
            rgba: vec![0; 16],
            source_revision: 1,
        });
        let key = dense_record_key(&base);
        let mut working = DenseDomainWorkingSets::default();
        working.entries.insert(
            key,
            DenseEditEntry {
                base: base.clone(),
                current: base.clone(),
                save_state: DenseSaveState::Idle,
            },
        );
        let mut changed = base.clone();
        let DenseSourceRecord::TerrainWeights(record) = &mut changed else {
            unreachable!()
        };
        record.rgba[0] = 255;
        assert!(working.replace_record(changed));
        assert_eq!(working.dirty_count(), 1);
        assert!(working.retained_bytes() >= 32);

        let invalid = DenseSourceRecord::TerrainWeights(SourceTerrainCellWeightPageRecord {
            space: WorldSpaceId(1),
            cell: CellCoord::ZERO,
            page: 0,
            resolution: 4,
            rgba: vec![0; 64],
            source_revision: 1,
        });
        assert!(!working.replace_record(invalid));
    }

    #[test]
    fn dense_save_pins_only_the_bounded_submitted_batch() {
        let mut working = DenseDomainWorkingSets::default();
        for x in 0..=MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION as i32 {
            let base = DenseSourceRecord::TerrainWeights(SourceTerrainCellWeightPageRecord {
                space: WorldSpaceId(1),
                cell: CellCoord { x, z: 0 },
                page: 0,
                resolution: 2,
                rgba: vec![0; 16],
                source_revision: 1,
            });
            let mut current = base.clone();
            let DenseSourceRecord::TerrainWeights(record) = &mut current else {
                unreachable!()
            };
            record.rgba[0] = 1;
            working.entries.insert(
                dense_record_key(&base),
                DenseEditEntry {
                    base,
                    current,
                    save_state: DenseSaveState::Idle,
                },
            );
        }
        let mut project = ProjectEditorStore::default();
        assert!(working.queue_save(&mut project));
        assert_eq!(
            working
                .entries
                .values()
                .filter(|entry| matches!(entry.save_state, DenseSaveState::Saving(_)))
                .count(),
            MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION
        );
        assert_eq!(
            working
                .entries
                .values()
                .filter(|entry| matches!(entry.save_state, DenseSaveState::Idle))
                .count(),
            1
        );
    }
}
