//! Validated catalog draft, revision tracking, persistence and conflict resolution.
use crate::{
    project_store::{ProjectEditorStore, VegetationSaveOutcome},
    saving::EditorSaveCoordinator,
};
use bevy::prelude::*;
use vegetation::{VegetationCatalog, VegetationGroupingProfile};

#[derive(Resource)]
pub(crate) struct VegetationAuthoringState {
    baseline: Option<VegetationCatalog>,
    pub(super) working: Option<VegetationCatalog>,
    pub(super) selected_population: usize,
    pub(super) selected_species: usize,
    pub(super) revision: u64,
    pub(super) dirty: bool,
    pub(super) preview_enabled: bool,
    pub(super) curve_editor_scale: f32,
    pub(super) validation_error: Option<String>,
    pub(super) preview_error: Option<String>,
    save_request: Option<u64>,
    pub(super) save_error: Option<String>,
    conflict_actual: Option<Option<VegetationCatalog>>,
}

impl Default for VegetationAuthoringState {
    fn default() -> Self {
        Self {
            baseline: None,
            working: None,
            selected_population: 0,
            selected_species: 0,
            revision: 1,
            dirty: false,
            preview_enabled: true,
            curve_editor_scale: 1.35,
            validation_error: None,
            preview_error: None,
            save_request: None,
            save_error: None,
            conflict_actual: None,
        }
    }
}

impl VegetationAuthoringState {
    pub(crate) fn study_source(&self) -> Option<(&VegetationCatalog, usize, u64)> {
        self.working
            .as_ref()
            .map(|catalog| (catalog, self.selected_population, self.revision))
    }

    /// Explicit replay import replaces the draft while retaining the project's save baseline.
    pub(crate) fn import_study_catalog(&mut self, catalog: VegetationCatalog, population: &str) {
        self.selected_population = catalog
            .populations
            .iter()
            .position(|p| p.key == population)
            .unwrap_or(0);
        self.selected_species = catalog
            .populations
            .get(self.selected_population)
            .and_then(|p| p.species.first())
            .and_then(|s| catalog.species.iter().position(|v| v.id == s.species))
            .unwrap_or(0);
        self.apply(catalog);
    }

    fn install(&mut self, catalog: VegetationCatalog) {
        self.selected_population = catalog
            .populations
            .iter()
            .position(|population| {
                matches!(population.grouping, VegetationGroupingProfile::Voronoi(_))
            })
            .unwrap_or(0);
        self.selected_species = catalog
            .populations
            .get(self.selected_population)
            .and_then(|population| population.species.first())
            .and_then(|choice| {
                catalog
                    .species
                    .iter()
                    .position(|species| species.id == choice.species)
            })
            .unwrap_or(0);
        self.baseline = Some(catalog.clone());
        self.working = Some(catalog);
        self.dirty = false;
        self.validation_error = None;
        self.preview_error = None;
        self.save_request = None;
        self.save_error = None;
        self.conflict_actual = None;
        self.bump_revision();
    }

    pub(super) fn apply(&mut self, catalog: VegetationCatalog) {
        match catalog.validate() {
            Ok(()) => {
                self.working = Some(catalog);
                self.dirty = self.working != self.baseline;
                self.validation_error = None;
                self.bump_revision();
            }
            Err(error) => self.validation_error = Some(error.to_string()),
        }
    }

    pub(super) fn reset(&mut self) {
        if let Some(baseline) = self.baseline.clone() {
            self.working = Some(baseline);
            self.dirty = false;
            self.validation_error = None;
            self.preview_error = None;
            self.save_error = None;
            self.conflict_actual = None;
            self.bump_revision();
        }
    }

    pub(crate) const fn dirty_count(&self) -> usize {
        self.dirty as usize
    }

    pub(crate) const fn saving(&self) -> bool {
        self.save_request.is_some()
    }

    pub(crate) const fn has_conflict(&self) -> bool {
        self.conflict_actual.is_some()
    }

    pub(crate) fn queue_save(&mut self, project: &mut ProjectEditorStore) -> bool {
        if !self.dirty || self.saving() || self.has_conflict() {
            return false;
        }
        let Some(replacement) = self.working.clone() else {
            return false;
        };
        let Some(request_id) = project.queue_vegetation_catalog(self.baseline.clone(), replacement)
        else {
            return false;
        };
        self.save_request = Some(request_id);
        self.save_error = None;
        true
    }

    fn finish_save(&mut self, request_id: u64, outcome: VegetationSaveOutcome) {
        if self.save_request != Some(request_id) {
            return;
        }
        self.save_request = None;
        match outcome {
            VegetationSaveOutcome::Committed(saved) => {
                self.baseline = Some(saved);
                self.dirty = self.working != self.baseline;
                self.save_error = None;
                self.conflict_actual = None;
            }
            VegetationSaveOutcome::Conflict { actual } => {
                self.save_error = Some(
                    "The project vegetation catalog changed after this draft was opened.".into(),
                );
                self.conflict_actual = Some(actual);
            }
            VegetationSaveOutcome::Failed(error) => {
                self.save_error = Some(format!("Vegetation save failed: {error}"));
            }
        }
    }

    pub(super) fn reload_conflict(&mut self) {
        let Some(actual) = self.conflict_actual.take() else {
            return;
        };
        self.save_error = None;
        if let Some(actual) = actual {
            self.install(actual);
        } else {
            self.baseline = None;
            self.working = None;
            self.dirty = false;
            self.bump_revision();
        }
    }

    pub(super) fn keep_draft_after_conflict(&mut self) {
        let Some(actual) = self.conflict_actual.take() else {
            return;
        };
        self.baseline = actual;
        self.dirty = self.working != self.baseline;
        self.save_error = None;
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
}

pub(crate) fn process_vegetation_save_completion(
    mut project: ResMut<ProjectEditorStore>,
    mut state: ResMut<VegetationAuthoringState>,
    mut coordinator: ResMut<EditorSaveCoordinator>,
) {
    let Some(completion) = project.take_vegetation_save_completion() else {
        return;
    };
    let committed = matches!(&completion.outcome, VegetationSaveOutcome::Committed(_));
    state.finish_save(completion.request_id, completion.outcome);
    coordinator.transaction_finished(committed);
}

pub(super) fn adopt_project_catalog(
    project: Res<ProjectEditorStore>,
    mut state: ResMut<VegetationAuthoringState>,
) {
    if state.baseline.is_some() {
        return;
    }
    let Some(source_catalog) = project.vegetation_catalog().cloned() else {
        return;
    };
    state.install(source_catalog);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_draft_validates_before_replacing_the_live_catalog() {
        let catalog = vegetation::fixtures::reference_catalog();
        let mut state = VegetationAuthoringState::default();
        state.install(catalog.clone());
        let mut invalid = catalog.clone();
        invalid.populations[0].density_per_square_meter = -1.0;
        state.apply(invalid);
        assert_eq!(state.working, Some(catalog));
        assert!(state.validation_error.is_some());
        assert!(!state.dirty);
    }
    #[test]
    fn reset_restores_the_project_baseline() {
        let catalog = vegetation::fixtures::reference_catalog();
        let mut state = VegetationAuthoringState::default();
        state.install(catalog.clone());
        let mut changed = catalog.clone();
        changed.populations[0].density_per_square_meter *= 0.9;
        state.apply(changed);
        assert!(state.dirty);
        state.reset();
        assert_eq!(state.working, Some(catalog));
        assert!(!state.dirty);
    }
    #[test]
    fn study_import_keeps_project_baseline_and_existing_save_request() {
        let catalog = vegetation::fixtures::reference_catalog();
        let mut state = VegetationAuthoringState::default();
        state.install(catalog.clone());
        state.save_request = Some(17);
        let mut study = catalog.clone();
        study.populations[3].density_per_square_meter = 25.0;
        state.import_study_catalog(study.clone(), "short_split_fill");
        assert_eq!(state.baseline, Some(catalog));
        assert_eq!(state.working, Some(study));
        assert_eq!(state.selected_population, 3);
        assert_eq!(state.save_request, Some(17));
        assert!(state.dirty);
    }
}
