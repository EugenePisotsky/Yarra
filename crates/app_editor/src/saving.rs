//! One user-facing save intent over bounded, domain-specific source transactions.
//!
//! The database writers remain deliberately bounded and typed, but the user should not have to
//! submit each batch or source domain manually. This coordinator drains objects and dense terrain
//! records in dependency-safe order and can continue directly into publication.

use bevy::prelude::*;

use crate::{
    domain_editing::DenseDomainWorkingSets, editing::EditorObjectWorkingSet,
    project_store::ProjectEditorStore, publication::RuntimePublicationState,
    vegetation_authoring::VegetationAuthoringState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SaveIntent {
    #[default]
    Idle,
    Save,
    SaveAndPublish,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct EditorSaveCoordinator {
    intent: SaveIntent,
}

impl EditorSaveCoordinator {
    pub(crate) fn request_save(&mut self) -> bool {
        if self.intent != SaveIntent::Idle {
            return false;
        }
        self.intent = SaveIntent::Save;
        true
    }

    pub(crate) fn request_publish(&mut self) -> bool {
        if self.intent != SaveIntent::Idle {
            return false;
        }
        self.intent = SaveIntent::SaveAndPublish;
        true
    }

    pub(crate) const fn active(&self) -> bool {
        !matches!(self.intent, SaveIntent::Idle)
    }

    pub(crate) const fn publishes_after_save(&self) -> bool {
        matches!(self.intent, SaveIntent::SaveAndPublish)
    }

    pub(crate) fn status(&self, remaining_changes: usize) -> String {
        match (self.intent, remaining_changes) {
            (SaveIntent::Idle, _) => "Source save idle".into(),
            (SaveIntent::Save, 0) => "Finishing source save…".into(),
            (SaveIntent::Save, count) => {
                format!("Saving all source changes… {count} remaining")
            }
            (SaveIntent::SaveAndPublish, 0) => {
                "Source saved; preparing runtime publication…".into()
            }
            (SaveIntent::SaveAndPublish, count) => {
                format!("Saving all source changes… {count} remaining, then publishing")
            }
        }
    }

    /// A conflict, rejected deletion, or writer failure terminates the automatic drain. The dirty
    /// working state and its domain-specific diagnostic remain available for explicit resolution.
    pub(crate) fn transaction_finished(&mut self, committed: bool) {
        if !committed {
            self.intent = SaveIntent::Idle;
        }
    }

    fn finish(&mut self) {
        self.intent = SaveIntent::Idle;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveDomain {
    Objects,
    Dense,
    Vegetation,
}

fn next_save_domain(
    object_count: usize,
    dense_count: usize,
    vegetation_count: usize,
) -> Option<SaveDomain> {
    if object_count > 0 {
        Some(SaveDomain::Objects)
    } else if dense_count > 0 {
        Some(SaveDomain::Dense)
    } else if vegetation_count > 0 {
        Some(SaveDomain::Vegetation)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_editor_save(
    mut coordinator: ResMut<EditorSaveCoordinator>,
    mut project: ResMut<ProjectEditorStore>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut vegetation: ResMut<VegetationAuthoringState>,
    mut publication: ResMut<RuntimePublicationState>,
    presets: Res<crate::workspaces::presets::PresetAuthoringState>,
    paint: Res<crate::environment_paint::EnvironmentPaintState>,
) {
    if !coordinator.active() {
        return;
    }
    if dense.atmospheres.gesture.is_some()
        || dense.gesture_active
        || project.save_in_flight()
        || objects.saving()
        || dense.saving()
        || vegetation.saving()
    {
        return;
    }
    if presets.dirty()
        || paint.has_unapplied_changes()
        || project.write_error().is_some()
        || objects.has_any_conflict()
        || dense.has_any_conflict()
        || vegetation.has_conflict()
        || publication.active()
    {
        coordinator.finish();
        return;
    }

    if dense.atmospheres.dirty_count() > 0 {
        if !dense.atmospheres.queue_save(&mut project) {
            coordinator.finish();
        }
        return;
    }

    match next_save_domain(
        objects.dirty_count(),
        dense.dirty_count(),
        vegetation.dirty_count(),
    ) {
        Some(SaveDomain::Objects) => {
            objects.queue_save(&mut project);
        }
        Some(SaveDomain::Dense) => {
            if !dense.queue_save(&mut project) {
                coordinator.finish();
            }
        }
        Some(SaveDomain::Vegetation) => {
            vegetation.queue_save(&mut project);
        }
        None => {
            if coordinator.publishes_after_save() {
                publication.request(project.source_epoch());
            }
            coordinator.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_intent_is_one_operation_and_failure_stops_automatic_drain() {
        let mut coordinator = EditorSaveCoordinator::default();
        assert!(coordinator.request_save());
        assert!(coordinator.active());
        assert!(!coordinator.request_save());
        coordinator.transaction_finished(true);
        assert!(coordinator.active());
        coordinator.transaction_finished(false);
        assert!(!coordinator.active());
    }

    #[test]
    fn publish_intent_survives_successful_batches_until_publication() {
        let mut coordinator = EditorSaveCoordinator::default();
        assert!(coordinator.request_publish());
        coordinator.transaction_finished(true);
        assert!(coordinator.active());
        assert!(coordinator.publishes_after_save());
    }

    #[test]
    fn domain_order_saves_objects_before_dense_records() {
        assert_eq!(next_save_domain(4, 5, 1), Some(SaveDomain::Objects));
        assert_eq!(next_save_domain(0, 5, 1), Some(SaveDomain::Dense));
        assert_eq!(next_save_domain(0, 0, 1), Some(SaveDomain::Vegetation));
        assert_eq!(next_save_domain(0, 0, 0), None);
    }
}
