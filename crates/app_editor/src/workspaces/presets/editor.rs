//! New presets of each kind, and the editor for one preset with its children.
use super::*;
use crate::environment_paint::preset_controls::{kind_name, preset_selector, quick_controls};

use environment::{
    EnvironmentDefinition, Exclusion, GRASS_CHANNEL, GroundTreatment, OutputId, Preset, PresetKind,
    PresetUse, PresetUseId, SurfaceWeight,
};

fn id() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}

pub(crate) fn new_ground(surface: world::TerrainSurfaceId) -> Preset {
    Preset {
        id: PresetId(id()),
        revision: 1,
        name: "New ground".into(),
        kind: PresetKind::Ground(GroundTreatment {
            id: OutputId(id()),
            strength: 1.0,
            surfaces: vec![SurfaceWeight {
                surface,
                weight: 1.0,
            }],
        }),
    }
}

pub(crate) fn new_collection() -> Preset {
    Preset {
        id: PresetId(id()),
        revision: 1,
        name: "New asset collection".into(),
        kind: PresetKind::AssetCollection(environment::AssetCollection {
            id: OutputId(id()),
            channel: environment::ASSET_CHANNEL,
            assets: vec![],
            spacing: 3.0,
            density: 1.0,
            seed: 0,
            max_slope_degrees: 40.0,
            road_clearance: 1.0,
        }),
    }
}

pub(crate) fn new_exclusion() -> Preset {
    Preset {
        id: PresetId(id()),
        revision: 1,
        name: "New exclusion".into(),
        kind: PresetKind::Exclusion(Exclusion {
            id: OutputId(id()),
            channel: GRASS_CHANNEL,
            strength: 1.0,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn preset_editor(
    ui: &mut egui::Ui,
    library: &mut PresetLibrary,
    editing: &mut PresetId,
    definition: &EnvironmentDefinition,
    project: &ProjectEditorStore,
    listings: &mut crate::listings::ProjectListings,
    assets: &[world_db::CollectionAssetView],
) {
    let snapshot = library.clone();
    let Some(preset) = library.presets.iter_mut().find(|p| p.id == *editing) else {
        return;
    };
    ui.label(kind_name(&preset.kind));
    ui.add(egui::TextEdit::singleline(&mut preset.name).char_limit(256));

    match &mut preset.kind {
        PresetKind::AssetCollection(c) => super::collection::editor(ui, c, listings, assets),
        PresetKind::Ground(g) => {
            let label = |id| {
                project
                    .terrain_resources(definition.space)
                    .and_then(|r| r.surfaces.iter().find(|s| s.surface.id == id))
                    .map_or("Unknown surface", |s| s.surface.display_name.as_str())
            };
            let mut remove = None;

            for (i, weight) in g.surfaces.iter_mut().enumerate() {
                ui.push_id(i, |ui| {
                    egui::ComboBox::from_id_salt("surface")
                        .selected_text(label(weight.surface))
                        .show_ui(ui, |ui| {
                            for surface in &definition.surfaces {
                                ui.selectable_value(&mut weight.surface, *surface, label(*surface));
                            }
                        });
                    ui.add(egui::Slider::new(&mut weight.weight, 0.01..=1.0).text("Mix weight"));

                    if i > 0 && ui.small_button("Remove surface").clicked() {
                        remove = Some(i);
                    }
                });
            }

            if let Some(i) = remove {
                g.surfaces.remove(i);
            }

            if g.surfaces.len() < 2
                && let Some(surface) = definition
                    .surfaces
                    .iter()
                    .find(|s| !g.surfaces.iter().any(|w| w.surface == **s))
                && ui.small_button("Add surface").clicked()
            {
                g.surfaces.push(SurfaceWeight {
                    surface: *surface,
                    weight: 1.0,
                });
            }

            ui.add(egui::Slider::new(&mut g.strength, 0.0..=1.0).text("Default influence"));
        }
        PresetKind::Exclusion(e) => {
            let mut channels = std::collections::BTreeMap::new();
            channels.insert(GRASS_CHANNEL, "Grass".to_owned());
            channels.insert(environment::ASSET_CHANNEL, "Asset collections".to_owned());

            for p in &snapshot.presets {
                if let PresetKind::AssetCollection(c) = &p.kind {
                    channels
                        .entry(c.channel)
                        .or_insert_with(|| format!("Assets · {}", p.name));
                }
            }

            egui::ComboBox::from_id_salt("exclusion_channel")
                .selected_text(
                    channels
                        .get(&e.channel)
                        .map_or("Custom channel", String::as_str),
                )
                .show_ui(ui, |ui| {
                    for (id, name) in channels {
                        ui.selectable_value(&mut e.channel, id, name);
                    }
                });
            ui.add(egui::Slider::new(&mut e.strength, 0.0..=1.0).text("Default influence"));
        }
        PresetKind::Composition(children) => {
            ui.small("Children remain shared. Quick settings here become defaults for this use.");
            let mut remove = None;

            for (i, child) in children.iter_mut().enumerate() {
                ui.push_id(child.id, |ui| {
                    ui.separator();
                    ui.add(egui::TextEdit::singleline(&mut child.name).char_limit(256));
                    let previous = child.preset;
                    preset_selector(ui, "child", &mut child.preset, &snapshot, Some(preset.id));

                    if previous != child.preset {
                        child.overrides.clear();
                    }

                    ui.horizontal(|ui| {
                        if ui.small_button("Edit shared child").clicked() {
                            *editing = child.preset;
                        }
                        if ui.small_button("Remove use").clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.collapsing("Use defaults", |ui| {
                        quick_controls(ui, &snapshot, child.preset, &mut child.overrides)
                    });
                });
            }

            if let Some(i) = remove {
                children.remove(i);
            }

            if children.len() < environment::MAX_PRESET_CHILDREN {
                ui.menu_button("Add child preset", |ui| {
                    for p in snapshot.presets.iter().filter(|p| p.id != preset.id) {
                        if ui.button(&p.name).clicked() {
                            children.push(PresetUse {
                                id: PresetUseId(id()),
                                name: p.name.clone(),
                                preset: p.id,
                                overrides: vec![],
                            });
                            ui.close();
                        }
                    }
                });
            }
        }
    }
}
