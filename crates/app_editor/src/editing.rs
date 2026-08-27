//! Stable-ID editor working state and command history layered over the bounded source cache.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    f32::consts::TAU,
};

use bevy::prelude::*;
use world::{StableObjectId, WorldPosition};
use world_db::{
    MAX_OBJECT_WRITES_PER_TRANSACTION, SourceObjectRecord, SourceObjectTransform,
    SourceObjectViewRecord, SourceObjectWrite, SourceObjectWriteCommit,
};

use crate::catalog_editing::GroundCoverRegionWorkingSet;
use crate::domain_editing::{DenseDomainWorkingSets, GroundCoverMaskPatch};
use crate::ground_cover_catalog::GroundCoverCatalogWorkingSet;
use crate::project_store::{ObjectSaveOutcome, ProjectEditorStore};
use crate::saving::EditorSaveCoordinator;

const MAX_HISTORY_ENTRIES: usize = 128;
const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
enum EditorSaveState {
    #[default]
    Idle,
    Saving(u64),
    Saved(i64),
    Conflict(Option<SourceObjectRecord>),
    Failed(String),
}

#[derive(Debug, Clone)]
struct ObjectEditEntry {
    presentation: SourceObjectViewRecord,
    /// The last source state accepted as a save/reconciliation checkpoint.
    base: Option<SourceObjectRecord>,
    /// The state produced by the local command stream. `None` is a deletion tombstone.
    current: Option<SourceObjectRecord>,
    /// The source state represented by the currently loaded cooked runtime database.
    runtime: Option<SourceObjectRecord>,
    save_state: EditorSaveState,
}

impl ObjectEditEntry {
    fn from_source(record: SourceObjectViewRecord) -> Self {
        let object = record.object.clone();
        Self {
            presentation: record,
            base: Some(object.clone()),
            current: Some(object.clone()),
            runtime: Some(object),
            save_state: EditorSaveState::Idle,
        }
    }

    fn from_created(record: SourceObjectViewRecord) -> Self {
        let object = record.object.clone();
        Self {
            presentation: record,
            base: None,
            current: Some(object),
            runtime: None,
            save_state: EditorSaveState::Idle,
        }
    }

    fn dirty(&self) -> bool {
        self.current != self.base
    }

    fn runtime_differs(&self) -> bool {
        self.current != self.runtime
    }

    fn view(&self) -> Option<SourceObjectViewRecord> {
        self.current.as_ref().map(|object| {
            let mut view = self.presentation.clone();
            view.object = object.clone();
            view
        })
    }

    fn has_conflict(&self) -> bool {
        matches!(self.save_state, EditorSaveState::Conflict(_))
    }
}

#[derive(Debug, Clone)]
struct ActiveObjectSave {
    request_id: u64,
    objects: Vec<StableObjectId>,
}

#[derive(Debug, Clone)]
pub(crate) struct DirtyObjectSnapshot {
    pub(crate) presentation: SourceObjectViewRecord,
    pub(crate) base: Option<SourceObjectRecord>,
    pub(crate) current: Option<SourceObjectRecord>,
}

/// Durable editor-side state for every object touched by the current session.
///
/// Entries are keyed by stable source ID and outlive selection and bounded source-query windows.
/// This is intentionally separate from runtime ECS entities, which may stream in and out.
#[derive(Resource, Default)]
pub(crate) struct EditorObjectWorkingSet {
    entries: HashMap<StableObjectId, ObjectEditEntry>,
    edit_revision: u64,
    active_save: Option<ActiveObjectSave>,
}

impl EditorObjectWorkingSet {
    pub(crate) fn dirty_snapshots(&self) -> Vec<DirtyObjectSnapshot> {
        let mut snapshots = self
            .entries
            .values()
            .filter(|entry| entry.dirty())
            .map(|entry| DirtyObjectSnapshot {
                presentation: entry.presentation.clone(),
                base: entry.base.clone(),
                current: entry.current.clone(),
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.presentation.object.id.0);
        snapshots
    }

    pub(crate) fn restore_dirty_snapshot(&mut self, snapshot: DirtyObjectSnapshot) -> bool {
        if self.saving() {
            return false;
        }
        let id = snapshot.presentation.object.id;
        let definition = snapshot.presentation.object.definition;
        if snapshot
            .base
            .iter()
            .chain(snapshot.current.iter())
            .any(|object| object.id != id || object.definition != definition)
            || snapshot.base == snapshot.current
        {
            return false;
        }
        if self.entries.get(&id).is_some_and(|entry| {
            entry.dirty() || matches!(entry.save_state, EditorSaveState::Saving(_))
        }) {
            return false;
        }
        self.entries.insert(
            id,
            ObjectEditEntry {
                presentation: snapshot.presentation,
                runtime: snapshot.base.clone(),
                base: snapshot.base,
                current: snapshot.current,
                save_state: EditorSaveState::Idle,
            },
        );
        self.bump_edit_revision();
        true
    }

    pub(crate) fn tracks(&self, object: StableObjectId) -> bool {
        self.entries.contains_key(&object)
    }

    pub(crate) fn pin(&mut self, fresh: SourceObjectViewRecord) {
        let id = fresh.object.id;
        let mut changed = false;
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.presentation.definition = fresh.definition;
            entry.presentation.visual_uri = fresh.visual_uri;
            entry.presentation.visual_bounds = fresh.visual_bounds;
            if !entry.dirty()
                && !matches!(entry.save_state, EditorSaveState::Saving(_))
                && entry
                    .base
                    .as_ref()
                    .is_none_or(|base| fresh.object.source_revision > base.source_revision)
            {
                entry.base = Some(fresh.object.clone());
                entry.current = Some(fresh.object);
                entry.save_state = EditorSaveState::Idle;
                changed = true;
            }
        } else {
            self.entries.insert(id, ObjectEditEntry::from_source(fresh));
            changed = true;
        }
        if changed {
            self.bump_edit_revision();
        }
    }

    pub(crate) fn reconcile_source(&mut self, fresh: &SourceObjectViewRecord) {
        self.pin(fresh.clone());
    }

    pub(crate) fn current_view(&self, object: StableObjectId) -> Option<SourceObjectViewRecord> {
        self.entries.get(&object).and_then(ObjectEditEntry::view)
    }

    pub(crate) fn current_views(&self) -> Vec<SourceObjectViewRecord> {
        self.entries
            .values()
            .filter_map(ObjectEditEntry::view)
            .collect()
    }

    pub(crate) fn resolve_source_view(
        &self,
        fresh: &SourceObjectViewRecord,
    ) -> Option<SourceObjectViewRecord> {
        self.entries
            .get(&fresh.object.id)
            .map_or_else(|| Some(fresh.clone()), ObjectEditEntry::view)
    }

    pub(crate) fn desired_proxy_views(
        &self,
        selected: &[StableObjectId],
    ) -> Vec<SourceObjectViewRecord> {
        self.entries
            .iter()
            .filter(|(id, entry)| selected.contains(id) || entry.runtime_differs())
            .filter_map(|(_, entry)| entry.view())
            .collect()
    }

    pub(crate) fn is_deleted(&self, object: StableObjectId) -> bool {
        self.entries
            .get(&object)
            .is_some_and(|entry| entry.current.is_none())
    }

    pub(crate) fn dirty(&self, object: StableObjectId) -> bool {
        self.entries
            .get(&object)
            .is_some_and(ObjectEditEntry::dirty)
    }

    pub(crate) fn dirty_count(&self) -> usize {
        self.entries.values().filter(|entry| entry.dirty()).count()
    }

    pub(crate) fn saving(&self) -> bool {
        self.active_save.is_some()
    }

    pub(crate) fn has_conflict(&self, object: StableObjectId) -> bool {
        self.entries
            .get(&object)
            .is_some_and(ObjectEditEntry::has_conflict)
    }

    pub(crate) fn has_any_conflict(&self) -> bool {
        self.entries.values().any(ObjectEditEntry::has_conflict)
    }

    pub(crate) fn can_edit(&self, object: StableObjectId) -> bool {
        !self.saving()
            && self
                .entries
                .get(&object)
                .is_some_and(|entry| entry.current.is_some() && !entry.has_conflict())
    }

    pub(crate) fn edit_revision(&self) -> u64 {
        self.edit_revision
    }

    /// Advances the cooked baseline to the project checkpoint used by a newly adopted runtime
    /// generation. Commands created while cooking remain divergent from that checkpoint.
    pub(crate) fn adopt_runtime_generation(&mut self) -> usize {
        let mut adopted = 0;
        for entry in self.entries.values_mut() {
            if entry.runtime != entry.base {
                entry.runtime.clone_from(&entry.base);
                adopted += 1;
            }
            if !entry.dirty() && matches!(entry.save_state, EditorSaveState::Saved(_)) {
                entry.save_state = EditorSaveState::Idle;
            }
        }
        if adopted > 0 {
            self.bump_edit_revision();
        }
        adopted
    }

    pub(crate) fn status(&self, object: StableObjectId) -> String {
        let Some(entry) = self.entries.get(&object) else {
            return "Object is not in the editor working set".into();
        };
        match &entry.save_state {
            EditorSaveState::Idle if entry.current.is_none() => "Unsaved deletion command".into(),
            EditorSaveState::Idle if entry.dirty() => "Unsaved object command".into(),
            EditorSaveState::Idle => "No local changes".into(),
            EditorSaveState::Saving(request_id) => {
                format!("Saving object transaction {request_id}…")
            }
            EditorSaveState::Saved(revision) => {
                format!("Saved source revision {revision}; cooked runtime data is unchanged")
            }
            EditorSaveState::Conflict(Some(actual)) => format!(
                "Conflict: database is at revision {}; local command was preserved",
                actual.source_revision
            ),
            EditorSaveState::Conflict(None) => {
                "Conflict: object was deleted; local command was preserved".into()
            }
            EditorSaveState::Failed(error) => format!("Save failed: {error}"),
        }
    }

    pub(crate) fn set_transforms(
        &mut self,
        transforms: &[(StableObjectId, SourceObjectTransform)],
    ) -> bool {
        if self.saving() || transforms.is_empty() {
            return false;
        }
        let mut unique = HashSet::with_capacity(transforms.len());
        let mut changed = false;
        for (object, transform) in transforms {
            if !unique.insert(*object) {
                return false;
            }
            let Some(entry) = self
                .entries
                .get(object)
                .filter(|entry| entry.current.is_some() && !entry.has_conflict())
            else {
                return false;
            };
            let current = entry
                .current
                .as_ref()
                .expect("current object was checked above");
            changed |= SourceObjectTransform::from(current) != *transform;
        }
        if !changed {
            return false;
        }

        for (object, transform) in transforms {
            let entry = self
                .entries
                .get_mut(object)
                .expect("all transformed objects were validated above");
            let current = entry
                .current
                .as_mut()
                .expect("all transformed objects had current source state");
            current.space = transform.space;
            current.owner_cell = transform.owner_cell;
            current.local_translation = transform.local_translation;
            current.yaw = transform.yaw;
            current.scale = transform.scale;
            entry.save_state = EditorSaveState::Idle;
        }
        self.bump_edit_revision();
        true
    }

    fn restore_object(&mut self, object: SourceObjectRecord) -> bool {
        self.restore_objects(&[object])
    }

    fn create_object(&mut self, record: SourceObjectViewRecord) -> bool {
        if self.saving() || self.entries.contains_key(&record.object.id) {
            return false;
        }
        self.entries
            .insert(record.object.id, ObjectEditEntry::from_created(record));
        self.bump_edit_revision();
        true
    }

    fn delete_object(&mut self, object: StableObjectId) -> Option<SourceObjectRecord> {
        self.delete_objects(&[object])?.pop()
    }

    fn restore_objects(&mut self, objects: &[SourceObjectRecord]) -> bool {
        if self.saving() || objects.is_empty() {
            return false;
        }
        let mut unique = HashSet::with_capacity(objects.len());
        if objects.iter().any(|object| {
            !unique.insert(object.id)
                || self
                    .entries
                    .get(&object.id)
                    .is_none_or(|entry| entry.current.is_some() || entry.has_conflict())
        }) {
            return false;
        }
        for object in objects {
            let entry = self
                .entries
                .get_mut(&object.id)
                .expect("all restored objects were validated above");
            entry.current = Some(object.clone());
            entry.save_state = EditorSaveState::Idle;
        }
        self.bump_edit_revision();
        true
    }

    fn delete_objects(&mut self, objects: &[StableObjectId]) -> Option<Vec<SourceObjectRecord>> {
        if self.saving() || objects.is_empty() {
            return None;
        }
        let mut unique = HashSet::with_capacity(objects.len());
        if objects.iter().any(|object| {
            !unique.insert(*object)
                || self
                    .entries
                    .get(object)
                    .is_none_or(|entry| entry.current.is_none() || entry.has_conflict())
        }) {
            return None;
        }
        let mut deleted = Vec::with_capacity(objects.len());
        for object in objects {
            let entry = self
                .entries
                .get_mut(object)
                .expect("all deleted objects were validated above");
            deleted.push(
                entry
                    .current
                    .take()
                    .expect("all deleted objects had current source state"),
            );
            entry.save_state = EditorSaveState::Idle;
        }
        self.bump_edit_revision();
        Some(deleted)
    }

    pub(crate) fn queue_save(&mut self, project: &mut ProjectEditorStore) -> bool {
        if self.saving() || self.has_any_conflict() {
            return false;
        }
        let mut writes = Vec::new();
        let mut objects = Vec::new();
        let mut dirty_entries = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.dirty())
            .collect::<Vec<_>>();
        dirty_entries.sort_by_key(|(id, _)| id.0);
        for (id, entry) in dirty_entries
            .into_iter()
            .take(MAX_OBJECT_WRITES_PER_TRANSACTION)
        {
            let write = match (&entry.base, &entry.current) {
                (Some(base), Some(current)) => SourceObjectWrite::UpdateTransform {
                    object: *id,
                    expected_source_revision: base.source_revision,
                    transform: SourceObjectTransform::from(current),
                },
                (Some(base), None) => SourceObjectWrite::Delete {
                    object: *id,
                    expected_source_revision: base.source_revision,
                },
                (None, Some(current)) => SourceObjectWrite::Create {
                    object: current.clone(),
                },
                (None, None) => continue,
            };
            writes.push(write);
            objects.push(*id);
        }
        let Some(request_id) = project.queue_object_transaction(writes) else {
            return false;
        };
        for object in &objects {
            if let Some(entry) = self.entries.get_mut(object) {
                entry.save_state = EditorSaveState::Saving(request_id);
            }
        }
        self.active_save = Some(ActiveObjectSave {
            request_id,
            objects,
        });
        true
    }

    pub(crate) fn discard_all(&mut self, history: &mut EditorHistory) -> bool {
        if self.saving() || self.dirty_count() == 0 {
            return false;
        }
        for entry in self.entries.values_mut().filter(|entry| entry.dirty()) {
            entry.current.clone_from(&entry.base);
            entry.save_state = EditorSaveState::Idle;
        }
        history.clear();
        self.bump_edit_revision();
        true
    }

    pub(crate) fn reload_conflict(
        &mut self,
        object: StableObjectId,
        history: &mut EditorHistory,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(&object) else {
            return false;
        };
        let EditorSaveState::Conflict(actual) = &entry.save_state else {
            return false;
        };
        entry.base.clone_from(actual);
        entry.current.clone_from(actual);
        if let Some(actual) = actual {
            entry.presentation.object = actual.clone();
        }
        entry.save_state = EditorSaveState::Idle;
        history.clear();
        self.bump_edit_revision();
        true
    }

    /// Accepts the latest database record as the new revision checkpoint while retaining the
    /// local placement intent for a revision-checked retry.
    pub(crate) fn rebase_conflict(
        &mut self,
        object: StableObjectId,
        history: &mut EditorHistory,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(&object) else {
            return false;
        };
        let EditorSaveState::Conflict(actual) = &entry.save_state else {
            return false;
        };
        let actual = actual.clone();
        entry.base.clone_from(&actual);
        if let (Some(current), Some(actual)) = (entry.current.as_mut(), actual.as_ref()) {
            // Object editing does not mutate definition identity. If an improbable UUID collision
            // created the conflict, retry only the local transform against the real definition.
            current.definition = actual.definition;
            entry.presentation.object = actual.clone();
        }
        entry.save_state = EditorSaveState::Idle;
        history.clear();
        self.bump_edit_revision();
        true
    }

    fn finish_save(&mut self, request_id: u64, outcome: ObjectSaveOutcome) {
        if self
            .active_save
            .as_ref()
            .is_none_or(|active| active.request_id != request_id)
        {
            return;
        }
        let active = self
            .active_save
            .take()
            .expect("the matching active save was checked above");
        match outcome {
            ObjectSaveOutcome::Committed(commits) => {
                for commit in commits {
                    match commit {
                        SourceObjectWriteCommit::Updated(object) => {
                            if let Some(entry) = self.entries.get_mut(&object.id) {
                                let revision = object.source_revision;
                                entry.base = Some(object.clone());
                                entry.current = Some(object.clone());
                                entry.presentation.object = object;
                                entry.save_state = EditorSaveState::Saved(revision);
                            }
                        }
                        SourceObjectWriteCommit::Deleted(object) => {
                            if let Some(entry) = self.entries.get_mut(&object) {
                                let revision = entry
                                    .base
                                    .as_ref()
                                    .map_or(0, |base| base.source_revision.saturating_add(1));
                                entry.base = None;
                                entry.current = None;
                                entry.save_state = EditorSaveState::Saved(revision);
                            }
                        }
                    }
                }
            }
            ObjectSaveOutcome::Conflict { object, actual } => {
                for id in active.objects {
                    if let Some(entry) = self.entries.get_mut(&id) {
                        entry.save_state = if id == object {
                            EditorSaveState::Conflict(actual.clone())
                        } else {
                            EditorSaveState::Idle
                        };
                    }
                }
            }
            ObjectSaveOutcome::Failed(error) => {
                for object in active.objects {
                    if let Some(entry) = self.entries.get_mut(&object) {
                        entry.save_state = EditorSaveState::Failed(error.clone());
                    }
                }
            }
        }
        self.bump_edit_revision();
    }

    fn bump_edit_revision(&mut self) {
        self.edit_revision = self.edit_revision.wrapping_add(1).max(1);
    }
}

#[derive(Resource, Default)]
pub(crate) struct EditorSelection {
    selected: Vec<StableObjectId>,
    active: Option<StableObjectId>,
}

impl EditorSelection {
    pub(crate) fn select(
        &mut self,
        record: SourceObjectViewRecord,
        objects: &mut EditorObjectWorkingSet,
    ) -> bool {
        if objects.saving() {
            return false;
        }
        let id = record.object.id;
        objects.pin(record);
        self.selected.clear();
        self.selected.push(id);
        self.active = Some(id);
        true
    }

    pub(crate) fn toggle(
        &mut self,
        record: SourceObjectViewRecord,
        objects: &mut EditorObjectWorkingSet,
    ) -> bool {
        if objects.saving() {
            return false;
        }
        let id = record.object.id;
        objects.pin(record);
        if let Some(index) = self.selected.iter().position(|selected| *selected == id) {
            self.selected.remove(index);
            if self.active == Some(id) {
                self.active = self.selected.last().copied();
            }
        } else {
            self.selected.push(id);
            self.active = Some(id);
        }
        true
    }

    pub(crate) fn select_id(
        &mut self,
        object: StableObjectId,
        objects: &EditorObjectWorkingSet,
    ) -> bool {
        if objects.saving() || objects.current_view(object).is_none() {
            return false;
        }
        self.selected.clear();
        self.selected.push(object);
        self.active = Some(object);
        true
    }

    pub(crate) fn clear(&mut self, objects: &EditorObjectWorkingSet) -> bool {
        if objects.saving() {
            return false;
        }
        self.selected.clear();
        self.active = None;
        true
    }

    pub(crate) fn selected_id(&self) -> Option<StableObjectId> {
        self.active
    }

    pub(crate) fn selected_ids(&self) -> &[StableObjectId] {
        &self.selected
    }

    pub(crate) fn contains(&self, object: StableObjectId) -> bool {
        self.selected.contains(&object)
    }

    pub(crate) fn selected_view(
        &self,
        objects: &EditorObjectWorkingSet,
    ) -> Option<SourceObjectViewRecord> {
        self.active.and_then(|id| objects.current_view(id))
    }

    pub(crate) fn can_change_selection(&self, objects: &EditorObjectWorkingSet) -> bool {
        !objects.saving()
    }

    pub(crate) fn can_edit(&self, objects: &EditorObjectWorkingSet) -> bool {
        self.active.is_some_and(|id| objects.can_edit(id))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ObjectTransformChange {
    object: StableObjectId,
    before: SourceObjectTransform,
    after: SourceObjectTransform,
}

#[derive(Debug, Clone, PartialEq)]
enum EditorCommand {
    Create {
        object: SourceObjectRecord,
    },
    Transform {
        changes: Vec<ObjectTransformChange>,
    },
    Delete {
        objects: Vec<SourceObjectRecord>,
    },
    GroundCover {
        patches: Vec<GroundCoverMaskPatch>,
    },
    CreateRegion {
        region: world_db::SourceGroundCoverRegionRecord,
    },
    DeleteRegion {
        region: world_db::SourceGroundCoverRegionRecord,
    },
    UpdateRegion {
        before: world_db::SourceGroundCoverRegionRecord,
        after: world_db::SourceGroundCoverRegionRecord,
    },
    UpdateGroundCoverPreset {
        before: world_db::SourceGroundCoverPresetRecord,
        after: world_db::SourceGroundCoverPresetRecord,
    },
    UpdateGroundCoverVisual {
        before: world_db::SourceGroundCoverVisualRecord,
        after: world_db::SourceGroundCoverVisualRecord,
    },
    CreateGroundCoverDefinition {
        record: world_db::GroundCoverCatalogRecord,
    },
    DeleteGroundCoverDefinition {
        record: world_db::GroundCoverCatalogRecord,
    },
}

impl EditorCommand {
    fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Create { .. } => std::mem::size_of::<SourceObjectRecord>(),
                Self::Transform { changes } => {
                    changes.len() * std::mem::size_of::<ObjectTransformChange>()
                }
                Self::Delete { objects } => {
                    objects.len() * std::mem::size_of::<SourceObjectRecord>()
                }
                Self::GroundCover { patches } => patches
                    .iter()
                    .map(GroundCoverMaskPatch::estimated_bytes)
                    .sum(),
                Self::CreateRegion { .. } | Self::DeleteRegion { .. } => {
                    std::mem::size_of::<world_db::SourceGroundCoverRegionRecord>()
                }
                Self::UpdateRegion { .. } => {
                    2 * std::mem::size_of::<world_db::SourceGroundCoverRegionRecord>()
                }
                Self::UpdateGroundCoverPreset { before, after } => {
                    2 * std::mem::size_of::<world_db::SourceGroundCoverPresetRecord>()
                        + before.key.len()
                        + before.display_name.len()
                        + after.key.len()
                        + after.display_name.len()
                }
                Self::UpdateGroundCoverVisual { before, after } => {
                    2 * std::mem::size_of::<world_db::SourceGroundCoverVisualRecord>()
                        + before.key.len()
                        + before.display_name.len()
                        + after.key.len()
                        + after.display_name.len()
                }
                Self::CreateGroundCoverDefinition { record }
                | Self::DeleteGroundCoverDefinition { record } => {
                    ground_cover_catalog_record_bytes(record)
                }
            }
    }

    fn apply(
        &self,
        objects: &mut EditorObjectWorkingSet,
        dense_domains: &mut DenseDomainWorkingSets,
        regions: &mut GroundCoverRegionWorkingSet,
        catalog: &mut GroundCoverCatalogWorkingSet,
    ) -> bool {
        match self {
            Self::Create { object } => objects.restore_object(object.clone()),
            Self::Transform { changes } => objects.set_transforms(
                &changes
                    .iter()
                    .map(|change| (change.object, change.after))
                    .collect::<Vec<_>>(),
            ),
            Self::Delete { objects: deleted } => objects
                .delete_objects(&deleted.iter().map(|object| object.id).collect::<Vec<_>>())
                .is_some(),
            Self::GroundCover { patches } => {
                dense_domains.apply_ground_cover_patches(patches, true)
            }
            Self::CreateRegion { region } => regions.restore(region.clone()),
            Self::DeleteRegion { region } => regions.delete(region.id).is_some(),
            Self::UpdateRegion { after, .. } => regions.replace(after.clone()),
            Self::UpdateGroundCoverPreset { after, .. } => catalog.replace_preset(after.clone()),
            Self::UpdateGroundCoverVisual { after, .. } => catalog.replace_visual(after.clone()),
            Self::CreateGroundCoverDefinition { record } => catalog.restore(record.clone()),
            Self::DeleteGroundCoverDefinition { record } => catalog.delete(record.key()).is_some(),
        }
    }

    fn revert(
        &self,
        objects: &mut EditorObjectWorkingSet,
        dense_domains: &mut DenseDomainWorkingSets,
        regions: &mut GroundCoverRegionWorkingSet,
        catalog: &mut GroundCoverCatalogWorkingSet,
    ) -> bool {
        match self {
            Self::Create { object } => objects.delete_object(object.id).is_some(),
            Self::Transform { changes } => objects.set_transforms(
                &changes
                    .iter()
                    .map(|change| (change.object, change.before))
                    .collect::<Vec<_>>(),
            ),
            Self::Delete { objects: deleted } => objects.restore_objects(deleted),
            Self::GroundCover { patches } => {
                dense_domains.apply_ground_cover_patches(patches, false)
            }
            Self::CreateRegion { region } => regions.delete(region.id).is_some(),
            Self::DeleteRegion { region } => regions.restore(region.clone()),
            Self::UpdateRegion { before, .. } => regions.replace(before.clone()),
            Self::UpdateGroundCoverPreset { before, .. } => catalog.replace_preset(before.clone()),
            Self::UpdateGroundCoverVisual { before, .. } => catalog.replace_visual(before.clone()),
            Self::CreateGroundCoverDefinition { record } => catalog.delete(record.key()).is_some(),
            Self::DeleteGroundCoverDefinition { record } => catalog.restore(record.clone()),
        }
    }
}

fn ground_cover_catalog_record_bytes(record: &world_db::GroundCoverCatalogRecord) -> usize {
    match record {
        world_db::GroundCoverCatalogRecord::Preset(record) => {
            std::mem::size_of::<world_db::SourceGroundCoverPresetRecord>()
                + record.key.len()
                + record.display_name.len()
        }
        world_db::GroundCoverCatalogRecord::Visual(record) => {
            std::mem::size_of::<world_db::SourceGroundCoverVisualRecord>()
                + record.key.len()
                + record.display_name.len()
        }
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    command: EditorCommand,
    bytes: usize,
}

#[derive(Resource)]
pub(crate) struct EditorHistory {
    undo: VecDeque<HistoryEntry>,
    redo: VecDeque<HistoryEntry>,
    retained_bytes: usize,
    checkpointed_commands: u64,
    maximum_entries: usize,
    maximum_bytes: usize,
}

impl Default for EditorHistory {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            retained_bytes: 0,
            checkpointed_commands: 0,
            maximum_entries: MAX_HISTORY_ENTRIES,
            maximum_bytes: MAX_HISTORY_BYTES,
        }
    }
}

impl EditorHistory {
    fn record(&mut self, command: EditorCommand) {
        self.clear_redo();
        let bytes = command.estimated_bytes();
        while !self.undo.is_empty()
            && (self.undo.len() >= self.maximum_entries
                || self.retained_bytes.saturating_add(bytes) > self.maximum_bytes)
        {
            let removed = self
                .undo
                .pop_front()
                .expect("history was checked as non-empty");
            self.retained_bytes = self.retained_bytes.saturating_sub(removed.bytes);
            self.checkpointed_commands = self.checkpointed_commands.saturating_add(1);
        }
        if bytes > self.maximum_bytes || self.maximum_entries == 0 {
            self.checkpointed_commands = self.checkpointed_commands.saturating_add(1);
            return;
        }
        self.retained_bytes = self.retained_bytes.saturating_add(bytes);
        self.undo.push_back(HistoryEntry { command, bytes });
    }

    pub(crate) fn create(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        record: SourceObjectViewRecord,
    ) -> bool {
        let object = record.object.clone();
        if !objects.create_object(record) {
            return false;
        }
        self.record(EditorCommand::Create { object });
        true
    }

    pub(crate) fn apply_transform(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        object: StableObjectId,
        transform: SourceObjectTransform,
    ) -> bool {
        self.apply_transforms(objects, &[(object, transform)])
    }

    pub(crate) fn apply_transforms(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        transforms: &[(StableObjectId, SourceObjectTransform)],
    ) -> bool {
        let mut unique = HashSet::with_capacity(transforms.len());
        let mut changes = Vec::with_capacity(transforms.len());
        for (object, after) in transforms {
            if !unique.insert(*object) {
                return false;
            }
            let Some(before) = objects
                .current_view(*object)
                .map(|view| SourceObjectTransform::from(&view.object))
            else {
                return false;
            };
            if before != *after {
                changes.push(ObjectTransformChange {
                    object: *object,
                    before,
                    after: *after,
                });
            }
        }
        if changes.is_empty()
            || !objects.set_transforms(
                &changes
                    .iter()
                    .map(|change| (change.object, change.after))
                    .collect::<Vec<_>>(),
            )
        {
            return false;
        }
        self.record(EditorCommand::Transform { changes });
        true
    }

    pub(crate) fn record_previews(
        &mut self,
        objects: &EditorObjectWorkingSet,
        before: &[(StableObjectId, SourceObjectTransform)],
    ) -> bool {
        let mut unique = HashSet::with_capacity(before.len());
        let mut changes = Vec::with_capacity(before.len());
        for (object, before) in before {
            if !unique.insert(*object) {
                return false;
            }
            let Some(after) = objects
                .current_view(*object)
                .map(|view| SourceObjectTransform::from(&view.object))
            else {
                return false;
            };
            if *before != after {
                changes.push(ObjectTransformChange {
                    object: *object,
                    before: *before,
                    after,
                });
            }
        }
        if changes.is_empty() {
            return false;
        }
        self.record(EditorCommand::Transform { changes });
        true
    }

    pub(crate) fn delete_many(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        selected: &[StableObjectId],
    ) -> bool {
        let Some(deleted) = objects.delete_objects(selected) else {
            return false;
        };
        self.record(EditorCommand::Delete { objects: deleted });
        true
    }

    /// Records a brush gesture whose live preview bytes have already been applied.
    pub(crate) fn record_ground_cover_previews(
        &mut self,
        patches: Vec<GroundCoverMaskPatch>,
    ) -> bool {
        if patches.is_empty() {
            return false;
        }
        self.record(EditorCommand::GroundCover { patches });
        true
    }

    pub(crate) fn create_region(
        &mut self,
        regions: &mut GroundCoverRegionWorkingSet,
        region: world_db::SourceGroundCoverRegionRecord,
    ) -> bool {
        if !regions.create(region.clone()) {
            return false;
        }
        self.record(EditorCommand::CreateRegion { region });
        true
    }

    pub(crate) fn delete_region(
        &mut self,
        regions: &mut GroundCoverRegionWorkingSet,
        region: world_db::SourceGroundCoverRegionRecord,
    ) -> bool {
        regions.pin_source(region.clone());
        if regions.delete(region.id).is_none() {
            return false;
        }
        self.record(EditorCommand::DeleteRegion { region });
        true
    }

    pub(crate) fn update_region(
        &mut self,
        regions: &mut GroundCoverRegionWorkingSet,
        before: world_db::SourceGroundCoverRegionRecord,
        after: world_db::SourceGroundCoverRegionRecord,
    ) -> bool {
        if before == after || before.id != after.id {
            return false;
        }
        regions.pin_source(before.clone());
        if !regions.replace(after.clone()) {
            return false;
        }
        self.record(EditorCommand::UpdateRegion { before, after });
        true
    }

    pub(crate) fn update_ground_cover_preset(
        &mut self,
        catalog: &mut GroundCoverCatalogWorkingSet,
        before: world_db::SourceGroundCoverPresetRecord,
        after: world_db::SourceGroundCoverPresetRecord,
    ) -> bool {
        if before == after || before.id != after.id || !catalog.replace_preset(after.clone()) {
            return false;
        }
        self.record(EditorCommand::UpdateGroundCoverPreset { before, after });
        true
    }

    pub(crate) fn update_ground_cover_visual(
        &mut self,
        catalog: &mut GroundCoverCatalogWorkingSet,
        before: world_db::SourceGroundCoverVisualRecord,
        after: world_db::SourceGroundCoverVisualRecord,
    ) -> bool {
        if before == after || before.id != after.id || !catalog.replace_visual(after.clone()) {
            return false;
        }
        self.record(EditorCommand::UpdateGroundCoverVisual { before, after });
        true
    }

    pub(crate) fn create_ground_cover_preset(
        &mut self,
        catalog: &mut GroundCoverCatalogWorkingSet,
        record: world_db::SourceGroundCoverPresetRecord,
    ) -> bool {
        if !catalog.create_preset(record.clone()) {
            return false;
        }
        self.record(EditorCommand::CreateGroundCoverDefinition {
            record: world_db::GroundCoverCatalogRecord::Preset(record),
        });
        true
    }

    pub(crate) fn create_ground_cover_visual(
        &mut self,
        catalog: &mut GroundCoverCatalogWorkingSet,
        record: world_db::SourceGroundCoverVisualRecord,
    ) -> bool {
        if !catalog.create_visual(record.clone()) {
            return false;
        }
        self.record(EditorCommand::CreateGroundCoverDefinition {
            record: world_db::GroundCoverCatalogRecord::Visual(record),
        });
        true
    }

    pub(crate) fn delete_ground_cover_definition(
        &mut self,
        catalog: &mut GroundCoverCatalogWorkingSet,
        record: world_db::GroundCoverCatalogRecord,
    ) -> bool {
        if catalog.delete(record.key()).is_none() {
            return false;
        }
        self.record(EditorCommand::DeleteGroundCoverDefinition { record });
        true
    }

    pub(crate) fn undo_with_catalog(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        dense_domains: &mut DenseDomainWorkingSets,
        regions: &mut GroundCoverRegionWorkingSet,
        catalog: &mut GroundCoverCatalogWorkingSet,
    ) -> bool {
        let Some(entry) = self.undo.pop_back() else {
            return false;
        };
        if !entry
            .command
            .revert(objects, dense_domains, regions, catalog)
        {
            self.undo.push_back(entry);
            return false;
        }
        self.redo.push_back(entry);
        true
    }

    pub(crate) fn redo_with_catalog(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        dense_domains: &mut DenseDomainWorkingSets,
        regions: &mut GroundCoverRegionWorkingSet,
        catalog: &mut GroundCoverCatalogWorkingSet,
    ) -> bool {
        let Some(entry) = self.redo.pop_back() else {
            return false;
        };
        if !entry
            .command
            .apply(objects, dense_domains, regions, catalog)
        {
            self.redo.push_back(entry);
            return false;
        }
        self.undo.push_back(entry);
        true
    }

    #[cfg(test)]
    pub(crate) fn undo(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        dense_domains: &mut DenseDomainWorkingSets,
        regions: &mut GroundCoverRegionWorkingSet,
    ) -> bool {
        self.undo_with_catalog(
            objects,
            dense_domains,
            regions,
            &mut GroundCoverCatalogWorkingSet::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn redo(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        dense_domains: &mut DenseDomainWorkingSets,
        regions: &mut GroundCoverRegionWorkingSet,
    ) -> bool {
        self.redo_with_catalog(
            objects,
            dense_domains,
            regions,
            &mut GroundCoverCatalogWorkingSet::default(),
        )
    }

    pub(crate) fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub(crate) fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub(crate) fn checkpointed_commands(&self) -> u64 {
        self.checkpointed_commands
    }

    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.retained_bytes = 0;
    }

    fn clear_redo(&mut self) {
        while let Some(entry) = self.redo.pop_front() {
            self.retained_bytes = self.retained_bytes.saturating_sub(entry.bytes);
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct TransformInspectorDraft {
    pub(crate) object: Option<StableObjectId>,
    pub(crate) edit_revision: u64,
    pub(crate) transform: Option<SourceObjectTransform>,
}

impl TransformInspectorDraft {
    pub(crate) fn sync(&mut self, selection: &EditorSelection, objects: &EditorObjectWorkingSet) {
        let object = selection.selected_id();
        if self.object == object && self.edit_revision == objects.edit_revision() {
            return;
        }
        self.object = object;
        self.edit_revision = objects.edit_revision();
        self.transform = selection
            .selected_view(objects)
            .map(|selected| SourceObjectTransform::from(&selected.object));
    }
}

pub(crate) fn normalize_transform(
    mut transform: SourceObjectTransform,
    cell_size: f32,
) -> Option<SourceObjectTransform> {
    if !cell_size.is_finite()
        || cell_size <= 0.0
        || !transform.local_translation.into_iter().all(f32::is_finite)
        || !transform.yaw.is_finite()
        || !transform.scale.is_finite()
        || transform.scale <= 0.0
    {
        return None;
    }
    let origin = transform.owner_cell.origin(cell_size);
    let world = [
        origin[0] + f64::from(transform.local_translation[0]),
        f64::from(transform.local_translation[1]),
        origin[1] + f64::from(transform.local_translation[2]),
    ];
    let minimum = f64::from(i32::MIN) * f64::from(cell_size);
    let maximum = (f64::from(i32::MAX) + 1.0) * f64::from(cell_size);
    if world[0] < minimum || world[0] >= maximum || world[2] < minimum || world[2] >= maximum {
        return None;
    }
    let position = WorldPosition::from_world(transform.space, world, cell_size);
    transform.owner_cell = position.cell;
    transform.local_translation = position.local;
    transform.yaw = transform.yaw.rem_euclid(TAU);
    Some(transform)
}

pub(crate) fn process_project_save_completion(
    mut project: ResMut<ProjectEditorStore>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut coordinator: ResMut<EditorSaveCoordinator>,
) {
    let Some(completion) = project.take_save_completion() else {
        return;
    };
    let committed = matches!(&completion.outcome, ObjectSaveOutcome::Committed(_));
    objects.finish_save(completion.request_id, completion.outcome);
    coordinator.transaction_finished(committed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::{
        CellCoord, GroundCoverLayerId, GroundCoverPresetId, GroundCoverRegionId,
        GroundCoverVisualId, ObjectActivationPolicy, ObjectDefinitionId, WorldSpaceId,
    };
    use world_db::SourceObjectDefinitionRecord;

    fn object_view(id: u8) -> SourceObjectViewRecord {
        let definition = ObjectDefinitionId([2; 16]);
        SourceObjectViewRecord {
            object: SourceObjectRecord {
                id: StableObjectId([id; 16]),
                space: WorldSpaceId(7),
                owner_cell: Default::default(),
                definition,
                local_translation: [1.0, 0.0, 2.0],
                yaw: 0.0,
                scale: 1.0,
                source_revision: 3,
            },
            definition: SourceObjectDefinitionRecord {
                id: definition,
                key: "test".into(),
                display_name: "Test".into(),
                visual_asset: None,
                activation: ObjectActivationPolicy::RenderOnly,
            },
            visual_uri: Some("local/runtime/test.gltf".into()),
            visual_bounds: Some([2.0, 8.0, 2.0]),
        }
    }

    #[test]
    fn history_targets_stable_ids_across_selection_changes() {
        let mut selection = EditorSelection::default();
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        assert!(selection.select(object_view(1), &mut objects));
        let first = selection.selected_id().unwrap();
        let mut moved = SourceObjectTransform::from(&objects.current_view(first).unwrap().object);
        moved.local_translation[0] = 9.0;
        moved.yaw = 1.25;
        moved.scale = 2.5;
        assert!(history.apply_transform(&mut objects, first, moved));
        assert!(selection.select(object_view(3), &mut objects));
        assert!(history.undo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(!objects.dirty(first));
        assert_eq!(selection.selected_id(), Some(StableObjectId([3; 16])));
        assert!(history.redo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(objects.dirty(first));
        let redone = objects.current_view(first).unwrap();
        assert_eq!(redone.object.yaw, 1.25);
        assert_eq!(redone.object.scale, 2.5);
    }

    #[test]
    fn delete_is_a_reversible_tombstone() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        let id = StableObjectId([5; 16]);
        objects.pin(object_view(5));
        assert!(history.delete_many(&mut objects, &[id]));
        assert!(objects.is_deleted(id));
        assert!(history.undo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(!objects.is_deleted(id));
        assert!(!objects.dirty(id));
    }

    #[test]
    fn creation_is_a_reversible_dirty_command() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        let record = object_view(8);
        let id = record.object.id;
        assert!(history.create(&mut objects, record));
        assert!(objects.dirty(id));
        assert!(history.undo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(!objects.dirty(id));
        assert!(history.redo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(objects.dirty(id));
    }

    #[test]
    fn history_checkpoints_oldest_commands_to_stay_within_both_budgets() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory {
            maximum_entries: 2,
            maximum_bytes: 2 * EditorCommand::Delete {
                objects: vec![object_view(1).object],
            }
            .estimated_bytes(),
            ..default()
        };
        for id in 1..=3 {
            let record = object_view(id);
            let object = record.object.id;
            objects.pin(record);
            assert!(history.delete_many(&mut objects, &[object]));
        }
        assert_eq!(history.undo_len(), 2);
        assert_eq!(history.checkpointed_commands(), 1);
        assert!(history.retained_bytes() <= history.maximum_bytes);
    }

    #[test]
    fn multi_selection_tracks_an_active_item_and_toggles_membership() {
        let mut selection = EditorSelection::default();
        let mut objects = EditorObjectWorkingSet::default();
        assert!(selection.select(object_view(1), &mut objects));
        assert!(selection.toggle(object_view(2), &mut objects));
        assert_eq!(selection.selected_ids().len(), 2);
        assert_eq!(selection.selected_id(), Some(StableObjectId([2; 16])));
        assert!(selection.toggle(object_view(2), &mut objects));
        assert_eq!(selection.selected_ids(), &[StableObjectId([1; 16])]);
        assert_eq!(selection.selected_id(), Some(StableObjectId([1; 16])));
    }

    #[test]
    fn grouped_deletion_is_one_reversible_command() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        let ids = [StableObjectId([1; 16]), StableObjectId([2; 16])];
        objects.pin(object_view(1));
        objects.pin(object_view(2));
        assert!(history.delete_many(&mut objects, &ids));
        assert_eq!(history.undo_len(), 1);
        assert!(ids.iter().all(|id| objects.is_deleted(*id)));
        assert!(history.undo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(ids.iter().all(|id| !objects.is_deleted(*id)));
    }

    #[test]
    fn grouped_transform_is_one_reversible_command() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        let first = StableObjectId([1; 16]);
        let second = StableObjectId([2; 16]);
        objects.pin(object_view(1));
        objects.pin(object_view(2));

        let before = [
            (
                first,
                SourceObjectTransform::from(&objects.current_view(first).unwrap().object),
            ),
            (
                second,
                SourceObjectTransform::from(&objects.current_view(second).unwrap().object),
            ),
        ];
        let mut first_after =
            SourceObjectTransform::from(&objects.current_view(first).unwrap().object);
        first_after.local_translation[0] = 7.0;
        let mut second_after =
            SourceObjectTransform::from(&objects.current_view(second).unwrap().object);
        second_after.local_translation[0] = 11.0;
        assert!(objects.set_transforms(&[(first, first_after), (second, second_after)]));
        assert!(history.record_previews(&objects, &before));
        assert_eq!(history.undo_len(), 1);
        assert!(objects.dirty(first));
        assert!(objects.dirty(second));

        assert!(history.undo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(!objects.dirty(first));
        assert!(!objects.dirty(second));
        assert!(history.redo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert_eq!(
            objects
                .current_view(first)
                .unwrap()
                .object
                .local_translation[0],
            7.0
        );
        assert_eq!(
            objects
                .current_view(second)
                .unwrap()
                .object
                .local_translation[0],
            11.0
        );
    }

    #[test]
    fn undoing_a_saved_deletion_can_be_saved_as_recreation() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        let id = StableObjectId([6; 16]);
        objects.pin(object_view(6));
        assert!(history.delete_many(&mut objects, &[id]));
        objects.active_save = Some(ActiveObjectSave {
            request_id: 9,
            objects: vec![id],
        });
        objects.finish_save(
            9,
            ObjectSaveOutcome::Committed(vec![SourceObjectWriteCommit::Deleted(id)]),
        );
        assert!(!objects.dirty(id));
        assert!(history.undo(
            &mut objects,
            &mut DenseDomainWorkingSets::default(),
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(objects.dirty(id));

        let mut project = ProjectEditorStore::default();
        assert!(objects.queue_save(&mut project));
    }

    #[test]
    fn generation_adoption_advances_the_object_runtime_checkpoint() {
        let mut objects = EditorObjectWorkingSet::default();
        let id = StableObjectId([10; 16]);
        objects.pin(object_view(10));
        let mut transform = SourceObjectTransform::from(&objects.current_view(id).unwrap().object);
        transform.local_translation[0] = 15.0;
        assert!(objects.set_transforms(&[(id, transform)]));

        objects.active_save = Some(ActiveObjectSave {
            request_id: 12,
            objects: vec![id],
        });
        let mut committed = objects.current_view(id).unwrap().object;
        committed.source_revision = 4;
        objects.finish_save(
            12,
            ObjectSaveOutcome::Committed(vec![SourceObjectWriteCommit::Updated(committed)]),
        );
        assert_eq!(objects.desired_proxy_views(&[]).len(), 1);
        assert_eq!(objects.adopt_runtime_generation(), 1);
        assert!(objects.desired_proxy_views(&[]).is_empty());
    }

    #[test]
    fn ground_cover_gesture_uses_the_shared_history_and_recreates_sparse_rows_on_redo() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut dense = DenseDomainWorkingSets::default();
        let mut history = EditorHistory::default();
        let region = GroundCoverRegionId([9; 16]);
        let space = WorldSpaceId(3);
        let cell = CellCoord { x: 2, z: -4 };
        assert!(dense.set_ground_cover_coverage(region, space, cell, 2, vec![255, 0, 0, 0]));
        let after = dense.ground_cover_mask(region, space, cell).unwrap();
        let patch = GroundCoverMaskPatch::between(None, &after).unwrap();
        assert!(history.record_ground_cover_previews(vec![patch]));

        assert!(history.undo(
            &mut objects,
            &mut dense,
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert!(dense.ground_cover_mask(region, space, cell).is_none());
        assert!(history.redo(
            &mut objects,
            &mut dense,
            &mut GroundCoverRegionWorkingSet::default(),
        ));
        assert_eq!(
            dense
                .ground_cover_mask(region, space, cell)
                .unwrap()
                .coverage,
            vec![255, 0, 0, 0]
        );
    }

    #[test]
    fn region_catalog_commands_share_the_editor_history() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut dense = DenseDomainWorkingSets::default();
        let mut regions = GroundCoverRegionWorkingSet::default();
        let mut history = EditorHistory::default();
        let region = world_db::SourceGroundCoverRegionRecord {
            id: GroundCoverRegionId([7; 16]),
            layer: GroundCoverLayerId([2; 16]),
            space: WorldSpaceId(3),
            preset: GroundCoverPresetId([4; 16]),
            display_name: "New region".into(),
            enabled: true,
            density_multiplier: 1.0,
            source_revision: 0,
        };

        assert!(history.create_region(&mut regions, region.clone()));
        assert_eq!(
            regions.current(&ProjectEditorStore::default(), region.id),
            Some(region.clone())
        );
        assert!(history.undo(&mut objects, &mut dense, &mut regions));
        assert!(
            regions
                .current(&ProjectEditorStore::default(), region.id)
                .is_none()
        );
        assert!(history.redo(&mut objects, &mut dense, &mut regions));
        assert_eq!(
            regions.current(&ProjectEditorStore::default(), region.id),
            Some(region)
        );
    }

    #[test]
    fn reusable_catalog_create_and_delete_share_the_editor_history() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut dense = DenseDomainWorkingSets::default();
        let mut regions = GroundCoverRegionWorkingSet::default();
        let mut catalog = GroundCoverCatalogWorkingSet::default();
        let mut history = EditorHistory::default();
        let preset = world_db::SourceGroundCoverPresetRecord {
            id: GroundCoverPresetId([8; 16]),
            key: "new-preset".into(),
            display_name: "New preset".into(),
            enabled: true,
            visual: GroundCoverVisualId([9; 16]),
            density_per_square_meter: 4.0,
            seed: 21,
            source_revision: 0,
        };

        assert!(history.create_ground_cover_preset(&mut catalog, preset.clone()));
        assert_eq!(
            catalog.current_preset(&ProjectEditorStore::default(), preset.id),
            Some(preset.clone())
        );
        assert!(history.undo_with_catalog(&mut objects, &mut dense, &mut regions, &mut catalog,));
        assert!(
            catalog
                .current_preset(&ProjectEditorStore::default(), preset.id)
                .is_none()
        );
        assert!(history.redo_with_catalog(&mut objects, &mut dense, &mut regions, &mut catalog,));
        assert!(history.delete_ground_cover_definition(
            &mut catalog,
            world_db::GroundCoverCatalogRecord::Preset(preset.clone()),
        ));
        assert!(history.undo_with_catalog(&mut objects, &mut dense, &mut regions, &mut catalog,));
        assert_eq!(
            catalog.current_preset(&ProjectEditorStore::default(), preset.id),
            Some(preset)
        );
    }

    #[test]
    fn conflict_can_rebase_local_intent_onto_the_latest_database_revision() {
        let mut objects = EditorObjectWorkingSet::default();
        let mut history = EditorHistory::default();
        let id = StableObjectId([4; 16]);
        objects.pin(object_view(4));
        let mut local = SourceObjectTransform::from(&objects.current_view(id).unwrap().object);
        local.local_translation[0] = 12.0;
        assert!(history.apply_transform(&mut objects, id, local));

        let mut actual = object_view(4).object;
        actual.source_revision = 8;
        actual.local_translation[0] = 20.0;
        objects.active_save = Some(ActiveObjectSave {
            request_id: 11,
            objects: vec![id],
        });
        objects.finish_save(
            11,
            ObjectSaveOutcome::Conflict {
                object: id,
                actual: Some(actual),
            },
        );

        assert!(objects.rebase_conflict(id, &mut history));
        assert!(!objects.has_conflict(id));
        assert!(objects.dirty(id));
        assert_eq!(
            objects.current_view(id).unwrap().object.local_translation[0],
            12.0
        );
        assert_eq!(
            objects.entries[&id].base.as_ref().unwrap().source_revision,
            8
        );
        assert_eq!(history.undo_len(), 0);
    }

    #[test]
    fn normalization_carries_local_coordinates_across_cells() {
        let mut transform = SourceObjectTransform::from(&object_view(1).object);
        transform.local_translation = [65.0, 2.0, -1.0];
        let normalized = normalize_transform(transform, 64.0).unwrap();
        assert_eq!(normalized.owner_cell.x, 1);
        assert_eq!(normalized.owner_cell.z, -1);
        assert_eq!(normalized.local_translation, [1.0, 2.0, 63.0]);
    }
}
