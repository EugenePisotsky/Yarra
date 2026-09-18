use super::*;
use environment::{Preset, PresetId, PresetKind, PresetLibrary};
fn id() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}
use environment::{
    ChannelId, Exclusion, GroundTreatment, OutputId, PresetOverride, PresetUse, PresetUseId,
    QuickValue, SurfaceWeight, VegetationBlend, VegetationTreatment,
};

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
pub(crate) fn new_foliage(assemblage: vegetation::VegetationAssemblageId) -> Preset {
    Preset {
        id: PresetId(id()),
        revision: 1,
        name: "New foliage".into(),
        kind: PresetKind::Foliage(VegetationTreatment {
            id: OutputId(id()),
            channel: ChannelId([1; 16]),
            assemblage,
            blend: VegetationBlend::Replace,
            strength: 1.0,
            density: 1.0,
            seed: 0,
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
            channel: ChannelId([1; 16]),
            strength: 1.0,
        }),
    }
}
pub(crate) fn kind_name(kind: &PresetKind) -> &'static str {
    match kind {
        PresetKind::Ground(_) => "Ground",
        PresetKind::Foliage(_) => "Foliage",
        PresetKind::Exclusion(_) => "Exclusion",
        PresetKind::Composition(_) => "Composition",
    }
}
pub(crate) fn preset_selector(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    selected: &mut PresetId,
    library: &PresetLibrary,
    exclude: Option<PresetId>,
) {
    egui::ComboBox::from_id_salt(salt)
        .selected_text(
            library
                .get(*selected)
                .map_or("Missing preset", |p| p.name.as_str()),
        )
        .show_ui(ui, |ui| {
            for preset in &library.presets {
                if Some(preset.id) != exclude {
                    ui.selectable_value(
                        selected,
                        preset.id,
                        format!("{} · {}", preset.name, kind_name(&preset.kind)),
                    );
                }
            }
        });
}
pub(crate) fn quick_controls(
    ui: &mut egui::Ui,
    library: &PresetLibrary,
    root: PresetId,
    overrides: &mut Vec<PresetOverride>,
) {
    let uses = match library.uses(root, overrides) {
        Ok(uses) => uses,
        Err(e) => {
            ui.colored_label(egui::Color32::LIGHT_RED, e.to_string());
            if !overrides.is_empty() && ui.small_button("Reset all overrides").clicked() {
                overrides.clear();
            }
            return;
        }
    };
    if uses.is_empty() {
        ui.weak("This composition has no outputs yet.");
    }
    for leaf in uses {
        ui.push_id(&leaf.path, |ui| {
            let name = if leaf.names.is_empty() {
                library
                    .get(leaf.preset)
                    .map_or("Preset", |p| p.name.as_str())
                    .to_owned()
            } else {
                leaf.names.join(" / ")
            };
            egui::CollapsingHeader::new(format!("{} · {}", name, kind_name(&leaf.kind)))
                .default_open(leaf.path.is_empty())
                .show(ui, |ui| {
                    let values = match leaf.kind {
                        PresetKind::Ground(g) => {
                            vec![("Ground influence", QuickValue::GroundInfluence(g.strength))]
                        }
                        PresetKind::Foliage(v) => vec![
                            ("Density", QuickValue::FoliageDensity(v.density)),
                            ("Influence", QuickValue::FoliageInfluence(v.strength)),
                            ("Seed", QuickValue::FoliageSeed(v.seed)),
                        ],
                        PresetKind::Exclusion(e) => vec![(
                            "Clearing influence",
                            QuickValue::ExclusionInfluence(e.strength),
                        )],
                        PresetKind::Composition(_) => unreachable!(),
                    };
                    for (label, mut value) in values {
                        let index = overrides
                            .iter()
                            .position(|o| o.path == leaf.path && o.value.key() == value.key());
                        ui.push_id(value.key(), |ui| {
                            ui.horizontal(|ui| {
                                let changed = match &mut value {
                                    QuickValue::FoliageSeed(seed) => {
                                        ui.label(label);
                                        ui.add(egui::DragValue::new(seed)).changed()
                                    }
                                    QuickValue::GroundInfluence(v)
                                    | QuickValue::FoliageDensity(v)
                                    | QuickValue::FoliageInfluence(v)
                                    | QuickValue::ExclusionInfluence(v) => ui
                                        .add(egui::Slider::new(v, 0.0..=1.0).text(label))
                                        .changed(),
                                };
                                if changed {
                                    if let Some(i) = index {
                                        overrides[i].value = value;
                                    } else {
                                        overrides.push(PresetOverride {
                                            path: leaf.path.clone(),
                                            value,
                                        });
                                    }
                                }
                                if let Some(index) = index {
                                    if ui
                                        .small_button("Reset")
                                        .on_hover_text(
                                            "Remove override and inherit the shared default",
                                        )
                                        .clicked()
                                    {
                                        overrides.remove(index);
                                    }
                                } else {
                                    ui.weak("Inherited");
                                }
                            });
                        });
                    }
                });
        });
    }
}
pub(crate) fn preset_editor(
    ui: &mut egui::Ui,
    library: &mut PresetLibrary,
    editing: &mut PresetId,
    definition: &EnvironmentDefinition,
    project: &ProjectEditorStore,
    plants: Option<&vegetation::VegetationCatalog>,
) {
    let snapshot = library.clone();
    let Some(preset) = library.presets.iter_mut().find(|p| p.id == *editing) else {
        return;
    };
    ui.label(kind_name(&preset.kind));
    ui.add(egui::TextEdit::singleline(&mut preset.name).char_limit(256));
    match &mut preset.kind {
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
        PresetKind::Foliage(v) => {
            if let Some(plants) = plants {
                egui::ComboBox::from_id_salt("assemblage")
                    .selected_text(
                        plants
                            .assemblages
                            .iter()
                            .find(|a| a.id == v.assemblage)
                            .map_or("Missing foliage", |a| a.key.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for a in &plants.assemblages {
                            ui.selectable_value(&mut v.assemblage, a.id, &a.key);
                        }
                    });
            }
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut v.blend,
                    VegetationBlend::Replace,
                    "Replace lower grass",
                );
                ui.selectable_value(&mut v.blend, VegetationBlend::Add, "Add grass");
            });
            ui.add(egui::Slider::new(&mut v.density, 0.0..=1.0).text("Default density"));
            ui.add(egui::Slider::new(&mut v.strength, 0.0..=1.0).text("Default influence"));
            ui.horizontal(|ui| {
                ui.label("Default seed");
                ui.add(egui::DragValue::new(&mut v.seed));
            });
        }
        PresetKind::Exclusion(e) => {
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
