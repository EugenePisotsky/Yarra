//! One user-facing save intent over bounded, domain-specific source transactions.
//!
//! The database writers remain deliberately bounded and typed, but the user should not have to
//! submit each batch or source domain manually. This coordinator drains reusable catalog records,
//! regions, objects, and dense cell records in dependency-safe order and can continue directly
//! into publication.

use bevy::prelude::*;

use crate::ground_cover_catalog::{CatalogSavePhase, GroundCoverCatalogWorkingSet};
use crate::{
    catalog_editing::GroundCoverRegionWorkingSet, domain_editing::DenseDomainWorkingSets,
    editing::EditorObjectWorkingSet, project_store::ProjectEditorStore,
    publication::RuntimePublicationState,
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
    CatalogUpserts,
    Regions,
    CatalogDeletions,
    Objects,
    Dense,
}

fn next_save_domain(
    catalog_upsert_count: usize,
    region_count: usize,
    catalog_deletion_count: usize,
    object_count: usize,
    dense_count: usize,
) -> Option<SaveDomain> {
    if catalog_upsert_count > 0 {
        Some(SaveDomain::CatalogUpserts)
    } else if region_count > 0 {
        Some(SaveDomain::Regions)
    } else if catalog_deletion_count > 0 {
        Some(SaveDomain::CatalogDeletions)
    } else if object_count > 0 {
        Some(SaveDomain::Objects)
    } else if dense_count > 0 {
        Some(SaveDomain::Dense)
    } else {
        None
    }
}

pub(crate) fn drive_editor_save(
    mut coordinator: ResMut<EditorSaveCoordinator>,
    mut project: ResMut<ProjectEditorStore>,
    mut catalog: ResMut<GroundCoverCatalogWorkingSet>,
    mut regions: ResMut<GroundCoverRegionWorkingSet>,
    mut objects: ResMut<EditorObjectWorkingSet>,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut publication: ResMut<RuntimePublicationState>,
) {
    if !coordinator.active() {
        return;
    }
    if project.save_in_flight()
        || catalog.saving()
        || regions.saving()
        || objects.saving()
        || dense.saving()
    {
        return;
    }
    if project.write_error().is_some()
        || regions.has_any_conflict()
        || catalog.has_any_conflict()
        || objects.has_any_conflict()
        || dense.has_any_conflict()
        || publication.active()
    {
        coordinator.finish();
        return;
    }

    match next_save_domain(
        catalog.dirty_upsert_count(),
        regions.dirty_count(),
        catalog.dirty_deletion_count(),
        objects.dirty_count(),
        dense.dirty_count(),
    ) {
        Some(SaveDomain::CatalogUpserts) => {
            catalog.queue_save_phase(&mut project, CatalogSavePhase::Upserts);
        }
        Some(SaveDomain::Regions) => {
            regions.queue_save(&mut project);
        }
        Some(SaveDomain::CatalogDeletions) => {
            catalog.queue_save_phase(&mut project, CatalogSavePhase::Deletions);
        }
        Some(SaveDomain::Objects) => {
            objects.queue_save(&mut project);
        }
        Some(SaveDomain::Dense) => {
            dense.queue_save(&mut project);
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
    fn domain_order_respects_catalog_and_region_dependencies() {
        assert_eq!(
            next_save_domain(1, 2, 3, 4, 5),
            Some(SaveDomain::CatalogUpserts)
        );
        assert_eq!(next_save_domain(0, 2, 3, 4, 5), Some(SaveDomain::Regions));
        assert_eq!(
            next_save_domain(0, 0, 3, 4, 5),
            Some(SaveDomain::CatalogDeletions)
        );
        assert_eq!(next_save_domain(0, 0, 0, 4, 5), Some(SaveDomain::Objects));
        assert_eq!(next_save_domain(0, 0, 0, 0, 5), Some(SaveDomain::Dense));
        assert_eq!(next_save_domain(0, 0, 0, 0, 0), None);
    }
}
