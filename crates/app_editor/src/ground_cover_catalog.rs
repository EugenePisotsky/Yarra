//! Mutable editor state for reusable ground-cover presets and visual definitions.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use world::{GroundCoverPresetId, GroundCoverVisualId};
use world_db::{
    GroundCoverCatalogDependency, GroundCoverCatalogKey, GroundCoverCatalogRecord,
    GroundCoverCatalogWrite, GroundCoverCatalogWriteCommit,
    MAX_GROUND_COVER_CATALOG_WRITES_PER_TRANSACTION, SourceGroundCoverPresetRecord,
    SourceGroundCoverVisualDefinition, SourceGroundCoverVisualRecord,
};

use crate::{
    project_store::{CatalogSaveOutcome, ProjectEditorStore},
    saving::EditorSaveCoordinator,
};

#[derive(Debug, Clone)]
enum CatalogSaveState {
    Idle,
    Saving(u64),
    Conflict(Option<GroundCoverCatalogRecord>),
    BlockedByDependency(GroundCoverCatalogDependency),
    Failed(String),
}

#[derive(Debug, Clone)]
struct CatalogEditEntry {
    base: Option<GroundCoverCatalogRecord>,
    current: Option<GroundCoverCatalogRecord>,
    save_state: CatalogSaveState,
}

impl CatalogEditEntry {
    fn dirty(&self) -> bool {
        self.base != self.current
    }

    fn pinned(&self) -> bool {
        self.base.is_none() || self.dirty() || !matches!(self.save_state, CatalogSaveState::Idle)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DirtyCatalogSnapshot {
    pub(crate) base: Option<GroundCoverCatalogRecord>,
    pub(crate) current: Option<GroundCoverCatalogRecord>,
}

#[derive(Resource, Default)]
pub(crate) struct GroundCoverCatalogWorkingSet {
    entries: HashMap<GroundCoverCatalogKey, CatalogEditEntry>,
    last_completed_query: u64,
    edit_revision: u64,
    runtime_diverged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CatalogSavePhase {
    Upserts,
    Deletions,
}

impl GroundCoverCatalogWorkingSet {
    pub(crate) const fn edit_revision(&self) -> u64 {
        self.edit_revision
    }

    /// Whether the source catalog differs from the immutable generation currently shown by the
    /// runtime streamer. Saving intentionally does not clear this flag; only successful runtime
    /// generation adoption does.
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

    pub(crate) fn dirty_upsert_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.dirty() && entry.current.is_some())
            .count()
    }

    pub(crate) fn dirty_deletion_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.base.is_some() && entry.current.is_none())
            .count()
    }

    pub(crate) fn saving(&self) -> bool {
        self.entries
            .values()
            .any(|entry| matches!(entry.save_state, CatalogSaveState::Saving(_)))
    }

    pub(crate) fn has_any_conflict(&self) -> bool {
        self.entries
            .values()
            .any(|entry| matches!(entry.save_state, CatalogSaveState::Conflict(_)))
    }

    pub(crate) fn conflict_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry.save_state, CatalogSaveState::Conflict(_)))
            .count()
    }

    pub(crate) fn can_keep_local_conflicts(&self) -> bool {
        self.entries.values().any(|entry| {
            let CatalogSaveState::Conflict(actual) = &entry.save_state else {
                return false;
            };
            match (&entry.base, actual, &entry.current) {
                (Some(_), Some(actual), Some(current)) => actual.key() == current.key(),
                (Some(_), Some(_), None) | (Some(_), None, Some(_)) => true,
                _ => false,
            }
        })
    }

    pub(crate) fn current_presets(
        &self,
        project: &ProjectEditorStore,
    ) -> Vec<SourceGroundCoverPresetRecord> {
        let mut records = project
            .ground_cover_presets()
            .iter()
            .cloned()
            .map(|record| (record.id, record))
            .collect::<HashMap<_, _>>();
        for (key, entry) in &self.entries {
            if let GroundCoverCatalogKey::Preset(id) = key {
                match &entry.current {
                    Some(GroundCoverCatalogRecord::Preset(record)) => {
                        records.insert(*id, record.clone());
                    }
                    None => {
                        records.remove(id);
                    }
                    Some(GroundCoverCatalogRecord::Visual(_)) => {}
                }
            }
        }
        let mut records = records.into_values().collect::<Vec<_>>();
        records.sort_by(|left, right| {
            (left.display_name.as_str(), left.id.0).cmp(&(right.display_name.as_str(), right.id.0))
        });
        records
    }

    pub(crate) fn current_visuals(
        &self,
        project: &ProjectEditorStore,
    ) -> Vec<SourceGroundCoverVisualRecord> {
        let mut records = project
            .ground_cover_visuals()
            .iter()
            .cloned()
            .map(|record| (record.id, record))
            .collect::<HashMap<_, _>>();
        for (key, entry) in &self.entries {
            if let GroundCoverCatalogKey::Visual(id) = key {
                match &entry.current {
                    Some(GroundCoverCatalogRecord::Visual(record)) => {
                        records.insert(*id, record.clone());
                    }
                    None => {
                        records.remove(id);
                    }
                    Some(GroundCoverCatalogRecord::Preset(_)) => {}
                }
            }
        }
        let mut records = records.into_values().collect::<Vec<_>>();
        records.sort_by(|left, right| {
            (left.display_name.as_str(), left.id.0).cmp(&(right.display_name.as_str(), right.id.0))
        });
        records
    }

    pub(crate) fn current_preset(
        &self,
        project: &ProjectEditorStore,
        id: GroundCoverPresetId,
    ) -> Option<SourceGroundCoverPresetRecord> {
        if let Some(entry) = self.entries.get(&GroundCoverCatalogKey::Preset(id)) {
            return match &entry.current {
                Some(GroundCoverCatalogRecord::Preset(record)) => Some(record.clone()),
                None | Some(GroundCoverCatalogRecord::Visual(_)) => None,
            };
        }
        project
            .ground_cover_presets()
            .iter()
            .find(|record| record.id == id)
            .cloned()
    }

    pub(crate) fn current_visual(
        &self,
        project: &ProjectEditorStore,
        id: GroundCoverVisualId,
    ) -> Option<SourceGroundCoverVisualRecord> {
        if let Some(entry) = self.entries.get(&GroundCoverCatalogKey::Visual(id)) {
            return match &entry.current {
                Some(GroundCoverCatalogRecord::Visual(record)) => Some(record.clone()),
                None | Some(GroundCoverCatalogRecord::Preset(_)) => None,
            };
        }
        project
            .ground_cover_visuals()
            .iter()
            .find(|record| record.id == id)
            .cloned()
    }

    pub(crate) fn replace_preset(&mut self, record: SourceGroundCoverPresetRecord) -> bool {
        self.replace(GroundCoverCatalogRecord::Preset(record))
    }

    pub(crate) fn replace_visual(&mut self, record: SourceGroundCoverVisualRecord) -> bool {
        self.replace(GroundCoverCatalogRecord::Visual(record))
    }

    pub(crate) fn create_preset(&mut self, record: SourceGroundCoverPresetRecord) -> bool {
        self.create(GroundCoverCatalogRecord::Preset(record))
    }

    pub(crate) fn create_visual(&mut self, record: SourceGroundCoverVisualRecord) -> bool {
        self.create(GroundCoverCatalogRecord::Visual(record))
    }

    fn create(&mut self, record: GroundCoverCatalogRecord) -> bool {
        if !valid_catalog_record(&record) {
            return false;
        }
        let key = record.key();
        if self
            .entries
            .get(&key)
            .is_some_and(|entry| entry.current.is_some())
        {
            return false;
        }
        self.entries.insert(
            key,
            CatalogEditEntry {
                base: None,
                current: Some(record),
                save_state: CatalogSaveState::Idle,
            },
        );
        self.mark_runtime_diverged();
        true
    }

    pub(crate) fn restore(&mut self, record: GroundCoverCatalogRecord) -> bool {
        let key = record.key();
        if let Some(entry) = self.entries.get_mut(&key) {
            if entry.current.is_some() || matches!(entry.save_state, CatalogSaveState::Saving(_)) {
                return false;
            }
            entry.current = Some(entry.base.clone().unwrap_or(record));
            entry.save_state = CatalogSaveState::Idle;
            self.mark_runtime_diverged();
            return true;
        }
        self.create(record)
    }

    pub(crate) fn delete(
        &mut self,
        key: GroundCoverCatalogKey,
    ) -> Option<GroundCoverCatalogRecord> {
        let entry = self.entries.get_mut(&key)?;
        if matches!(entry.save_state, CatalogSaveState::Saving(_)) {
            return None;
        }
        let deleted = entry.current.take()?;
        entry.save_state = CatalogSaveState::Idle;
        self.mark_runtime_diverged();
        Some(deleted)
    }

    fn replace(&mut self, mut record: GroundCoverCatalogRecord) -> bool {
        if !valid_catalog_record(&record) {
            return false;
        }
        let key = record.key();
        let Some(entry) = self.entries.get_mut(&key) else {
            return false;
        };
        if matches!(entry.save_state, CatalogSaveState::Saving(_)) {
            return false;
        }
        let Some(current) = entry.current.as_ref() else {
            return false;
        };
        let current_revision = current.source_revision();
        match &mut record {
            GroundCoverCatalogRecord::Preset(record) => {
                record.source_revision = current_revision;
            }
            GroundCoverCatalogRecord::Visual(record) => {
                record.source_revision = current_revision;
            }
        }
        if current == &record {
            return false;
        }
        entry.current = Some(record);
        entry.save_state = CatalogSaveState::Idle;
        self.mark_runtime_diverged();
        true
    }

    pub(crate) fn dirty_snapshots(&self) -> Vec<DirtyCatalogSnapshot> {
        let mut snapshots = self
            .entries
            .values()
            .filter(|entry| entry.dirty())
            .map(|entry| DirtyCatalogSnapshot {
                base: entry.base.clone(),
                current: entry.current.clone(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| {
            catalog_sort_key(
                snapshot
                    .current
                    .as_ref()
                    .or(snapshot.base.as_ref())
                    .expect("a dirty catalog snapshot has one record")
                    .key(),
            )
        });
        snapshots
    }

    pub(crate) fn restore_dirty_snapshot(&mut self, snapshot: DirtyCatalogSnapshot) -> bool {
        let Some(key) = snapshot
            .current
            .as_ref()
            .or(snapshot.base.as_ref())
            .map(GroundCoverCatalogRecord::key)
        else {
            return false;
        };
        if snapshot.base == snapshot.current
            || snapshot
                .base
                .as_ref()
                .is_some_and(|record| record.key() != key || !valid_catalog_record(record))
            || snapshot
                .current
                .as_ref()
                .is_some_and(|record| record.key() != key || !valid_catalog_record(record))
            || matches!((&snapshot.base, &snapshot.current), (Some(base), Some(current))
                if base.source_revision() != current.source_revision())
        {
            return false;
        }
        self.entries.insert(
            key,
            CatalogEditEntry {
                base: snapshot.base,
                current: snapshot.current,
                save_state: CatalogSaveState::Idle,
            },
        );
        self.mark_runtime_diverged();
        true
    }

    pub(crate) fn queue_save_phase(
        &mut self,
        project: &mut ProjectEditorStore,
        phase: CatalogSavePhase,
    ) -> bool {
        if self.saving() || self.has_any_conflict() {
            return false;
        }
        let mut dirty = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                entry.dirty()
                    && match phase {
                        CatalogSavePhase::Upserts => entry.current.is_some(),
                        CatalogSavePhase::Deletions => {
                            entry.base.is_some() && entry.current.is_none()
                        }
                    }
            })
            .collect::<Vec<_>>();
        dirty.sort_by_key(|(key, _)| catalog_write_sort_key(**key, phase));
        let selected = dirty
            .into_iter()
            .take(MAX_GROUND_COVER_CATALOG_WRITES_PER_TRANSACTION)
            .filter_map(|(key, entry)| {
                let write = match (&entry.base, &entry.current) {
                    (None, Some(GroundCoverCatalogRecord::Preset(record))) => {
                        GroundCoverCatalogWrite::CreatePreset {
                            record: record.clone(),
                        }
                    }
                    (None, Some(GroundCoverCatalogRecord::Visual(record))) => {
                        GroundCoverCatalogWrite::CreateVisual {
                            record: record.clone(),
                        }
                    }
                    (Some(base), Some(GroundCoverCatalogRecord::Preset(record))) => {
                        GroundCoverCatalogWrite::UpdatePreset {
                            expected_source_revision: base.source_revision(),
                            record: record.clone(),
                        }
                    }
                    (Some(base), Some(GroundCoverCatalogRecord::Visual(record))) => {
                        GroundCoverCatalogWrite::UpdateVisual {
                            expected_source_revision: base.source_revision(),
                            record: record.clone(),
                        }
                    }
                    (Some(GroundCoverCatalogRecord::Preset(base)), None) => {
                        GroundCoverCatalogWrite::DeletePreset {
                            preset: base.id,
                            expected_source_revision: base.source_revision,
                        }
                    }
                    (Some(GroundCoverCatalogRecord::Visual(base)), None) => {
                        GroundCoverCatalogWrite::DeleteVisual {
                            visual: base.id,
                            expected_source_revision: base.source_revision,
                        }
                    }
                    (None, None) => return None,
                };
                Some((*key, write))
            })
            .collect::<Vec<_>>();
        let Some(request_id) = project
            .queue_catalog_transaction(selected.iter().map(|(_, write)| write.clone()).collect())
        else {
            return false;
        };
        for (key, _) in selected {
            self.entries
                .get_mut(&key)
                .expect("selected catalog entry remains tracked")
                .save_state = CatalogSaveState::Saving(request_id);
        }
        true
    }

    pub(crate) fn status(&self) -> Option<String> {
        self.entries
            .values()
            .find_map(|entry| match &entry.save_state {
                CatalogSaveState::Idle => None,
                CatalogSaveState::Saving(request) => {
                    Some(format!("saving ground-cover catalog transaction {request}"))
                }
                CatalogSaveState::Conflict(actual) => Some(if actual.is_some() {
                    "ground-cover definition changed in the database; local edit retained".into()
                } else {
                    "ground-cover definition disappeared from the database; local edit retained"
                        .into()
                }),
                CatalogSaveState::BlockedByDependency(dependency) => Some(match dependency {
                    GroundCoverCatalogDependency::PresetRegions => {
                        "preset deletion is blocked by one or more project regions".into()
                    }
                    GroundCoverCatalogDependency::VisualPresets => {
                        "visual deletion is blocked by one or more presets".into()
                    }
                }),
                CatalogSaveState::Failed(error) => {
                    Some(format!("ground-cover catalog save failed: {error}"))
                }
            })
    }

    pub(crate) fn accept_database_conflicts(&mut self) -> usize {
        let conflicts = self
            .entries
            .iter()
            .filter_map(|(key, entry)| match &entry.save_state {
                CatalogSaveState::Conflict(actual) => Some((*key, actual.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (key, actual) in &conflicts {
            if let Some(actual) = actual {
                self.entries.insert(
                    *key,
                    CatalogEditEntry {
                        base: Some(actual.clone()),
                        current: Some(actual.clone()),
                        save_state: CatalogSaveState::Idle,
                    },
                );
            } else {
                self.entries.remove(key);
            }
        }
        if !conflicts.is_empty() {
            self.mark_runtime_diverged();
        }
        conflicts.len()
    }

    pub(crate) fn keep_local_conflicts(&mut self) -> usize {
        let mut resolved = 0;
        let mut remove = Vec::new();
        for (key, entry) in &mut self.entries {
            let CatalogSaveState::Conflict(actual) = &entry.save_state else {
                continue;
            };
            match (entry.base.is_some(), actual.clone(), entry.current.as_mut()) {
                (true, Some(actual), Some(current)) if actual.key() == current.key() => {
                    set_catalog_revision(current, actual.source_revision());
                    entry.base = Some(actual);
                }
                (true, Some(actual), None) => {
                    entry.base = Some(actual);
                }
                (true, None, Some(current)) => {
                    set_catalog_revision(current, 0);
                    entry.base = None;
                }
                (true, None, None) => {
                    remove.push(*key);
                }
                _ => continue,
            }
            entry.save_state = CatalogSaveState::Idle;
            resolved += 1;
        }
        for key in remove {
            self.entries.remove(&key);
        }
        if resolved > 0 {
            self.mark_runtime_diverged();
        }
        resolved
    }

    pub(crate) fn dismiss_failures(&mut self) -> bool {
        let mut dismissed = false;
        for entry in self.entries.values_mut() {
            if matches!(
                entry.save_state,
                CatalogSaveState::BlockedByDependency(_) | CatalogSaveState::Failed(_)
            ) {
                entry.save_state = CatalogSaveState::Idle;
                dismissed = true;
            }
        }
        dismissed
    }

    fn reconcile(&mut self, project: &ProjectEditorStore) {
        if self.last_completed_query == project.completed_queries() {
            return;
        }
        self.last_completed_query = project.completed_queries();
        let fresh = project
            .ground_cover_visuals()
            .iter()
            .cloned()
            .map(GroundCoverCatalogRecord::Visual)
            .chain(
                project
                    .ground_cover_presets()
                    .iter()
                    .cloned()
                    .map(GroundCoverCatalogRecord::Preset),
            )
            .collect::<Vec<_>>();
        let fresh_keys = fresh
            .iter()
            .map(GroundCoverCatalogRecord::key)
            .collect::<HashSet<_>>();
        self.entries
            .retain(|key, entry| entry.pinned() || fresh_keys.contains(key));
        for record in fresh {
            let key = record.key();
            match self.entries.get_mut(&key) {
                Some(entry) if !entry.pinned() => {
                    entry.base = Some(record.clone());
                    entry.current = Some(record);
                    entry.save_state = CatalogSaveState::Idle;
                }
                Some(_) => {}
                None => {
                    self.entries.insert(
                        key,
                        CatalogEditEntry {
                            base: Some(record.clone()),
                            current: Some(record),
                            save_state: CatalogSaveState::Idle,
                        },
                    );
                }
            }
        }
    }

    fn finish_save(&mut self, request_id: u64, outcome: CatalogSaveOutcome) {
        let matching = self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                matches!(entry.save_state, CatalogSaveState::Saving(request) if request == request_id)
                    .then_some(*key)
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return;
        }
        match outcome {
            CatalogSaveOutcome::Committed(commits) => {
                for commit in commits {
                    match commit {
                        GroundCoverCatalogWriteCommit::Created(record)
                        | GroundCoverCatalogWriteCommit::Updated(record) => {
                            if let Some(entry) = self.entries.get_mut(&record.key()) {
                                entry.base = Some(record.clone());
                                entry.current = Some(record);
                                entry.save_state = CatalogSaveState::Idle;
                            }
                        }
                        GroundCoverCatalogWriteCommit::Deleted(key) => {
                            self.entries.remove(&key);
                        }
                    }
                }
            }
            CatalogSaveOutcome::Conflict { key, actual } => {
                for matching_key in matching {
                    if let Some(entry) = self.entries.get_mut(&matching_key) {
                        entry.save_state = if matching_key == key {
                            CatalogSaveState::Conflict(actual.clone())
                        } else {
                            CatalogSaveState::Idle
                        };
                    }
                }
            }
            CatalogSaveOutcome::BlockedByDependency { key, dependency } => {
                for matching_key in matching {
                    if let Some(entry) = self.entries.get_mut(&matching_key) {
                        entry.save_state = if matching_key == key {
                            CatalogSaveState::BlockedByDependency(dependency)
                        } else {
                            CatalogSaveState::Idle
                        };
                    }
                }
            }
            CatalogSaveOutcome::Failed(error) => {
                for key in matching {
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.save_state = CatalogSaveState::Failed(error.clone());
                    }
                }
            }
        }
        self.bump_revision();
    }

    fn bump_revision(&mut self) {
        self.edit_revision = self.edit_revision.wrapping_add(1).max(1);
    }

    fn mark_runtime_diverged(&mut self) {
        self.runtime_diverged = true;
        self.bump_revision();
    }
}

pub(crate) fn reconcile_ground_cover_catalog(
    mut catalog: ResMut<GroundCoverCatalogWorkingSet>,
    project: Res<ProjectEditorStore>,
) {
    catalog.reconcile(&project);
}

pub(crate) fn process_catalog_save_completion(
    mut catalog: ResMut<GroundCoverCatalogWorkingSet>,
    mut project: ResMut<ProjectEditorStore>,
    mut coordinator: ResMut<EditorSaveCoordinator>,
) {
    let Some(completion) = project.take_catalog_save_completion() else {
        return;
    };
    let committed = matches!(&completion.outcome, CatalogSaveOutcome::Committed(_));
    catalog.finish_save(completion.request_id, completion.outcome);
    coordinator.transaction_finished(committed);
}

fn valid_catalog_record(record: &GroundCoverCatalogRecord) -> bool {
    match record {
        GroundCoverCatalogRecord::Preset(record) => {
            !record.key.trim().is_empty()
                && !record.display_name.trim().is_empty()
                && record.density_per_square_meter.is_finite()
                && record.density_per_square_meter > 0.0
                && record.source_revision >= 0
        }
        GroundCoverCatalogRecord::Visual(record) => {
            let SourceGroundCoverVisualDefinition::CardCluster(card) = &record.definition;
            !record.key.trim().is_empty()
                && !record.display_name.trim().is_empty()
                && record.source_revision >= 0
                && card.built_in_atlas_version == 1
                && card
                    .procedural_recipe
                    .is_none_or(world::GroundCoverBladeRecipe::is_valid)
                && card
                    .bottom_color
                    .into_iter()
                    .chain(card.top_color)
                    .all(|component| component.is_finite() && (0.0..=1.0).contains(&component))
                && card.minimum_card_height.is_finite()
                && card.minimum_card_height > 0.0
                && card.maximum_card_height.is_finite()
                && card.maximum_card_height >= card.minimum_card_height
                && card.minimum_card_width.is_finite()
                && card.minimum_card_width > 0.0
                && card.maximum_card_width.is_finite()
                && card.maximum_card_width >= card.minimum_card_width
                && card.flattened_card_probability.is_finite()
                && (0.0..=1.0).contains(&card.flattened_card_probability)
                && card.maximum_wind_displacement.is_finite()
                && card.maximum_wind_displacement >= 0.0
        }
    }
}

fn catalog_sort_key(key: GroundCoverCatalogKey) -> (u8, [u8; 16]) {
    match key {
        GroundCoverCatalogKey::Visual(id) => (0, id.0),
        GroundCoverCatalogKey::Preset(id) => (1, id.0),
    }
}

fn catalog_write_sort_key(key: GroundCoverCatalogKey, phase: CatalogSavePhase) -> (u8, [u8; 16]) {
    match (phase, key) {
        (CatalogSavePhase::Upserts, GroundCoverCatalogKey::Visual(id)) => (0, id.0),
        (CatalogSavePhase::Upserts, GroundCoverCatalogKey::Preset(id)) => (1, id.0),
        (CatalogSavePhase::Deletions, GroundCoverCatalogKey::Preset(id)) => (0, id.0),
        (CatalogSavePhase::Deletions, GroundCoverCatalogKey::Visual(id)) => (1, id.0),
    }
}

fn set_catalog_revision(record: &mut GroundCoverCatalogRecord, source_revision: i64) {
    match record {
        GroundCoverCatalogRecord::Visual(record) => record.source_revision = source_revision,
        GroundCoverCatalogRecord::Preset(record) => record.source_revision = source_revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset() -> SourceGroundCoverPresetRecord {
        SourceGroundCoverPresetRecord {
            id: GroundCoverPresetId([1; 16]),
            key: "meadow".into(),
            display_name: "Meadow".into(),
            enabled: true,
            visual: GroundCoverVisualId([2; 16]),
            density_per_square_meter: 5.0,
            seed: 7,
            source_revision: 3,
        }
    }

    #[test]
    fn catalog_snapshot_restores_an_existing_definition_update() {
        let base = preset();
        let mut current = base.clone();
        current.density_per_square_meter = 8.0;
        let mut catalog = GroundCoverCatalogWorkingSet::default();
        assert!(catalog.restore_dirty_snapshot(DirtyCatalogSnapshot {
            base: Some(GroundCoverCatalogRecord::Preset(base)),
            current: Some(GroundCoverCatalogRecord::Preset(current.clone())),
        }));
        assert_eq!(catalog.dirty_count(), 1);
        assert_eq!(
            catalog
                .current_preset(&ProjectEditorStore::default(), current.id)
                .unwrap()
                .density_per_square_meter,
            8.0
        );
    }

    #[test]
    fn catalog_snapshots_restore_creation_and_deletion_tombstones() {
        let created = preset();
        let mut catalog = GroundCoverCatalogWorkingSet::default();
        assert!(catalog.restore_dirty_snapshot(DirtyCatalogSnapshot {
            base: None,
            current: Some(GroundCoverCatalogRecord::Preset(created.clone())),
        }));
        assert_eq!(catalog.dirty_upsert_count(), 1);

        let deleted = SourceGroundCoverPresetRecord {
            id: GroundCoverPresetId([3; 16]),
            ..created
        };
        assert!(catalog.restore_dirty_snapshot(DirtyCatalogSnapshot {
            base: Some(GroundCoverCatalogRecord::Preset(deleted.clone())),
            current: None,
        }));
        assert_eq!(catalog.dirty_deletion_count(), 1);
        assert!(
            catalog
                .current_preset(&ProjectEditorStore::default(), deleted.id)
                .is_none()
        );
    }

    #[test]
    fn replaying_old_command_values_keeps_the_latest_saved_revision() {
        let before = preset();
        let mut after = before.clone();
        after.density_per_square_meter = 8.0;
        let key = GroundCoverCatalogKey::Preset(before.id);
        let mut catalog = GroundCoverCatalogWorkingSet::default();
        catalog.entries.insert(
            key,
            CatalogEditEntry {
                base: Some(GroundCoverCatalogRecord::Preset(before.clone())),
                current: Some(GroundCoverCatalogRecord::Preset(after)),
                save_state: CatalogSaveState::Saving(9),
            },
        );
        let mut committed = before.clone();
        committed.density_per_square_meter = 8.0;
        committed.source_revision = 4;
        catalog.finish_save(
            9,
            CatalogSaveOutcome::Committed(vec![GroundCoverCatalogWriteCommit::Updated(
                GroundCoverCatalogRecord::Preset(committed),
            )]),
        );

        assert!(catalog.replace_preset(before));
        let Some(GroundCoverCatalogRecord::Preset(current)) = &catalog.entries[&key].current else {
            panic!("expected preset")
        };
        assert_eq!(current.source_revision, 4);
        assert_eq!(current.density_per_square_meter, 5.0);
    }

    #[test]
    fn saving_a_catalog_edit_keeps_preview_diverged_until_publication_adoption() {
        let record = preset();
        let key = GroundCoverCatalogKey::Preset(record.id);
        let mut catalog = GroundCoverCatalogWorkingSet::default();
        assert!(catalog.create_preset(record.clone()));
        catalog.entries.get_mut(&key).unwrap().save_state = CatalogSaveState::Saving(12);
        let mut committed = record;
        committed.source_revision += 1;

        catalog.finish_save(
            12,
            CatalogSaveOutcome::Committed(vec![GroundCoverCatalogWriteCommit::Created(
                GroundCoverCatalogRecord::Preset(committed),
            )]),
        );
        assert_eq!(catalog.dirty_count(), 0);
        assert!(catalog.runtime_diverged());
        assert!(catalog.adopt_runtime_generation());
        assert!(!catalog.runtime_diverged());
    }
}
