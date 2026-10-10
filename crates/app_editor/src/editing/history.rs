//! Undo and redo across objects, environment, roads, areas and presets.

use bevy::prelude::*;
use std::collections::{HashSet, VecDeque};
use world::StableObjectId;
use world_db::{
    SourceEnvironmentCellRecord, SourceObjectRecord, SourceObjectTransform, SourceObjectViewRecord,
};

use super::objects::EditorObjectWorkingSet;
use crate::domain_editing::SourceWorkingSets;

const MAX_HISTORY_ENTRIES: usize = 128;
const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ObjectTransformChange {
    object: StableObjectId,
    before: SourceObjectTransform,
    after: SourceObjectTransform,
}

#[derive(Debug, Clone, PartialEq)]
enum EditorCommand {
    Atmosphere {
        space: world::WorldSpaceId,
        before: Box<world::atmosphere::AtmosphereProfile>,
        after: Box<world::atmosphere::AtmosphereProfile>,
    },
    Roads {
        before: Vec<crate::road_authoring::working::RoadChange>,
        after: Vec<crate::road_authoring::working::RoadChange>,
    },
    /// The whole set before and after: it is one small record.
    Areas {
        before: std::sync::Arc<[world::GameplayArea]>,
        after: std::sync::Arc<[world::GameplayArea]>,
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
                Self::Atmosphere { .. } => {
                    2 * std::mem::size_of::<world::atmosphere::AtmosphereProfile>()
                }
                Self::Roads { before, after } => (before.len() + after.len()) * 4096,
                Self::Areas { before, after } => before
                    .iter()
                    .chain(after.iter())
                    .map(|area| {
                        std::mem::size_of::<world::GameplayArea>()
                            + area.name.len()
                            + std::mem::size_of_val(&area.points[..])
                    })
                    .sum(),
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

    fn apply(&self, objects: &mut EditorObjectWorkingSet, dense: &mut SourceWorkingSets) -> bool {
        match self {
            Self::Atmosphere { space, after, .. } => dense.atmospheres.apply(*space, after),
            Self::Roads { before, after } => dense.roads.replay(before, after).is_ok(),
            Self::Areas { after, .. } => dense.areas.apply(after.clone()).is_ok(),
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

    fn revert(&self, objects: &mut EditorObjectWorkingSet, dense: &mut SourceWorkingSets) -> bool {
        match self {
            Self::Atmosphere { space, before, .. } => dense.atmospheres.apply(*space, before),
            Self::Roads { before, after } => dense.roads.replay(after, before).is_ok(),
            Self::Areas { before, .. } => dense.areas.apply(before.clone()).is_ok(),
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
    dense: &mut SourceWorkingSets,
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
    pub(crate) fn record_atmosphere(
        &mut self,
        space: world::WorldSpaceId,
        before: world::atmosphere::AtmosphereProfile,
        after: world::atmosphere::AtmosphereProfile,
    ) {
        if before != after {
            self.record(EditorCommand::Atmosphere {
                space,
                before: Box::new(before),
                after: Box::new(after),
            });
        }
    }

    pub(crate) fn record_areas(
        &mut self,
        before: std::sync::Arc<[world::GameplayArea]>,
        after: std::sync::Arc<[world::GameplayArea]>,
    ) {
        if before != after {
            self.record(EditorCommand::Areas { before, after });
        }
    }

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
        dense: &mut SourceWorkingSets,
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
        dense: &mut SourceWorkingSets,
        replacement: &environment::EnvironmentDefinition,
    ) -> Result<(), String> {
        let presets = dense.presets().ok_or("Presets are still loading")?.clone();
        self.edit_environment(dense, replacement, &presets)
    }
    pub(crate) fn edit_environment(
        &mut self,
        dense: &mut SourceWorkingSets,
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
        dense: &mut SourceWorkingSets,
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
        dense: &mut SourceWorkingSets,
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
