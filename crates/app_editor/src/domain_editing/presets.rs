//! The shared library participates in the same source transaction and undo intent as layers.
use super::definitions::DefinitionSaveState;
use super::*;
use environment::{EnvironmentDefinition, PresetLibrary};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DirtyPresetSnapshot {
    pub(crate) base: PresetLibrary,
    pub(crate) current: PresetLibrary,
}
pub(crate) fn library_bytes(library: &PresetLibrary) -> usize {
    ron::ser::to_string(library).map_or(usize::MAX, |s| s.len())
}
fn normalize(current: &mut PresetLibrary, base: &PresetLibrary) {
    current.revision = base.revision;
    for p in &mut current.presets {
        p.revision = base.get(p.id).map_or(1, |p| p.revision);
    }
    current.presets.sort_by_key(|p| p.id);
}
impl DenseDomainWorkingSets {
    pub(crate) fn apply_presets(&mut self, library: &PresetLibrary) -> Result<(), String> {
        if self.presets.is_none() {
            return Err("Presets are still loading".into());
        }
        if self.saving() || self.has_any_conflict() || self.gesture_active {
            return Err(
                "Finish the stroke, save or conflict resolution before editing presets".into(),
            );
        }
        self.validate_preset_edit(library, None)?;
        self.install_preset_edit(library);
        Ok(())
    }
    pub(crate) fn presets(&self) -> Option<&PresetLibrary> {
        self.presets.as_ref().map(|p| &p.current)
    }
    pub(crate) fn preset_base_revision(&self) -> u64 {
        self.presets.as_ref().map_or(0, |p| p.base.revision)
    }
    pub(super) fn preset_write(&self) -> Option<PresetLibrary> {
        self.presets
            .as_ref()
            .filter(|p| p.dirty())
            .map(|p| p.current.clone())
    }
    pub(super) fn preset_status(&self) -> Option<String> {
        self.presets.as_ref().and_then(|p|match &p.state {
            DefinitionSaveState::Conflict(_)=>Some("Shared presets changed in the database; local edits are retained. Resolve the conflict.".into()),
            DefinitionSaveState::Failed(e)=>Some(format!("Preset save failed: {e}")),_=>None
        })
    }
    pub(super) fn validate_preset_edit(
        &self,
        library: &PresetLibrary,
        replacement: Option<&EnvironmentDefinition>,
    ) -> Result<(), String> {
        if library_bytes(library) > 1024 * 1024 {
            return Err("Preset library exceeds the editor's 1 MiB authoring limit".into());
        }
        let plants = self.plants.as_ref().ok_or("Plant assets are loading")?;
        library.validate(plants).map_err(|e| e.to_string())?;
        for definition in self.definitions() {
            let d = replacement
                .filter(|r| r.space == definition.space)
                .unwrap_or(definition);
            environment_compile::CompilePlan::new(d, plants, library, Default::default())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub(super) fn install_preset_edit(&mut self, library: &PresetLibrary) {
        let entry = self
            .presets
            .as_mut()
            .expect("library loaded before editing");
        let mut current = library.clone();
        normalize(&mut current, &entry.base);
        if current != entry.current {
            entry.current = current;
            entry.state = DefinitionSaveState::Idle;
            self.bump_definitions();
        }
    }
    pub(super) fn reconcile_presets(&mut self, project: &ProjectEditorStore) {
        let Some(saved) = project.presets() else {
            return;
        };
        match &mut self.presets {
            None => {
                self.presets = Some(DefinitionEntry {
                    base: saved.clone(),
                    current: saved.clone(),
                    state: DefinitionSaveState::Idle,
                });
                self.bump_definitions();
            }
            Some(entry)
                if entry.base != *saved && matches!(entry.state, DefinitionSaveState::Idle) =>
            {
                if entry.dirty()
                    || self.definitions.values().any(|e| e.dirty())
                    || self.entries.values().any(|e| e.pinned())
                {
                    entry.state = DefinitionSaveState::Conflict(Some(saved.clone()));
                } else {
                    entry.base = saved.clone();
                    entry.current = saved.clone();
                }
                self.bump_definitions();
            }
            _ => {}
        }
    }
    pub(super) fn finish_presets(&mut self, request: u64, outcome: &DenseSaveOutcome) {
        let Some(entry) = &mut self.presets else {
            return;
        };
        if matches!(entry.state,DefinitionSaveState::Saving(id) if id==request) {
            entry.state = match outcome {
                DenseSaveOutcome::Failed(e) => DefinitionSaveState::Failed(e.clone()),
                _ => DefinitionSaveState::Idle,
            };
        }
        match outcome {
            DenseSaveOutcome::Committed(commit) => {
                if let Some(library) = &commit.presets {
                    entry.base = library.clone();
                    entry.current = library.clone();
                    entry.state = DefinitionSaveState::Idle;
                    self.bump_definitions();
                }
            }
            DenseSaveOutcome::LibraryConflict { actual } => {
                entry.state = DefinitionSaveState::Conflict(Some(actual.clone()))
            }
            _ => {}
        }
    }
    fn resolved_preset_conflict(&self, keep: bool) -> Option<PresetLibrary> {
        let entry = self.presets.as_ref()?;
        let DefinitionSaveState::Conflict(Some(actual)) = &entry.state else {
            return None;
        };
        let mut result = actual.clone();
        if keep {
            let ids = entry
                .base
                .presets
                .iter()
                .chain(&entry.current.presets)
                .map(|p| p.id)
                .collect::<std::collections::BTreeSet<_>>();
            for id in ids {
                if entry.base.get(id) == entry.current.get(id) {
                    continue;
                }
                result.presets.retain(|p| p.id != id);
                if let Some(p) = entry.current.get(id) {
                    result.presets.push(p.clone());
                }
            }
            normalize(&mut result, actual);
        }
        self.validate_preset_edit(&result, None).ok()?;
        Some(result)
    }
    pub(super) fn can_resolve_presets(&self, keep: bool) -> bool {
        self.resolved_preset_conflict(keep).is_some()
    }
    pub(super) fn resolve_preset_conflict(&mut self, keep: bool) -> usize {
        let Some(current) = self.resolved_preset_conflict(keep) else {
            return 0;
        };
        let entry = self.presets.as_mut().unwrap();
        let DefinitionSaveState::Conflict(Some(actual)) = &entry.state else {
            unreachable!()
        };
        entry.base = actual.clone();
        entry.current = current;
        entry.state = DefinitionSaveState::Idle;
        self.bump_definitions();
        1
    }
    pub(crate) fn dirty_preset_snapshot(&self) -> Option<DirtyPresetSnapshot> {
        self.presets
            .as_ref()
            .filter(|p| p.dirty())
            .map(|p| DirtyPresetSnapshot {
                base: p.base.clone(),
                current: p.current.clone(),
            })
    }
    pub(crate) fn restore_preset_snapshot(
        &mut self,
        snapshot: DirtyPresetSnapshot,
        plants: &vegetation::VegetationCatalog,
    ) -> bool {
        if snapshot.current.validate(plants).is_err()
            || snapshot.base == snapshot.current
            || library_bytes(&snapshot.current) > 1024 * 1024
        {
            return false;
        }
        self.presets = Some(DefinitionEntry {
            base: snapshot.base,
            current: snapshot.current,
            state: DefinitionSaveState::Idle,
        });
        self.plants = Some(plants.clone());
        self.bump_definitions();
        true
    }
    pub(crate) fn initialize_presets(&mut self, library: &PresetLibrary) {
        if self.presets.is_none() {
            self.presets = Some(DefinitionEntry {
                base: library.clone(),
                current: library.clone(),
                state: DefinitionSaveState::Idle,
            });
            self.bump_definitions();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use environment::fixtures::*;
    fn working() -> DenseDomainWorkingSets {
        let library = meadow_library(
            world::TerrainSurfaceId([1; 16]),
            world::TerrainSurfaceId([2; 16]),
            environment::ChannelId([1; 16]),
        );
        let mut dense = DenseDomainWorkingSets::default();
        dense.initialize_presets(&library);
        dense.plants = Some(vegetation::fixtures::reference_catalog());
        dense
    }
    #[test]
    fn keeping_local_preset_edits_preserves_unrelated_external_changes() {
        let mut dense = working();
        let mut local = dense.presets().unwrap().clone();
        local
            .presets
            .iter_mut()
            .find(|p| p.id == DRY_FOLIAGE)
            .unwrap()
            .name = "Local grass".into();
        dense.install_preset_edit(&local);
        let mut actual = dense.presets.as_ref().unwrap().base.clone();
        actual.revision += 1;
        let other = actual
            .presets
            .iter_mut()
            .find(|p| p.id == GREEN_FOLIAGE)
            .unwrap();
        other.name = "External grass".into();
        other.revision += 1;
        dense.finish_presets(
            1,
            &DenseSaveOutcome::LibraryConflict {
                actual: actual.clone(),
            },
        );
        assert!(dense.has_any_conflict());
        assert_eq!(dense.resolve_preset_conflict(true), 1);
        assert_eq!(dense.preset_base_revision(), actual.revision);
        assert_eq!(
            dense.presets().unwrap().get(DRY_FOLIAGE).unwrap().name,
            "Local grass"
        );
        assert_eq!(
            dense.presets().unwrap().get(GREEN_FOLIAGE),
            actual.get(GREEN_FOLIAGE)
        );
        assert!(!dense.has_any_conflict());
        assert!(dense.preset_write().is_some());
    }
    #[test]
    fn a_paint_only_library_conflict_does_not_revert_shared_defaults() {
        let mut dense = working();
        let mut actual = dense.presets().unwrap().clone();
        actual.revision += 1;
        actual.presets[0].name = "External default".into();
        actual.presets[0].revision += 1;
        dense.finish_presets(
            1,
            &DenseSaveOutcome::LibraryConflict {
                actual: actual.clone(),
            },
        );
        assert_eq!(dense.resolve_preset_conflict(true), 1);
        assert_eq!(dense.presets(), Some(&actual));
        assert!(dense.preset_write().is_none());
    }
    #[test]
    fn dedicated_preset_history_survives_save_and_preserves_unrelated_changes() {
        use crate::editing::{EditorHistory, EditorObjectWorkingSet, merge_preset_changes};
        let mut dense = working();
        let original = dense.presets().unwrap().clone();
        let mut edited = original.clone();
        edited
            .presets
            .iter_mut()
            .find(|p| p.id == DRY_FOLIAGE)
            .unwrap()
            .name = "Edited grass".into();
        let mut history = EditorHistory::default();
        let mut objects = EditorObjectWorkingSet::default();
        history.edit_presets(&mut dense, &edited).unwrap();
        let mut saved = dense.presets().unwrap().clone();
        saved.revision += 1;
        for p in &mut saved.presets {
            p.revision += 1;
        }
        saved
            .presets
            .iter_mut()
            .find(|p| p.id == GREEN_FOLIAGE)
            .unwrap()
            .name = "Unrelated external name".into();
        let entry = dense.presets.as_mut().unwrap();
        entry.base = saved.clone();
        entry.current = saved.clone();
        history.undo(&mut objects, &mut dense);
        assert_eq!(
            dense.presets().unwrap().get(DRY_FOLIAGE).unwrap().name,
            original.get(DRY_FOLIAGE).unwrap().name
        );
        assert_eq!(
            dense.presets().unwrap().get(GREEN_FOLIAGE),
            saved.get(GREEN_FOLIAGE)
        );
        history.redo(&mut objects, &mut dense);
        assert_eq!(
            dense.presets().unwrap().get(DRY_FOLIAGE).unwrap().name,
            "Edited grass"
        );
        assert_eq!(
            dense.presets().unwrap().get(GREEN_FOLIAGE),
            saved.get(GREEN_FOLIAGE)
        );
        assert!(merge_preset_changes(Some(&saved), &original, &edited).is_err());
    }
}

#[cfg(test)]
mod scatter_tests;
