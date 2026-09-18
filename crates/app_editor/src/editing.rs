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

use crate::domain_editing::DenseDomainWorkingSets;
use crate::project_store::{ObjectSaveOutcome, ProjectEditorStore};
use crate::saving::EditorSaveCoordinator;
use world_db::SourceEnvironmentCellRecord;

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
    Roads {
        before: Vec<crate::road_authoring::working::RoadChange>,
        after: Vec<crate::road_authoring::working::RoadChange>,
    },
    Presets {
        before: Box<environment::PresetLibrary>,
        after: Box<environment::PresetLibrary>,
    },
    Definition {
        before_presets: Box<environment::PresetLibrary>,
        after_presets: Box<environment::PresetLibrary>,
        before: Box<environment::EnvironmentDefinition>,
        after: Box<environment::EnvironmentDefinition>,
    },
    Environment {
        before: Vec<SourceEnvironmentCellRecord>,
        after: Vec<SourceEnvironmentCellRecord>,
    },
    Create {
        object: SourceObjectRecord,
    },
    Transform {
        changes: Vec<ObjectTransformChange>,
    },
    Delete {
        objects: Vec<SourceObjectRecord>,
    },
}

impl EditorCommand {
    fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Roads { before, after } => (before.len() + after.len()) * 4096,
                Self::Presets { before, after } => {
                    crate::domain_editing::library_bytes(before)
                        + crate::domain_editing::library_bytes(after)
                }
                Self::Definition {
                    before,
                    after,
                    before_presets,
                    after_presets,
                } => {
                    crate::domain_editing::library_bytes(before_presets)
                        + crate::domain_editing::library_bytes(after_presets)
                        + crate::domain_editing::definition_bytes(before)
                        + crate::domain_editing::definition_bytes(after)
                }
                Self::Environment { before, after } => before
                    .iter()
                    .chain(after)
                    .map(|r| std::mem::size_of::<SourceEnvironmentCellRecord>() + r.sample_bytes())
                    .sum(),
                Self::Create { .. } => std::mem::size_of::<SourceObjectRecord>(),
                Self::Transform { changes } => {
                    changes.len() * std::mem::size_of::<ObjectTransformChange>()
                }
                Self::Delete { objects } => {
                    objects.len() * std::mem::size_of::<SourceObjectRecord>()
                }
            }
    }

    fn apply(
        &self,
        objects: &mut EditorObjectWorkingSet,
        dense: &mut DenseDomainWorkingSets,
    ) -> bool {
        match self {
            Self::Roads { before, after } => dense.roads.replay(before, after).is_ok(),
            Self::Presets { before, after } => merge_preset_changes(dense.presets(), before, after)
                .is_ok_and(|p| dense.apply_presets(&p).is_ok()),
            Self::Definition {
                after,
                before_presets,
                after_presets,
                ..
            } => replay_environment(dense, after, before_presets, after_presets),
            Self::Environment { after, .. } => dense.apply_environment_records(after).is_ok(),
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
        }
    }

    fn revert(
        &self,
        objects: &mut EditorObjectWorkingSet,
        dense: &mut DenseDomainWorkingSets,
    ) -> bool {
        match self {
            Self::Roads { before, after } => dense.roads.replay(after, before).is_ok(),
            Self::Presets { before, after } => merge_preset_changes(dense.presets(), after, before)
                .is_ok_and(|p| dense.apply_presets(&p).is_ok()),
            Self::Definition {
                before,
                before_presets,
                after_presets,
                ..
            } => replay_environment(dense, before, after_presets, before_presets),
            Self::Environment { before, .. } => dense.apply_environment_records(before).is_ok(),
            Self::Create { object } => objects.delete_object(object.id).is_some(),
            Self::Transform { changes } => objects.set_transforms(
                &changes
                    .iter()
                    .map(|change| (change.object, change.before))
                    .collect::<Vec<_>>(),
            ),
            Self::Delete { objects: deleted } => objects.restore_objects(deleted),
        }
    }
}

// History owns only the presets changed by this command. An unrelated shared edit
// adopted since the command was recorded must survive undo/redo of a layer edit.
fn replay_environment(
    dense: &mut DenseDomainWorkingSets,
    definition: &environment::EnvironmentDefinition,
    from: &environment::PresetLibrary,
    to: &environment::PresetLibrary,
) -> bool {
    merge_preset_changes(dense.presets(), from, to)
        .is_ok_and(|p| dense.apply_environment(definition, &p).is_ok())
}
/// Merge only authored changes; revision-only save checkpoints do not conflict.
pub(crate) fn merge_preset_changes(
    current: Option<&environment::PresetLibrary>,
    from: &environment::PresetLibrary,
    to: &environment::PresetLibrary,
) -> Result<environment::PresetLibrary, String> {
    let mut library = current.ok_or("Presets are still loading")?.clone();
    let same = |a: Option<&environment::Preset>, b: Option<&environment::Preset>| match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.id == b.id && a.name == b.name && a.kind == b.kind,
        _ => false,
    };
    let ids = from
        .presets
        .iter()
        .chain(&to.presets)
        .map(|p| p.id)
        .collect::<std::collections::BTreeSet<_>>();
    for id in ids {
        if same(from.get(id), to.get(id)) {
            continue;
        }
        if !same(library.get(id), from.get(id)) {
            return Err(format!(
                "Preset '{}' changed outside this draft. Reload the draft before applying.",
                to.get(id).or_else(|| from.get(id)).unwrap().name
            ));
        }
        library.presets.retain(|p| p.id != id);
        if let Some(preset) = to.get(id) {
            library.presets.push(preset.clone());
        }
    }
    Ok(library)
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
    pub(crate) fn road_keys(&self) -> std::collections::BTreeSet<world_db::RoadRecordKey> {
        self.undo
            .iter()
            .chain(&self.redo)
            .flat_map(|e| match &e.command {
                EditorCommand::Roads { before, after } => before
                    .iter()
                    .chain(after)
                    .flat_map(|c| {
                        std::iter::once(c.key).chain(
                            c.record
                                .iter()
                                .flat_map(world_db::RoadSourceRecord::references),
                        )
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect()
    }
    pub(crate) fn record_roads(
        &mut self,
        before: Vec<crate::road_authoring::working::RoadChange>,
        after: Vec<crate::road_authoring::working::RoadChange>,
    ) {
        if before != after {
            self.record(EditorCommand::Roads { before, after });
        }
    }

    pub(crate) fn edit_presets(
        &mut self,
        dense: &mut DenseDomainWorkingSets,
        replacement: &environment::PresetLibrary,
    ) -> Result<(), String> {
        let before = dense.presets().ok_or("Presets are still loading")?.clone();
        dense.apply_presets(replacement)?;
        let after = dense.presets().unwrap().clone();
        if before != after {
            self.record(EditorCommand::Presets {
                before: Box::new(before),
                after: Box::new(after),
            });
        }
        Ok(())
    }

    pub(crate) fn edit_definition(
        &mut self,
        dense: &mut DenseDomainWorkingSets,
        replacement: &environment::EnvironmentDefinition,
    ) -> Result<(), String> {
        let presets = dense.presets().ok_or("Presets are still loading")?.clone();
        self.edit_environment(dense, replacement, &presets)
    }
    pub(crate) fn edit_environment(
        &mut self,
        dense: &mut DenseDomainWorkingSets,
        replacement: &environment::EnvironmentDefinition,
        presets: &environment::PresetLibrary,
    ) -> Result<(), String> {
        let before = dense
            .definition(replacement.space)
            .ok_or("Environment is still loading")?
            .clone();
        let before_presets = dense.presets().ok_or("Presets are still loading")?.clone();
        dense.apply_environment(replacement, presets)?;
        let after = dense.definition(replacement.space).unwrap().clone();
        let after_presets = dense.presets().unwrap().clone();
        if before != after || before_presets != after_presets {
            self.record(EditorCommand::Definition {
                before: Box::new(before),
                after: Box::new(after),
                before_presets: Box::new(before_presets),
                after_presets: Box::new(after_presets),
            });
        }
        Ok(())
    }
    pub(crate) fn environment_cells(&self) -> HashSet<(world::WorldSpaceId, world::CellCoord)> {
        self.undo
            .iter()
            .chain(&self.redo)
            .flat_map(|entry| match &entry.command {
                EditorCommand::Environment { before, .. } => {
                    before.iter().map(|r| (r.space, r.cell)).collect::<Vec<_>>()
                }
                _ => Vec::new(),
            })
            .collect()
    }
    pub(crate) fn record_environment_stroke(
        &mut self,
        before: Vec<SourceEnvironmentCellRecord>,
        after: Vec<SourceEnvironmentCellRecord>,
    ) {
        if before != after {
            self.record(EditorCommand::Environment { before, after });
        }
    }

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

    pub(crate) fn undo(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        dense: &mut DenseDomainWorkingSets,
    ) -> bool {
        let Some(entry) = self.undo.pop_back() else {
            return false;
        };
        if !entry.command.revert(objects, dense) {
            self.undo.push_back(entry);
            return false;
        }
        self.redo.push_back(entry);
        true
    }

    pub(crate) fn redo(
        &mut self,
        objects: &mut EditorObjectWorkingSet,
        dense: &mut DenseDomainWorkingSets,
    ) -> bool {
        let Some(entry) = self.redo.pop_back() else {
            return false;
        };
        if !entry.command.apply(objects, dense) {
            self.redo.push_back(entry);
            return false;
        }
        self.undo.push_back(entry);
        true
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
