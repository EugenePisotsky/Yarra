//! Display names and save revisions do not change the preview's visual inputs.
use environment::{PresetKind, PresetLibrary, roads::CartTrackProfile};

pub(super) fn same_library(a: &PresetLibrary, b: &PresetLibrary) -> bool {
    a.presets.len() == b.presets.len()
        && a.presets.iter().zip(&b.presets).all(|(a, b)| {
            a.id == b.id
                && match (&a.kind, &b.kind) {
                    (PresetKind::Composition(a), PresetKind::Composition(b)) => {
                        a.len() == b.len()
                            && a.iter().zip(b).all(|(a, b)| {
                                a.id == b.id && a.preset == b.preset && a.overrides == b.overrides
                            })
                    }
                    (a, b) => a == b,
                }
        })
}

pub(super) fn same_road(a: Option<&CartTrackProfile>, b: Option<&CartTrackProfile>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            let mut a = a.clone();
            a.name.clone_from(&b.name);
            a.revision = b.revision;
            a == *b
        }
        (None, None) => true,
        _ => false,
    }
}
