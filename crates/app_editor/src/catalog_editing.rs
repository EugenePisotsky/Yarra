//! Stable-ID working state for ground-cover region catalog commands.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use world::GroundCoverRegionId;
use world_db::{
    GroundCoverRegionWrite, GroundCoverRegionWriteCommit,
    MAX_GROUND_COVER_REGION_WRITES_PER_TRANSACTION, SourceGroundCoverRegionRecord,
};

use crate::{
    editing::EditorHistory,
    project_store::{ProjectEditorStore, RegionSaveOutcome},
    saving::EditorSaveCoordinator,
};

#[derive(Debug, Clone)]
enum RegionSaveState {
    Idle,
    Saving(u64),
    Conflict(Option<SourceGroundCoverRegionRecord>),
    BlockedByCoverage,
    Failed(String),
}

#[derive(Debug, Clone)]
struct RegionEditEntry {
    base: Option<SourceGroundCoverRegionRecord>,
    current: Option<SourceGroundCoverRegionRecord>,
    save_state: RegionSaveState,
}

impl RegionEditEntry {
    fn dirty(&self) -> bool {
        self.base != self.current
    }

    fn pinned(&self) -> bool {
        self.dirty() || !matches!(self.save_state, RegionSaveState::Idle)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirtyRegionSnapshot {
    pub(crate) base: Option<SourceGroundCoverRegionRecord>,
    pub(crate) current: Option<SourceGroundCoverRegionRecord>,
}

#[derive(Resource, Default)]
pub(crate) struct GroundCoverRegionWorkingSet {
    entries: HashMap<GroundCoverRegionId, RegionEditEntry>,
    last_completed_query: u64,
    edit_revision: u64,
    runtime_diverged: bool,
}

impl GroundCoverRegionWorkingSet {
    pub(crate) fn edit_revision(&self) -> u64 {
        self.edit_revision
    }

    /// Saving updates the source database but the runtime still owns the last published
    /// generation. Keep presenting source-derived pages until that generation is replaced.
    pub(crate) const fn runtime_diverged(&self) -> bool {
        self.runtime_diverged
    }

    pub(crate) fn adopt_runtime_generation(&mut self) -> bool {
        if !self.runtime_diverged {
            return false;
        }
        self.runtime_diverged = false;
        self.bump_revision();
        true
    }

    pub(crate) fn dirty_count(&self) -> usize {
        self.entries.values().filter(|entry| entry.dirty()).count()
    }

    pub(crate) fn saving(&self) -> bool {
        self.entries
            .values()
            .any(|entry| matches!(entry.save_state, RegionSaveState::Saving(_)))
    }

    pub(crate) fn has_any_conflict(&self) -> bool {
        self.entries
            .values()
            .any(|entry| matches!(entry.save_state, RegionSaveState::Conflict(_)))
    }

    pub(crate) fn conflict_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry.save_state, RegionSaveState::Conflict(_)))
            .count()
    }

    pub(crate) fn can_keep_local_conflicts(&self) -> bool {
        self.entries.values().any(|entry| match &entry.save_state {
            RegionSaveState::Conflict(Some(actual)) => {
                entry.current.as_ref().is_none_or(|current| {
                    current.id == actual.id
                        && current.layer == actual.layer
                        && current.space == actual.space
                })
            }
            RegionSaveState::Conflict(None) => true,
            _ => false,
        })
    }

    pub(crate) fn keep_local_conflicts(&mut self) -> usize {
        let mut resolved = 0;
        let mut remove = Vec::new();
        for (id, entry) in &mut self.entries {
            let RegionSaveState::Conflict(actual) = &entry.save_state else {
                continue;
            };
            match (actual.clone(), entry.current.as_mut()) {
                (Some(actual), None) => {
                    entry.base = Some(actual);
                    entry.save_state = RegionSaveState::Idle;
                    resolved += 1;
                }
                (Some(actual), Some(current))
                    if current.id == actual.id
                        && current.layer == actual.layer
                        && current.space == actual.space =>
                {
                    current.source_revision = actual.source_revision;
                    entry.base = Some(actual);
                    entry.save_state = RegionSaveState::Idle;
                    resolved += 1;
                }
                (None, Some(_)) => {
                    entry.base = None;
                    entry.save_state = RegionSaveState::Idle;
                    resolved += 1;
                }
                (None, None) => {
                    remove.push(*id);
                    resolved += 1;
                }
                (Some(_), Some(_)) => {}
            }
        }
        for id in remove {
            self.entries.remove(&id);
        }
        if resolved > 0 {
            self.mark_runtime_diverged();
        }
        resolved
    }

    pub(crate) fn accept_database_conflicts(&mut self) -> usize {
        let conflicts = self
            .entries
            .iter()
            .filter_map(|(id, entry)| match &entry.save_state {
                RegionSaveState::Conflict(actual) => Some((*id, actual.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (id, actual) in &conflicts {
            if let Some(actual) = actual {
                if let Some(entry) = self.entries.get_mut(id) {
                    entry.base = Some(actual.clone());
                    entry.current = Some(actual.clone());
                    entry.save_state = RegionSaveState::Idle;
                }
            } else {
                self.entries.remove(id);
            }
        }
        if !conflicts.is_empty() {
            self.mark_runtime_diverged();
        }
        conflicts.len()
    }

    pub(crate) fn dismiss_status(&mut self) -> bool {
        let mut dismissed = false;
        for entry in self.entries.values_mut() {
            if matches!(
                entry.save_state,
                RegionSaveState::BlockedByCoverage | RegionSaveState::Failed(_)
            ) {
                entry.save_state = RegionSaveState::Idle;
                dismissed = true;
            }
        }
        if dismissed {
            self.bump_revision();
        }
        dismissed
    }

    pub(crate) fn current_records(
        &self,
        project: &ProjectEditorStore,
    ) -> Vec<SourceGroundCoverRegionRecord> {
        let mut records = project
            .ground_cover_regions()
            .iter()
            .cloned()
            .map(|record| (record.id, record))
            .collect::<HashMap<_, _>>();
        for (id, entry) in &self.entries {
            if let Some(current) = &entry.current {
                records.insert(*id, current.clone());
            } else {
                records.remove(id);
            }
        }
        let mut records = records.into_values().collect::<Vec<_>>();
        records.sort_by(|left, right| {
            (left.layer.0, left.display_name.as_str(), left.id.0).cmp(&(
                right.layer.0,
                right.display_name.as_str(),
                right.id.0,
            ))
        });
        records
    }

    pub(crate) fn current(
        &self,
        project: &ProjectEditorStore,
        region: GroundCoverRegionId,
    ) -> Option<SourceGroundCoverRegionRecord> {
        self.entries
            .get(&region)
            .map(|entry| entry.current.clone())
            .unwrap_or_else(|| {
                project
                    .ground_cover_regions()
                    .iter()
                    .find(|record| record.id == region)
                    .cloned()
            })
    }

    pub(crate) fn create(&mut self, record: SourceGroundCoverRegionRecord) -> bool {
        if self
            .entries
            .get(&record.id)
            .is_some_and(|entry| entry.current.is_some())
        {
            return false;
        }
        self.entries.insert(
            record.id,
            RegionEditEntry {
                base: None,
                current: Some(record),
                save_state: RegionSaveState::Idle,
            },
        );
        self.mark_runtime_diverged();
        true
    }

    pub(crate) fn restore(&mut self, record: SourceGroundCoverRegionRecord) -> bool {
        if let Some(entry) = self.entries.get_mut(&record.id) {
            if entry.current.is_some() || matches!(entry.save_state, RegionSaveState::Saving(_)) {
                return false;
            }
            entry.current = Some(entry.base.clone().unwrap_or(record));
            entry.save_state = RegionSaveState::Idle;
            self.mark_runtime_diverged();
            return true;
        }
        self.create(record)
    }

    pub(crate) fn replace(&mut self, mut record: SourceGroundCoverRegionRecord) -> bool {
        if !valid_region(&record) {
            return false;
        }
        let Some(entry) = self.entries.get_mut(&record.id) else {
            return false;
        };
        let Some(current) = entry.current.as_ref() else {
            return false;
        };
        if matches!(entry.save_state, RegionSaveState::Saving(_))
            || current.layer != record.layer
            || current.space != record.space
        {
            return false;
        }
        // History remains valid across save checkpoints: replay the command's authored fields on
        // the latest accepted revision rather than resurrecting its old optimistic-lock token.
        record.source_revision = current.source_revision;
        if current == &record {
            return false;
        }
        entry.current = Some(record);
        entry.save_state = RegionSaveState::Idle;
        self.mark_runtime_diverged();
        true
    }

    pub(crate) fn pin_source(&mut self, source: SourceGroundCoverRegionRecord) {
        self.entries
            .entry(source.id)
            .or_insert_with(|| RegionEditEntry {
                base: Some(source.clone()),
                current: Some(source),
                save_state: RegionSaveState::Idle,
            });
    }

    pub(crate) fn delete(
        &mut self,
        region: GroundCoverRegionId,
    ) -> Option<SourceGroundCoverRegionRecord> {
        if !self.entries.contains_key(&region) {
            return None;
        }
        let entry = self.entries.get_mut(&region)?;
        if matches!(entry.save_state, RegionSaveState::Saving(_)) {
            return None;
        }
        let deleted = entry.current.take()?;
        entry.save_state = RegionSaveState::Idle;
        self.mark_runtime_diverged();
        Some(deleted)
    }

    pub(crate) fn dirty_snapshots(&self) -> Vec<DirtyRegionSnapshot> {
        let mut snapshots = self
            .entries
            .values()
            .filter(|entry| entry.dirty())
            .map(|entry| DirtyRegionSnapshot {
                base: entry.base.clone(),
                current: entry.current.clone(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| {
            snapshot
                .current
                .as_ref()
                .or(snapshot.base.as_ref())
                .map(|record| record.id.0)
                .unwrap_or([0; 16])
        });
        snapshots
    }

    pub(crate) fn restore_dirty_snapshot(&mut self, snapshot: DirtyRegionSnapshot) -> bool {
        let Some(id) = snapshot
            .current
            .as_ref()
            .or(snapshot.base.as_ref())
            .map(|record| record.id)
        else {
            return false;
        };
        let supported_command_shape = match (&snapshot.base, &snapshot.current) {
            (None, Some(_)) | (Some(_), None) => true,
            (Some(base), Some(current)) => {
                base.layer == current.layer
                    && base.space == current.space
                    && base.source_revision == current.source_revision
            }
            (None, None) => false,
        };
        if !supported_command_shape
            || snapshot.base == snapshot.current
            || snapshot
                .base
                .as_ref()
                .is_some_and(|record| record.id != id || !valid_region(record))
            || snapshot
                .current
                .as_ref()
                .is_some_and(|record| record.id != id || !valid_region(record))
        {
            return false;
        }
        self.entries.insert(
            id,
            RegionEditEntry {
                base: snapshot.base,
                current: snapshot.current,
                save_state: RegionSaveState::Idle,
            },
        );
        self.mark_runtime_diverged();
        true
    }

    pub(crate) fn queue_save(&mut self, project: &mut ProjectEditorStore) -> bool {
        if self.saving() || self.has_any_conflict() {
            return false;
        }
        let mut dirty = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.dirty())
            .collect::<Vec<_>>();
        dirty.sort_by_key(|(id, _)| id.0);
        let selected = dirty
            .into_iter()
            .take(MAX_GROUND_COVER_REGION_WRITES_PER_TRANSACTION)
            .filter_map(|(id, entry)| {
                let write = match (&entry.base, &entry.current) {
                    (None, Some(current)) => GroundCoverRegionWrite::Create {
                        record: current.clone(),
                    },
                    (Some(base), None) => GroundCoverRegionWrite::DeleteEmpty {
                        region: *id,
                        expected_source_revision: base.source_revision,
                    },
                    (Some(base), Some(current)) => GroundCoverRegionWrite::Update {
                        expected_source_revision: base.source_revision,
                        record: current.clone(),
                    },
                    _ => return None,
                };
                Some((*id, write))
            })
            .collect::<Vec<_>>();
        let Some(request_id) = project
            .queue_region_transaction(selected.iter().map(|(_, write)| write.clone()).collect())
        else {
            return false;
        };
        for (id, _) in selected {
            self.entries
                .get_mut(&id)
                .expect("selected region remains tracked")
                .save_state = RegionSaveState::Saving(request_id);
        }
        true
    }

    pub(crate) fn status(&self) -> Option<String> {
        self.entries
            .values()
            .find_map(|entry| match &entry.save_state {
                RegionSaveState::Idle => None,
                RegionSaveState::Saving(request) => {
                    Some(format!("saving region transaction {request}"))
                }
                RegionSaveState::Conflict(actual) => Some(if actual.is_some() {
                    "region changed in the database; local command retained".into()
                } else {
                    "region disappeared from the database; local command retained".into()
                }),
                RegionSaveState::BlockedByCoverage => {
                    Some("region deletion was blocked because coverage still exists".into())
                }
                RegionSaveState::Failed(error) => Some(format!("region save failed: {error}")),
            })
    }

    fn reconcile(&mut self, project: &ProjectEditorStore) {
        if self.last_completed_query == project.completed_queries() {
            return;
        }
        self.last_completed_query = project.completed_queries();
        let fresh_ids = project
            .ground_cover_regions()
            .iter()
            .map(|record| record.id)
            .collect::<HashSet<_>>();
        self.entries
            .retain(|id, entry| entry.pinned() || fresh_ids.contains(id));
        for record in project.ground_cover_regions() {
            match self.entries.get_mut(&record.id) {
                Some(entry) if !entry.pinned() => {
                    entry.base = Some(record.clone());
                    entry.current = Some(record.clone());
                    entry.save_state = RegionSaveState::Idle;
                }
                Some(_) => {}
                None => {
                    self.entries.insert(
                        record.id,
                        RegionEditEntry {
                            base: Some(record.clone()),
                            current: Some(record.clone()),
                            save_state: RegionSaveState::Idle,
                        },
                    );
                }
            }
        }
    }

    fn finish_save(&mut self, request_id: u64, outcome: RegionSaveOutcome) -> bool {
        let matching = self
            .entries
            .iter()
            .filter_map(|(id, entry)| {
                matches!(entry.save_state, RegionSaveState::Saving(request) if request == request_id)
                    .then_some(*id)
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return false;
        }
        let mut history_is_stale = false;
        match outcome {
            RegionSaveOutcome::Committed(commits) => {
                for commit in commits {
                    match commit {
                        GroundCoverRegionWriteCommit::Created(record) => {
                            if let Some(entry) = self.entries.get_mut(&record.id) {
                                entry.base = Some(record.clone());
                                entry.current = Some(record);
                                entry.save_state = RegionSaveState::Idle;
                            }
                        }
                        GroundCoverRegionWriteCommit::Updated(record) => {
                            if let Some(entry) = self.entries.get_mut(&record.id) {
                                entry.base = Some(record.clone());
                                entry.current = Some(record);
                                entry.save_state = RegionSaveState::Idle;
                            }
                        }
                        GroundCoverRegionWriteCommit::Deleted(region) => {
                            self.entries.remove(&region);
                        }
                    }
                }
            }
            RegionSaveOutcome::Conflict { region, actual } => {
                for id in matching {
                    if let Some(entry) = self.entries.get_mut(&id) {
                        entry.save_state = if id == region {
                            RegionSaveState::Conflict(actual.clone())
                        } else {
                            RegionSaveState::Idle
                        };
                    }
                }
            }
            RegionSaveOutcome::BlockedByCoverage { region } => {
                for id in matching {
                    if let Some(entry) = self.entries.get_mut(&id) {
                        if id == region {
                            entry.current.clone_from(&entry.base);
                            entry.save_state = RegionSaveState::BlockedByCoverage;
                            history_is_stale = true;
                        } else {
                            entry.save_state = RegionSaveState::Idle;
                        }
                    }
                }
            }
            RegionSaveOutcome::Failed(error) => {
                for id in matching {
                    if let Some(entry) = self.entries.get_mut(&id) {
                        entry.save_state = RegionSaveState::Failed(error.clone());
                    }
                }
            }
        }
        self.bump_revision();
        history_is_stale
    }

    fn bump_revision(&mut self) {
        self.edit_revision = self.edit_revision.wrapping_add(1).max(1);
    }

    fn mark_runtime_diverged(&mut self) {
        self.runtime_diverged = true;
        self.bump_revision();
    }
}

pub(crate) fn reconcile_ground_cover_regions(
    mut regions: ResMut<GroundCoverRegionWorkingSet>,
    project: Res<ProjectEditorStore>,
) {
    regions.reconcile(&project);
}

pub(crate) fn process_region_save_completion(
    mut regions: ResMut<GroundCoverRegionWorkingSet>,
    mut history: ResMut<EditorHistory>,
    mut project: ResMut<ProjectEditorStore>,
    mut coordinator: ResMut<EditorSaveCoordinator>,
) {
    let Some(completion) = project.take_region_save_completion() else {
        return;
    };
    let committed = matches!(&completion.outcome, RegionSaveOutcome::Committed(_));
    if regions.finish_save(completion.request_id, completion.outcome) {
        history.clear();
    }
    coordinator.transaction_finished(committed);
}

fn valid_region(record: &SourceGroundCoverRegionRecord) -> bool {
    !record.display_name.trim().is_empty()
        && record.density_multiplier.is_finite()
        && record.density_multiplier > 0.0
        && record.source_revision >= 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::{GroundCoverLayerId, GroundCoverPresetId, WorldSpaceId};

    fn region(id: u8) -> SourceGroundCoverRegionRecord {
        SourceGroundCoverRegionRecord {
            id: GroundCoverRegionId([id; 16]),
            layer: GroundCoverLayerId([2; 16]),
            space: WorldSpaceId(1),
            preset: GroundCoverPresetId([3; 16]),
            display_name: format!("Region {id}"),
            enabled: true,
            density_multiplier: 1.0,
            source_revision: 0,
        }
    }

    #[test]
    fn unsaved_region_create_and_delete_are_reversible_working_states() {
        let mut working = GroundCoverRegionWorkingSet::default();
        let record = region(7);
        assert!(working.create(record.clone()));
        assert_eq!(working.dirty_count(), 1);
        assert_eq!(working.delete(record.id), Some(record.clone()));
        assert_eq!(working.dirty_count(), 0);
        assert!(working.restore(record));
        assert_eq!(working.dirty_count(), 1);
    }

    #[test]
    fn dirty_region_snapshot_restores_a_sparse_creation() {
        let record = region(9);
        let mut working = GroundCoverRegionWorkingSet::default();
        assert!(working.restore_dirty_snapshot(DirtyRegionSnapshot {
            base: None,
            current: Some(record.clone()),
        }));
        assert_eq!(working.dirty_snapshots().len(), 1);
        assert_eq!(
            working.current(&ProjectEditorStore::default(), record.id),
            Some(record)
        );
    }

    #[test]
    fn replaying_region_values_uses_the_latest_revision() {
        let before = region(4);
        let mut saved = before.clone();
        saved.display_name = "Renamed".into();
        saved.source_revision = 3;
        let mut working = GroundCoverRegionWorkingSet::default();
        working.entries.insert(
            before.id,
            RegionEditEntry {
                base: Some(saved.clone()),
                current: Some(saved),
                save_state: RegionSaveState::Idle,
            },
        );
        assert!(working.replace(before.clone()));
        let current = working
            .current(&ProjectEditorStore::default(), before.id)
            .unwrap();
        assert_eq!(current.display_name, before.display_name);
        assert_eq!(current.source_revision, 3);
    }

    #[test]
    fn saving_a_region_edit_keeps_preview_diverged_until_publication_adoption() {
        let record = region(6);
        let mut working = GroundCoverRegionWorkingSet::default();
        assert!(working.create(record.clone()));
        working.entries.get_mut(&record.id).unwrap().save_state = RegionSaveState::Saving(12);
        let mut committed = record;
        committed.source_revision = 1;

        assert!(!working.finish_save(
            12,
            RegionSaveOutcome::Committed(vec![GroundCoverRegionWriteCommit::Created(committed)]),
        ));
        assert_eq!(working.dirty_count(), 0);
        assert!(working.runtime_diverged());
        assert!(working.adopt_runtime_generation());
        assert!(!working.runtime_diverged());
    }
}
