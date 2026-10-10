//! Stable-ID editor working state and command history layered over the bounded source cache.

mod history;
mod objects;
mod selection;
mod transform;

pub(crate) use history::{EditorHistory, merge_preset_changes};
pub(crate) use objects::{
    DirtyObjectSnapshot, EditorObjectWorkingSet, process_project_save_completion,
};
pub(crate) use selection::EditorSelection;
pub(crate) use transform::{TransformInspectorDraft, normalize_transform};
