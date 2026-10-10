use super::*;
use environment::{PresetId, PresetKind, PresetLibrary, PresetOverride, QuickValue};

pub(crate) fn kind_name(kind: &PresetKind) -> &'static str {
    match kind {
        PresetKind::Ground(_) => "Ground",
        PresetKind::Foliage(_) => "Foliage",
        PresetKind::Exclusion(_) => "Exclusion",
        PresetKind::Composition(_) => "Composition",
        PresetKind::AssetCollection(_) => "Asset collection",
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
                        PresetKind::AssetCollection(c) => vec![
                            ("Density", QuickValue::CollectionDensity(c.density)),
                            ("Seed", QuickValue::CollectionSeed(c.seed)),
                        ],
                        PresetKind::Composition(_) => unreachable!(),
                    };
                    for (label, mut value) in values {
                        let index = overrides
                            .iter()
                            .position(|o| o.path == leaf.path && o.value.key() == value.key());
                        ui.push_id(value.key(), |ui| {
                            ui.horizontal(|ui| {
                                let changed = match &mut value {
                                    QuickValue::FoliageSeed(seed)
                                    | QuickValue::CollectionSeed(seed) => {
                                        ui.label(label);
                                        ui.add(egui::DragValue::new(seed)).changed()
                                    }
                                    QuickValue::CollectionDensity(v)
                                    | QuickValue::GroundInfluence(v)
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
