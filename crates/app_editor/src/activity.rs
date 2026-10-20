//! Whether source changes may happen now. A save or publication, a domain being written, a
//! pointer gesture, a conflict and a draft waiting elsewhere each hold some of them up; they
//! are gathered here so every button, shortcut and tool applies the same rules.
use crate::{
    domain_editing::SourceWorkingSets,
    editing::{EditorHistory, EditorObjectWorkingSet},
    project_store::ProjectEditorStore,
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
};

use bevy_egui::egui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SourceActivity {
    /// A save or publication runs, or a domain is being written.
    pub(crate) writing: bool,
    /// A pointer gesture is under way: a stroke, an area drag, an atmosphere slider.
    pub(crate) gesture: bool,
    /// A working set disagrees with the source and must be resolved first.
    pub(crate) conflict: bool,
    /// The project could not be written.
    pub(crate) write_failed: bool,
    /// A draft elsewhere (layer settings, presets) waits to be applied or discarded.
    pub(crate) drafts: bool,
}

impl SourceActivity {
    pub(crate) fn of(
        project: &ProjectEditorStore,
        save: &EditorSaveCoordinator,
        publication: &RuntimePublicationState,
        objects: &EditorObjectWorkingSet,
        dense: &SourceWorkingSets,
    ) -> Self {
        Self {
            writing: save.active()
                || publication.active()
                || project.save_in_flight()
                || objects.saving()
                || dense.saving(),
            gesture: dense.gesture_active || dense.atmospheres.gesture.is_some(),
            conflict: objects.has_any_conflict() || dense.has_any_conflict(),
            write_failed: project.write_error().is_some(),
            drafts: false,
        }
    }

    pub(crate) fn with_drafts(self, drafts: bool) -> Self {
        Self { drafts, ..self }
    }

    /// A tool may change the source: nothing is being written and nothing is in conflict.
    /// Its own gesture may be under way.
    pub(crate) fn can_edit(&self) -> bool {
        !self.writing && !self.conflict
    }

    /// History may step: also no gesture under way and no draft waiting elsewhere.
    pub(crate) fn can_step(&self) -> bool {
        self.can_edit() && !self.gesture && !self.drafts
    }

    /// A save or publication may start: also a project that can be written.
    pub(crate) fn can_save(&self) -> bool {
        self.can_step() && !self.write_failed
    }
}

/// What the Save, Publish, Undo and Redo buttons act on.
pub(crate) struct SourceActions<'a> {
    pub(crate) activity: SourceActivity,
    /// Whether the project has been saved before, so there is something to publish.
    pub(crate) saved_before: bool,
    pub(crate) history: &'a mut EditorHistory,
    pub(crate) objects: &'a mut EditorObjectWorkingSet,
    pub(crate) dense: &'a mut SourceWorkingSets,
    pub(crate) save: &'a mut EditorSaveCoordinator,
}

impl SourceActions<'_> {
    /// Save, Publish (Save & Publish with changes), Undo and Redo, each enabled as
    /// [`SourceActivity`] allows. True when undo or redo changed the working sets.
    pub(crate) fn buttons(&mut self, ui: &mut egui::Ui) -> bool {
        let dirty = self.objects.dirty_count() + self.dense.dirty_count() > 0;
        let can_save = self.activity.can_save();

        if ui
            .add_enabled(can_save && dirty, egui::Button::new("Save"))
            .on_hover_text("Save all local source changes (Cmd+S)")
            .clicked()
        {
            self.save.request_save();
        }

        let (label, hint) = if dirty {
            (
                "Save & Publish",
                "Save every local source change, then cook, validate, publish, and adopt it",
            )
        } else {
            (
                "Publish",
                "Cook, validate, atomically publish, and adopt a new immutable runtime generation",
            )
        };

        if ui
            .add_enabled(
                can_save && (self.saved_before || dirty),
                egui::Button::new(label),
            )
            .on_hover_text(hint)
            .clicked()
        {
            self.save.request_publish();
        }

        let can_step = self.activity.can_step();
        let mut stepped = false;

        if ui
            .add_enabled(
                can_step && self.history.undo_len() > 0,
                egui::Button::new("Undo"),
            )
            .on_hover_text("Undo the last command (Cmd+Z)")
            .clicked()
        {
            stepped |= self.history.undo(self.objects, self.dense);
        }

        if ui
            .add_enabled(
                can_step && self.history.redo_len() > 0,
                egui::Button::new("Redo"),
            )
            .on_hover_text("Redo the last command (Cmd+Shift+Z)")
            .clicked()
        {
            stepped |= self.history.redo(self.objects, self.dense);
        }

        stepped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_adds_its_own_holds() {
        let idle = SourceActivity::default();
        assert!(idle.can_edit() && idle.can_step() && idle.can_save());

        let gesture = SourceActivity {
            gesture: true,
            ..idle
        };
        assert!(gesture.can_edit() && !gesture.can_step() && !gesture.can_save());

        let failed = SourceActivity {
            write_failed: true,
            ..idle
        };
        assert!(failed.can_step() && !failed.can_save());

        let drafts = idle.with_drafts(true);
        assert!(drafts.can_edit() && !drafts.can_step());

        for blocked in [
            SourceActivity {
                writing: true,
                ..idle
            },
            SourceActivity {
                conflict: true,
                ..idle
            },
        ] {
            assert!(!blocked.can_edit() && !blocked.can_step() && !blocked.can_save());
        }
    }
}
