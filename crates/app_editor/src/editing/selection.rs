//! Which objects are selected, and the primary one.

use bevy::prelude::*;
use world::StableObjectId;
use world_db::SourceObjectViewRecord;

use super::objects::EditorObjectWorkingSet;

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
