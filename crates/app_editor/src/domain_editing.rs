//! Bounded terrain and ground-cover source working sets.
//!
//! Dense records are keyed by domain/cell, keep database checkpoints, local values, and the cooked
//! runtime baseline separate, and survive spatial-query eviction while dirty, saving, or newer
//! than the active runtime generation. Ground-cover brushes add compact reversible rectangles
//! without changing this persistence boundary; terrain will use the same ownership model.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use world::{CellCoord, GroundCoverRegionId, WorldSpaceId};
use world_db::{
    DenseSourceRecord, DenseSourceRecordKey, DenseSourceWrite,
    MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION, SourceGroundCoverCellMaskRecord,
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

/// The smallest axis-aligned byte rectangle changed by one mask gesture.
///
/// Patches keep history proportional to the painted area rather than retaining whole cells for
/// every command. `created` lets redo reconstruct a sparse row that did not exist before the
/// gesture; undo removes that row again while it is still unsaved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroundCoverMaskPatch {
    pub(crate) region: GroundCoverRegionId,
    pub(crate) space: WorldSpaceId,
    pub(crate) cell: CellCoord,
    pub(crate) resolution: u8,
    pub(crate) minimum: [u8; 2],
    pub(crate) size: [u8; 2],
    pub(crate) before: Vec<u8>,
    pub(crate) after: Vec<u8>,
    pub(crate) created: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DirtyDenseSnapshot {
    pub(crate) base: Option<DenseSourceRecord>,
    pub(crate) current: DenseSourceRecord,
    pub(crate) runtime: Option<DenseSourceRecord>,
}

impl GroundCoverMaskPatch {
    pub(crate) fn between(
        before: Option<&SourceGroundCoverCellMaskRecord>,
        after: &SourceGroundCoverCellMaskRecord,
    ) -> Option<Self> {
        let resolution = usize::from(after.resolution);
        if resolution == 0 || after.coverage.len() != resolution * resolution {
            return None;
        }
        let created = before.is_none();
        let before_coverage = before.map_or_else(
            || vec![0; after.coverage.len()],
            |record| {
                if record.region == after.region
                    && record.space == after.space
                    && record.cell == after.cell
                    && record.resolution == after.resolution
                    && record.coverage.len() == after.coverage.len()
                {
                    record.coverage.clone()
                } else {
                    Vec::new()
                }
            },
        );
        if before_coverage.len() != after.coverage.len() {
            return None;
        }

        let mut minimum = [resolution, resolution];
        let mut maximum = [0usize, 0usize];
        let mut changed = false;
        for z in 0..resolution {
            for x in 0..resolution {
                let index = z * resolution + x;
                if before_coverage[index] == after.coverage[index] {
                    continue;
                }
                changed = true;
                minimum[0] = minimum[0].min(x);
                minimum[1] = minimum[1].min(z);
                maximum[0] = maximum[0].max(x);
                maximum[1] = maximum[1].max(z);
            }
        }
        if !changed {
            return None;
        }
        let size = [maximum[0] - minimum[0] + 1, maximum[1] - minimum[1] + 1];
        let extract = |coverage: &[u8]| {
            let mut bytes = Vec::with_capacity(size[0] * size[1]);
            for z in minimum[1]..minimum[1] + size[1] {
                let start = z * resolution + minimum[0];
                bytes.extend_from_slice(&coverage[start..start + size[0]]);
            }
            bytes
        };
        Some(Self {
            region: after.region,
            space: after.space,
            cell: after.cell,
            resolution: after.resolution,
            minimum: [minimum[0] as u8, minimum[1] as u8],
            size: [size[0] as u8, size[1] as u8],
            before: extract(&before_coverage),
            after: extract(&after.coverage),
            created,
        })
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.before.len() + self.after.len()
    }

    fn key(&self) -> DenseSourceRecordKey {
        DenseSourceRecordKey::GroundCoverMask {
            region: self.region,
            space: self.space,
            cell: self.cell,
        }
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

    pub(crate) fn ground_cover_cell_diverges(&self, space: WorldSpaceId, cell: CellCoord) -> bool {
        self.entries.values().any(|entry| {
            matches!(
                &entry.current,
                DenseSourceRecord::GroundCoverMask(record)
                    if record.space == space && record.cell == cell
            ) && entry.runtime_diverged()
        })
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

    pub(crate) fn ground_cover_mask(
        &self,
        region: GroundCoverRegionId,
        space: WorldSpaceId,
        cell: CellCoord,
    ) -> Option<SourceGroundCoverCellMaskRecord> {
        let key = DenseSourceRecordKey::GroundCoverMask {
            region,
            space,
            cell,
        };
        self.entries
            .get(&key)
            .and_then(|entry| match &entry.current {
                DenseSourceRecord::GroundCoverMask(record) => Some(record.clone()),
                DenseSourceRecord::TerrainWeights(_) => None,
            })
    }

    pub(crate) fn ground_cover_region_resolution(&self, region: GroundCoverRegionId) -> Option<u8> {
        self.entries
            .values()
            .find_map(|entry| match &entry.current {
                DenseSourceRecord::GroundCoverMask(record) if record.region == region => {
                    Some(record.resolution)
                }
                _ => None,
            })
    }

    pub(crate) fn ground_cover_region_has_coverage(&self, region: GroundCoverRegionId) -> bool {
        self.entries.values().any(|entry| {
            matches!(
                &entry.current,
                DenseSourceRecord::GroundCoverMask(record) if record.region == region
            )
        })
    }

    /// Replaces one complete bounded mask after a brush sample has edited its bytes. Missing rows
    /// are created only for non-empty coverage, preserving the sparse source representation.
    pub(crate) fn set_ground_cover_coverage(
        &mut self,
        region: GroundCoverRegionId,
        space: WorldSpaceId,
        cell: CellCoord,
        resolution: u8,
        coverage: Vec<u8>,
    ) -> bool {
        let expected_len = usize::from(resolution).pow(2);
        if resolution == 0 || coverage.len() != expected_len {
            return false;
        }
        let key = DenseSourceRecordKey::GroundCoverMask {
            region,
            space,
            cell,
        };
        if let Some(entry) = self.entries.get_mut(&key) {
            let DenseSourceRecord::GroundCoverMask(current) = &entry.current else {
                return false;
            };
            if current.resolution != resolution
                || current.coverage == coverage
                || matches!(entry.save_state, DenseSaveState::Saving(_))
            {
                return false;
            }
            let mut replacement = current.clone();
            replacement.coverage = coverage;
            entry.current = DenseSourceRecord::GroundCoverMask(replacement);
            entry.save_state = DenseSaveState::Idle;
            self.bump_revision();
            return true;
        }
        if coverage.iter().all(|value| *value == 0) {
            return false;
        }
        self.entries.insert(
            key,
            DenseEditEntry {
                base: None,
                current: DenseSourceRecord::GroundCoverMask(SourceGroundCoverCellMaskRecord {
                    region,
                    space,
                    cell,
                    resolution,
                    coverage,
                    source_revision: 0,
                }),
                runtime: None,
                save_state: DenseSaveState::Idle,
            },
        );
        self.bump_revision();
        true
    }

    /// Applies all rectangles atomically enough for command history: every target is validated
    /// before any byte is changed, then all changes share one working-set revision bump.
    pub(crate) fn apply_ground_cover_patches(
        &mut self,
        patches: &[GroundCoverMaskPatch],
        forward: bool,
    ) -> bool {
        if patches.is_empty()
            || !patches
                .iter()
                .all(|patch| self.can_apply_patch(patch, forward))
        {
            return false;
        }
        for patch in patches {
            let key = patch.key();
            if !self.entries.contains_key(&key) {
                self.entries.insert(
                    key.clone(),
                    DenseEditEntry {
                        base: None,
                        current: DenseSourceRecord::GroundCoverMask(
                            SourceGroundCoverCellMaskRecord {
                                region: patch.region,
                                space: patch.space,
                                cell: patch.cell,
                                resolution: patch.resolution,
                                coverage: vec![0; usize::from(patch.resolution).pow(2)],
                                source_revision: 0,
                            },
                        ),
                        runtime: None,
                        save_state: DenseSaveState::Idle,
                    },
                );
            }
            let entry = self
                .entries
                .get_mut(&key)
                .expect("validated patch target exists");
            let DenseSourceRecord::GroundCoverMask(record) = &mut entry.current else {
                unreachable!("ground-cover keys only address ground-cover records")
            };
            let bytes = if forward { &patch.after } else { &patch.before };
            write_patch_bytes(record, patch, bytes);
            entry.save_state = DenseSaveState::Idle;
            let remove_sparse_row = !forward
                && patch.created
                && entry.base.is_none()
                && record.coverage.iter().all(|value| *value == 0);
            if remove_sparse_row {
                self.entries.remove(&key);
            }
        }
        self.bump_revision();
        true
    }

    fn can_apply_patch(&self, patch: &GroundCoverMaskPatch, forward: bool) -> bool {
        let resolution = usize::from(patch.resolution);
        let minimum = [usize::from(patch.minimum[0]), usize::from(patch.minimum[1])];
        let size = [usize::from(patch.size[0]), usize::from(patch.size[1])];
        if resolution == 0
            || size[0] == 0
            || size[1] == 0
            || minimum[0] + size[0] > resolution
            || minimum[1] + size[1] > resolution
            || patch.before.len() != size[0] * size[1]
            || patch.after.len() != size[0] * size[1]
        {
            return false;
        }
        let Some(entry) = self.entries.get(&patch.key()) else {
            return forward && patch.created;
        };
        matches!(
            &entry.current,
            DenseSourceRecord::GroundCoverMask(record)
                if record.resolution == patch.resolution
                    && record.coverage.len() == resolution * resolution
                    && !matches!(entry.save_state, DenseSaveState::Saving(_))
        )
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
        DenseSourceRecord::GroundCoverMask(record) => DenseSourceRecordKey::GroundCoverMask {
            region: record.region,
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
        DenseSourceRecord::GroundCoverMask(record) => {
            let resolution = usize::from(record.resolution);
            record.source_revision >= 0
                && resolution > 0
                && resolution.checked_mul(resolution) == Some(record.coverage.len())
        }
    }
}

fn dense_source_revision(record: &DenseSourceRecord) -> i64 {
    match record {
        DenseSourceRecord::TerrainWeights(record) => record.source_revision,
        DenseSourceRecord::GroundCoverMask(record) => record.source_revision,
    }
}

fn set_dense_revision(record: &mut DenseSourceRecord, revision: i64) {
    match record {
        DenseSourceRecord::TerrainWeights(record) => record.source_revision = revision,
        DenseSourceRecord::GroundCoverMask(record) => record.source_revision = revision,
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
        (DenseSourceRecord::GroundCoverMask(left), DenseSourceRecord::GroundCoverMask(right)) => {
            left.region == right.region
                && left.space == right.space
                && left.cell == right.cell
                && left.resolution == right.resolution
        }
        _ => false,
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
        (
            Some(DenseSourceRecord::GroundCoverMask(base)),
            DenseSourceRecord::GroundCoverMask(current),
        ) => DenseSourceWrite::GroundCoverMask {
            expected_source_revision: Some(base.source_revision),
            record: current.clone(),
        },
        (None, DenseSourceRecord::TerrainWeights(current)) => DenseSourceWrite::TerrainWeights {
            expected_source_revision: None,
            record: current.clone(),
        },
        (None, DenseSourceRecord::GroundCoverMask(current)) => DenseSourceWrite::GroundCoverMask {
            expected_source_revision: None,
            record: current.clone(),
        },
        _ => unreachable!("a dense working-set entry cannot change domain identity"),
    }
}

fn write_patch_bytes(
    record: &mut SourceGroundCoverCellMaskRecord,
    patch: &GroundCoverMaskPatch,
    bytes: &[u8],
) {
    let resolution = usize::from(record.resolution);
    let minimum_x = usize::from(patch.minimum[0]);
    let minimum_z = usize::from(patch.minimum[1]);
    let width = usize::from(patch.size[0]);
    let height = usize::from(patch.size[1]);
    for row in 0..height {
        let destination = (minimum_z + row) * resolution + minimum_x;
        let source = row * width;
        record.coverage[destination..destination + width]
            .copy_from_slice(&bytes[source..source + width]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::GroundCoverRegionId;
    use world_db::{SourceGroundCoverCellMaskRecord, SourceTerrainCellWeightPageRecord};

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
                base: Some(base.clone()),
                current: base.clone(),
                runtime: Some(base.clone()),
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
                    base: Some(base.clone()),
                    current,
                    runtime: Some(base),
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

    #[test]
    fn saved_ground_cover_remains_divergent_from_the_immutable_runtime_baseline() {
        let base = DenseSourceRecord::GroundCoverMask(SourceGroundCoverCellMaskRecord {
            region: GroundCoverRegionId([7; 16]),
            space: WorldSpaceId(1),
            cell: CellCoord { x: 3, z: -2 },
            resolution: 1,
            coverage: vec![0],
            source_revision: 1,
        });
        let key = dense_record_key(&base);
        let mut working = DenseDomainWorkingSets::default();
        working.entries.insert(
            key.clone(),
            DenseEditEntry {
                base: Some(base.clone()),
                current: base.clone(),
                runtime: Some(base.clone()),
                save_state: DenseSaveState::Idle,
            },
        );

        let mut changed = base.clone();
        let DenseSourceRecord::GroundCoverMask(mask) = &mut changed else {
            unreachable!()
        };
        mask.coverage[0] = 255;
        assert!(working.replace_record(changed.clone()));
        assert!(working.ground_cover_cell_diverges(WorldSpaceId(1), CellCoord { x: 3, z: -2 }));

        assert!(working.replace_record(base.clone()));
        assert!(!working.ground_cover_cell_diverges(WorldSpaceId(1), CellCoord { x: 3, z: -2 }));

        assert!(working.replace_record(changed.clone()));
        working.entries.get_mut(&key).unwrap().save_state = DenseSaveState::Saving(9);
        let mut committed = changed;
        let DenseSourceRecord::GroundCoverMask(mask) = &mut committed else {
            unreachable!()
        };
        mask.source_revision = 2;
        working.finish_save(9, DenseSaveOutcome::Committed(vec![committed]));
        let entry = &working.entries[&key];
        assert!(!entry.dirty());
        assert!(entry.runtime_diverged());
        assert!(entry.pinned());
        assert_eq!(working.adopt_runtime_generation(), 1);
        let entry = &working.entries[&key];
        assert!(!entry.runtime_diverged());
        assert!(!entry.pinned());
    }

    #[test]
    fn ground_cover_history_patch_retains_only_the_changed_rectangle() {
        let before = SourceGroundCoverCellMaskRecord {
            region: GroundCoverRegionId([4; 16]),
            space: WorldSpaceId(2),
            cell: CellCoord { x: -1, z: 3 },
            resolution: 4,
            coverage: vec![0; 16],
            source_revision: 7,
        };
        let mut after = before.clone();
        after.coverage[5] = 64;
        after.coverage[10] = 255;

        let patch = GroundCoverMaskPatch::between(Some(&before), &after).unwrap();
        assert_eq!(patch.minimum, [1, 1]);
        assert_eq!(patch.size, [2, 2]);
        assert_eq!(patch.before.len(), 4);
        assert_eq!(patch.after, vec![64, 0, 0, 255]);
        assert!(!patch.created);
    }

    #[test]
    fn dirty_dense_snapshot_restores_sparse_intent_and_runtime_baseline() {
        let region = GroundCoverRegionId([5; 16]);
        let space = WorldSpaceId(3);
        let cell = CellCoord { x: 8, z: -6 };
        let snapshot = DirtyDenseSnapshot {
            base: None,
            current: DenseSourceRecord::GroundCoverMask(SourceGroundCoverCellMaskRecord {
                region,
                space,
                cell,
                resolution: 2,
                coverage: vec![0, 255, 0, 0],
                source_revision: 0,
            }),
            runtime: None,
        };
        let mut restored = DenseDomainWorkingSets::default();
        assert!(restored.restore_dirty_snapshot(snapshot));
        assert_eq!(restored.dirty_count(), 1);
        assert_eq!(
            restored
                .ground_cover_mask(region, space, cell)
                .unwrap()
                .coverage,
            vec![0, 255, 0, 0]
        );
        assert_eq!(restored.dirty_snapshots().len(), 1);
    }

    #[test]
    fn dense_conflicts_can_rebase_local_bytes_or_accept_database_bytes() {
        let base = DenseSourceRecord::GroundCoverMask(SourceGroundCoverCellMaskRecord {
            region: GroundCoverRegionId([6; 16]),
            space: WorldSpaceId(4),
            cell: CellCoord::ZERO,
            resolution: 2,
            coverage: vec![0; 4],
            source_revision: 1,
        });
        let mut local = base.clone();
        let DenseSourceRecord::GroundCoverMask(local_mask) = &mut local else {
            unreachable!()
        };
        local_mask.coverage[0] = 255;
        let mut actual = base.clone();
        let DenseSourceRecord::GroundCoverMask(actual_mask) = &mut actual else {
            unreachable!()
        };
        actual_mask.coverage[1] = 128;
        actual_mask.source_revision = 2;

        let key = dense_record_key(&base);
        let mut working = DenseDomainWorkingSets::default();
        working.entries.insert(
            key.clone(),
            DenseEditEntry {
                base: Some(base.clone()),
                current: local.clone(),
                runtime: Some(base),
                save_state: DenseSaveState::Conflict(Some(actual.clone())),
            },
        );
        assert_eq!(working.conflict_count(), 1);
        assert_eq!(working.keep_local_conflicts(), 1);
        let DenseSourceRecord::GroundCoverMask(rebased) = &working.entries[&key].current else {
            unreachable!()
        };
        assert_eq!(rebased.coverage, vec![255, 0, 0, 0]);
        assert_eq!(rebased.source_revision, 2);
        assert!(working.entries[&key].dirty());

        let mut newest = actual;
        let DenseSourceRecord::GroundCoverMask(newest_mask) = &mut newest else {
            unreachable!()
        };
        newest_mask.coverage = vec![0, 0, 200, 0];
        newest_mask.source_revision = 3;
        working.entries.get_mut(&key).unwrap().save_state =
            DenseSaveState::Conflict(Some(newest.clone()));
        assert!(working.can_accept_database_conflicts());
        assert_eq!(working.accept_database_conflicts(), 1);
        assert_eq!(working.entries[&key].current, newest);
        assert!(!working.entries[&key].dirty());
    }
}
