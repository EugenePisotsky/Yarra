use super::preset_controls as controls;
use super::*;
use environment::{Layer, PresetId};

pub(super) struct DefinitionForm {
    original: EnvironmentDefinition,
    draft: EnvironmentDefinition,

    layer: Option<LayerId>,
}
impl DefinitionForm {
    fn changed(&self) -> bool {
        self.original != self.draft
    }
}
impl EnvironmentPaintState {
    pub(crate) fn has_unapplied_changes(&self) -> bool {
        self.definition_form
            .as_ref()
            .is_some_and(DefinitionForm::changed)
    }
}
fn id() -> [u8; 16] {
    *uuid::Uuid::new_v4().as_bytes()
}
fn add_layer(definition: &mut EnvironmentDefinition, preset: PresetId) -> LayerId {
    let layer = LayerId(id());
    definition.layers.sort_by_key(|l| (l.order, l.id));
    for (i, l) in definition.layers.iter_mut().enumerate() {
        l.order = i as i32;
    }
    definition.layers.push(Layer {
        id: layer,
        revision: 1,
        name: "New layer".into(),
        preset,
        overrides: vec![],
        order: definition.layers.len() as i32,
        seed: u32::from_le_bytes(id()[..4].try_into().unwrap()),
        enabled: true,
        opacity: 1.0,
    });
    layer
}
fn move_layer(definition: &mut EnvironmentDefinition, layer: LayerId, up: bool) {
    definition.layers.sort_by_key(|l| (l.order, l.id));
    if let Some(i) = definition.layers.iter().position(|l| l.id == layer) {
        let next = if up {
            i.saturating_add(1)
        } else {
            i.saturating_sub(1)
        };
        if next < definition.layers.len() {
            definition.layers.swap(i, next);
        }
    }
    for (i, l) in definition.layers.iter_mut().enumerate() {
        l.order = i as i32;
    }
}
fn commit(
    paint: &mut EnvironmentPaintState,
    dense: &mut DenseDomainWorkingSets,
    history: &mut EditorHistory,
    definition: &EnvironmentDefinition,
) {
    match history.edit_definition(dense, definition) {
        Ok(()) => {
            paint.definition_form = None;
            paint.status = None;
        }
        Err(error) => paint.status = Some(error),
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn inspector(
    ui: &mut egui::Ui,
    paint: &mut EnvironmentPaintState,
    browser: &mut EnvironmentLayerBrowser,
    preview: &EnvironmentPreview,
    _project: &ProjectEditorStore,
    space: Option<WorldSpaceId>,
    history: &mut EditorHistory,
    dense: &mut DenseDomainWorkingSets,
    _plants: Option<&vegetation::VegetationCatalog>,
    busy: bool,
) {
    ui.heading("Environment");
    let (Some(definition), Some(library)) = (
        space.and_then(|s| dense.definition(s)).cloned(),
        dense.presets().cloned(),
    ) else {
        ui.weak("Environment presets are loading…");
        return;
    };
    if paint.space != space {
        paint.space = space;
        paint.selected = None;
        paint.definition_form = None;
    }
    if !definition
        .layers
        .iter()
        .any(|l| Some(l.id) == paint.selected)
    {
        paint.selected = definition
            .layers
            .iter()
            .max_by_key(|l| (l.order, l.id))
            .map(|l| l.id);
        paint.definition_form = None;
    }
    if paint
        .definition_form
        .as_ref()
        .is_some_and(|f| f.original != definition || f.layer != paint.selected)
    {
        paint.definition_form = None;
    }
    let unapplied = paint.has_unapplied_changes();
    let idle = !busy && !dense.gesture_active && !dense.saving() && !dense.has_any_conflict();
    let mut immediate = None;
    ui.horizontal(|ui| {
        ui.selectable_value(&mut browser.all, false, "Nearby");
        ui.selectable_value(&mut browser.all, true, "All");
        if ui
            .add_enabled(idle && !unapplied, egui::Button::new("Presets…"))
            .clicked()
        {
            paint.preset_request = Some((
                definition.space,
                paint.selected.and_then(|id| {
                    definition
                        .layers
                        .iter()
                        .find(|l| l.id == id)
                        .map(|l| l.preset)
                }),
            ));
        }
    });
    ui.add(egui::TextEdit::singleline(&mut browser.search).hint_text("Find layers"));
    let nearby = browser.nearby(dense, definition.space);
    if !browser.all {
        let status = browser.status();
        ui.add(egui::Label::new(egui::RichText::new(&status).weak()).truncate())
            .on_hover_text(status);
        if ui.small_button("Refresh nearby").clicked() {
            browser.refresh();
        }
    }
    ui.label("Layers · highest priority first");
    let mut layers = definition.layers.iter().collect::<Vec<_>>();
    layers.sort_by_key(|l| (l.order, l.id));
    egui::ScrollArea::vertical()
        .id_salt("layer_list")
        .max_height(180.0)
        .show(ui, |ui| {
            let selected = paint.selected;
            for layer in layers
                .into_iter()
                .rev()
                .filter(|l| browser.includes(l, selected, &nearby))
            {
                ui.add_enabled_ui(idle && !unapplied, |ui| {
                    ui.horizontal(|ui| {
                        let mut enabled = layer.enabled;
                        if ui
                            .checkbox(&mut enabled, "")
                            .on_hover_text("Enable this layer")
                            .changed()
                        {
                            let mut next = definition.clone();
                            next.layers
                                .iter_mut()
                                .find(|l| l.id == layer.id)
                                .unwrap()
                                .enabled = enabled;
                            immediate = Some(next);
                        }
                        if ui
                            .selectable_label(paint.selected == Some(layer.id), &layer.name)
                            .clicked()
                        {
                            paint.selected = Some(layer.id);
                            paint.definition_form = None;
                        }
                        if !browser.all
                            && paint.selected == Some(layer.id)
                            && !nearby.contains(&layer.id)
                        {
                            ui.weak("selected · outside area or empty");
                        }
                    });
                });
            }
        });
    ui.add_enabled_ui(idle && !unapplied, |ui| {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!library.presets.is_empty(), egui::Button::new("New layer"))
                .clicked()
            {
                let mut next = definition.clone();

                let preset = paint
                    .selected
                    .and_then(|s| next.layers.iter().find(|l| l.id == s).map(|l| l.preset))
                    .or_else(|| library.presets.first().map(|p| p.id))
                    .unwrap();
                paint.selected = Some(add_layer(&mut next, preset));
                immediate = Some(next);
            }
            if let Some(selected) = paint.selected {
                let min = definition
                    .layers
                    .iter()
                    .min_by_key(|l| (l.order, l.id))
                    .map(|l| l.id);
                let max = definition
                    .layers
                    .iter()
                    .max_by_key(|l| (l.order, l.id))
                    .map(|l| l.id);
                for (label, up, enabled) in [
                    ("Up", true, max != Some(selected)),
                    ("Down", false, min != Some(selected)),
                ] {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        let mut next = definition.clone();
                        move_layer(&mut next, selected, up);
                        immediate = Some(next);
                    }
                }
            }
        });
    });
    if let Some(next) = immediate {
        commit(paint, dense, history, &next);
        return;
    }
    ui.checkbox(&mut paint.show_coverage,"Show selected layer coverage").on_hover_text("Raw painted mask over loaded ground. Dark is empty; cyan is full. Grass is hidden temporarily.");
    ui.separator();
    if let Some(selected) = paint.selected {
        let form = paint.definition_form.get_or_insert_with(|| DefinitionForm {
            original: definition.clone(),
            draft: definition.clone(),
            layer: Some(selected),
        });
        ui.add_enabled_ui(idle, |ui| {
            let layer = form
                .draft
                .layers
                .iter_mut()
                .find(|l| l.id == selected)
                .unwrap();
            ui.collapsing("Layer settings", |ui| {
                ui.add(egui::TextEdit::singleline(&mut layer.name).char_limit(256));
                ui.add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0).text("Opacity"));
                ui.label("Distribution seed");
                ui.add(egui::DragValue::new(&mut layer.seed));
            });
            let previous = layer.preset;
            controls::preset_selector(ui, "environment_preset", &mut layer.preset, &library, None);
            if previous != layer.preset {
                layer.overrides.clear();
            }
            ui.small("Choosing another preset resets this layer's overrides; paint is preserved.");
            egui::CollapsingHeader::new("Quick settings")
                .default_open(true)
                .show(ui, |ui| {
                    controls::quick_controls(ui, &library, layer.preset, &mut layer.overrides);
                });
            if ui
                .add_enabled(!form.changed(), egui::Button::new("Edit shared preset…"))
                .clicked()
            {
                paint.preset_request = Some((
                    definition.space,
                    Some(
                        form.draft
                            .layers
                            .iter()
                            .find(|l| l.id == selected)
                            .unwrap()
                            .preset,
                    ),
                ));
            }
        });
        let changed = form.changed();
        let mut apply = false;
        let mut discard = false;
        if changed {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Apply or discard these settings before painting or saving.",
            );
        }
        ui.horizontal(|ui| {
            apply = ui
                .add_enabled(idle && changed, egui::Button::new("Apply settings"))
                .clicked();
            discard = ui
                .add_enabled(idle && changed, egui::Button::new("Discard"))
                .clicked();
        });
        if apply {
            let next = form.draft.clone();
            commit(paint, dense, history, &next);
        } else if discard {
            paint.definition_form = None;
        }
    } else {
        ui.weak("Create a layer to start painting this world.");
    }
    ui.separator();
    ui.add_enabled_ui(
        idle && !paint.has_unapplied_changes() && paint.selected.is_some(),
        |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut paint.brush.operation, BrushOperation::Paint, "Paint");
                ui.selectable_value(&mut paint.brush.operation, BrushOperation::Erase, "Erase");
            });
            ui.add(egui::Slider::new(&mut paint.brush.radius, 0.5..=16.0).text("Radius (m)"));
            ui.add(egui::Slider::new(&mut paint.brush.strength, 0.01..=1.0).text("Strength"));
            ui.add(egui::Slider::new(&mut paint.brush.falloff, 0.0..=1.0).text("Soft edge"));
        },
    );
    ui.small("Drag on ground to paint. Shift-drag erases; Esc cancels a stroke. Each drag or applied settings edit is one undo step.");
    if let Some(status) = &paint.status {
        ui.colored_label(egui::Color32::YELLOW, status);
    }
    if let Some(error) = &preview.error {
        ui.colored_label(egui::Color32::LIGHT_RED, format!("Preview: {error}"));
    } else if preview.pending() > 0 {
        ui.weak(format!(
            "Updating ground and grass · {} cells",
            preview.pending()
        ));
    } else {
        ui.weak("Ground and grass preview current");
    }
    ui.small("Save stores applied presets, layers and masks together. Save & Publish also updates the game world.");
    ui.collapsing("History", |ui| {
        ui.label(format!("{} undo commands", history.undo_len()));
        if ui
            .add_enabled(
                idle && !paint.has_unapplied_changes(),
                egui::Button::new("Clear undo history"),
            )
            .clicked()
        {
            history.clear();
        }
    });
}
