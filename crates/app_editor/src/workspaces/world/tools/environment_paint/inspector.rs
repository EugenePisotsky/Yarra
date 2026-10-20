use super::preset_controls as controls;
use super::*;
use environment::{Layer, PresetId};
use std::collections::BTreeSet;

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
        imported_mask: None,
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
    dense: &mut SourceWorkingSets,
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
    space: Option<WorldSpaceId>,
    history: &mut EditorHistory,
    dense: &mut SourceWorkingSets,
    editable: bool,
) {
    ui.heading("Environment");
    let (Some(definition), Some(library)) = (
        space.and_then(|s| dense.definition(s)).cloned(),
        dense.presets().cloned(),
    ) else {
        ui.weak("Environment presets are loading…");
        return;
    };
    follow_definition(paint, space, &definition);

    // Layers change at once only while no layer settings wait to be applied.
    let layers_editable = editable && !paint.has_unapplied_changes();
    browser_bar(ui, paint, browser, &definition, layers_editable);
    let nearby = browser.nearby(dense, definition.space);
    let changed = ui
        .add_enabled_ui(layers_editable, |ui| {
            let listed = layer_list(ui, paint, browser, &definition, &nearby);
            let arranged = ui
                .horizontal(|ui| layer_buttons(ui, paint, &definition, &library))
                .inner;
            listed.or(arranged)
        })
        .inner;

    if let Some(next) = changed {
        commit(paint, dense, history, &next);
        return;
    }

    ui.checkbox(&mut paint.show_coverage, "Show selected layer coverage")
        .on_hover_text(
            "Raw painted mask over loaded ground. Dark is empty; cyan is full. Grass is hidden \
             temporarily.",
        );
    ui.separator();

    if let Some(selected) = paint.selected {
        layer_form(
            ui,
            paint,
            dense,
            history,
            &definition,
            &library,
            selected,
            editable,
        );
    } else {
        ui.weak("Create a layer to start painting this world.");
    }

    ui.separator();
    brush(ui, paint, &definition, editable);
    footer(ui, paint, preview, history, editable);
}

/// Another world, a removed layer or a definition changed under the form start over.
fn follow_definition(
    paint: &mut EnvironmentPaintState,
    space: Option<WorldSpaceId>,
    definition: &EnvironmentDefinition,
) {
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
        .is_some_and(|f| f.original != *definition || f.layer != paint.selected)
    {
        paint.definition_form = None;
    }
}

/// Nearby or all layers, the Presets workspace, the search and the nearby status.
fn browser_bar(
    ui: &mut egui::Ui,
    paint: &mut EnvironmentPaintState,
    browser: &mut EnvironmentLayerBrowser,
    definition: &EnvironmentDefinition,
    editable: bool,
) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut browser.all, false, "Nearby");
        ui.selectable_value(&mut browser.all, true, "All");

        if ui
            .add_enabled(editable, egui::Button::new("Presets…"))
            .clicked()
        {
            let preset = paint.selected.and_then(|id| {
                definition
                    .layers
                    .iter()
                    .find(|l| l.id == id)
                    .map(|l| l.preset)
            });
            paint.preset_request = Some((definition.space, preset));
        }
    });
    ui.add(egui::TextEdit::singleline(&mut browser.search).hint_text("Find layers"));

    if !browser.all {
        let status = browser.status();
        ui.add(egui::Label::new(egui::RichText::new(&status).weak()).truncate())
            .on_hover_text(status);

        if ui.small_button("Refresh nearby").clicked() {
            browser.refresh();
        }
    }
}

/// The layers, highest priority first, to select or switch on and off; returns the
/// definition with a layer switched.
fn layer_list(
    ui: &mut egui::Ui,
    paint: &mut EnvironmentPaintState,
    browser: &EnvironmentLayerBrowser,
    definition: &EnvironmentDefinition,
    nearby: &BTreeSet<LayerId>,
) -> Option<EnvironmentDefinition> {
    ui.label("Layers · highest priority first");
    let mut layers = definition.layers.iter().collect::<Vec<_>>();
    layers.sort_by_key(|l| (l.order, l.id));
    let mut changed = None;
    egui::ScrollArea::vertical()
        .id_salt("layer_list")
        .max_height(180.0)
        .show(ui, |ui| {
            let selected = paint.selected;
            let shown = layers
                .into_iter()
                .rev()
                .filter(|l| browser.includes(l, selected, nearby));

            for layer in shown {
                ui.horizontal(|ui| {
                    let mut enabled = layer.enabled;

                    if ui
                        .checkbox(&mut enabled, "")
                        .on_hover_text("Enable this layer")
                        .changed()
                    {
                        let mut next = definition.clone();
                        let switched = next.layers.iter_mut().find(|l| l.id == layer.id);
                        switched.unwrap().enabled = enabled;
                        changed = Some(next);
                    }

                    if ui
                        .selectable_label(paint.selected == Some(layer.id), &layer.name)
                        .clicked()
                    {
                        paint.selected = Some(layer.id);
                        paint.definition_form = None;
                    }

                    if let Some(mask) = &layer.imported_mask {
                        ui.weak(format!("· imports {mask}"));
                    }

                    if !browser.all
                        && paint.selected == Some(layer.id)
                        && !nearby.contains(&layer.id)
                    {
                        ui.weak("selected · outside area or empty");
                    }
                });
            }
        });
    changed
}

/// New layer, and Up and Down for the selected one; returns the changed definition.
fn layer_buttons(
    ui: &mut egui::Ui,
    paint: &mut EnvironmentPaintState,
    definition: &EnvironmentDefinition,
    library: &environment::PresetLibrary,
) -> Option<EnvironmentDefinition> {
    let mut changed = None;

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
        changed = Some(next);
    }

    let Some(selected) = paint.selected else {
        return changed;
    };
    let order = |l: &&Layer| (l.order, l.id);
    let lowest = definition.layers.iter().min_by_key(order).map(|l| l.id);
    let highest = definition.layers.iter().max_by_key(order).map(|l| l.id);

    for (label, up, enabled) in [
        ("Up", true, highest != Some(selected)),
        ("Down", false, lowest != Some(selected)),
    ] {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            let mut next = definition.clone();
            move_layer(&mut next, selected, up);
            changed = Some(next);
        }
    }

    changed
}

/// The selected layer's settings, edited as a draft that Apply commits as one undo step.
#[allow(clippy::too_many_arguments)]
fn layer_form(
    ui: &mut egui::Ui,
    paint: &mut EnvironmentPaintState,
    dense: &mut SourceWorkingSets,
    history: &mut EditorHistory,
    definition: &EnvironmentDefinition,
    library: &environment::PresetLibrary,
    selected: LayerId,
    editable: bool,
) {
    let form = paint.definition_form.get_or_insert_with(|| DefinitionForm {
        original: definition.clone(),
        draft: definition.clone(),
        layer: Some(selected),
    });
    let mut edit_preset = false;
    ui.add_enabled_ui(editable, |ui| {
        let layer = form
            .draft
            .layers
            .iter_mut()
            .find(|l| l.id == selected)
            .unwrap();
        ui.collapsing("Layer settings", |ui| layer_settings(ui, layer));
        let previous = layer.preset;
        controls::preset_selector(ui, "environment_preset", &mut layer.preset, library, None);

        if previous != layer.preset {
            layer.overrides.clear();
        }

        ui.small("Choosing another preset resets this layer's overrides; paint is preserved.");
        egui::CollapsingHeader::new("Quick settings")
            .default_open(true)
            .show(ui, |ui| {
                controls::quick_controls(ui, library, layer.preset, &mut layer.overrides);
            });
        let unchanged = !form.changed();
        edit_preset = ui
            .add_enabled(unchanged, egui::Button::new("Edit shared preset…"))
            .clicked();
    });

    if edit_preset {
        let layer = form.draft.layers.iter().find(|l| l.id == selected);
        paint.preset_request = Some((definition.space, layer.map(|l| l.preset)));
    }

    let Some(form) = &paint.definition_form else {
        return;
    };
    let changed = form.changed();
    let valid = form.draft.layers.iter().all(|l| {
        l.imported_mask
            .as_deref()
            .is_none_or(environment::valid_mask_name)
    });

    if changed {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Apply or discard these settings before painting or saving.",
        );
    }

    let (apply, discard) = ui
        .horizontal(|ui| {
            let apply = ui
                .add_enabled(
                    editable && changed && valid,
                    egui::Button::new("Apply settings"),
                )
                .clicked();
            let discard = ui
                .add_enabled(editable && changed, egui::Button::new("Discard"))
                .clicked();
            (apply, discard)
        })
        .inner;

    if apply {
        let next = form.draft.clone();
        commit(paint, dense, history, &next);
    } else if discard {
        paint.definition_form = None;
    }
}

/// Name, opacity, seed and whether coverage comes from a terrain import mask.
fn layer_settings(ui: &mut egui::Ui, layer: &mut Layer) {
    ui.add(egui::TextEdit::singleline(&mut layer.name).char_limit(256));
    ui.add(egui::Slider::new(&mut layer.opacity, 0.0..=1.0).text("Opacity"));
    ui.label("Distribution seed");
    ui.add(egui::DragValue::new(&mut layer.seed));
    let mut imported = layer.imported_mask.is_some();

    if ui
        .checkbox(&mut imported, "Coverage from terrain import")
        .on_hover_text(
            "Each terrain import rewrites this layer's coverage from the named mask, so it \
             cannot be painted. Paint touch-ups on a layer above it.",
        )
        .changed()
    {
        layer.imported_mask = imported.then(String::new);
    }

    let Some(mask) = &mut layer.imported_mask else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("Mask");
        ui.add(
            egui::TextEdit::singleline(mask)
                .char_limit(32)
                .hint_text("sand"),
        );
    });

    if !environment::valid_mask_name(mask) {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Use lowercase letters, digits and underscores, starting with a letter.",
        );
    }

    ui.small("The next terrain import fills it; until then it keeps its coverage.");
}

/// Paint or erase, radius, strength and soft edge; not for imported layers.
fn brush(
    ui: &mut egui::Ui,
    paint: &mut EnvironmentPaintState,
    definition: &EnvironmentDefinition,
    editable: bool,
) {
    let imported = paint.selected.is_some_and(|selected| {
        definition
            .layers
            .iter()
            .any(|l| l.id == selected && l.imported_mask.is_some())
    });

    if imported {
        ui.weak("This layer's coverage comes from the terrain import.");
    }

    let paintable =
        editable && !paint.has_unapplied_changes() && paint.selected.is_some() && !imported;
    ui.add_enabled_ui(paintable, |ui| {
        let brush = &mut paint.brush;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut brush.operation, BrushOperation::Paint, "Paint");
            ui.selectable_value(&mut brush.operation, BrushOperation::Erase, "Erase");
        });
        ui.add(egui::Slider::new(&mut brush.radius, 0.5..=16.0).text("Radius (m)"));
        ui.add(egui::Slider::new(&mut brush.strength, 0.01..=1.0).text("Strength"));
        ui.add(egui::Slider::new(&mut brush.falloff, 0.0..=1.0).text("Soft edge"));
    });
    ui.small(
        "Drag on ground to paint. Shift-drag erases; Esc cancels a stroke. Each drag or applied \
         settings edit is one undo step.",
    );
}

/// The paint status, the preview's progress and the undo history.
fn footer(
    ui: &mut egui::Ui,
    paint: &EnvironmentPaintState,
    preview: &EnvironmentPreview,
    history: &mut EditorHistory,
    editable: bool,
) {
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

    ui.small(
        "Save stores applied presets, layers and masks together. Save & Publish also updates \
         the game world.",
    );
    ui.collapsing("History", |ui| {
        ui.label(format!("{} undo commands", history.undo_len()));

        if ui
            .add_enabled(
                editable && !paint.has_unapplied_changes(),
                egui::Button::new("Clear undo history"),
            )
            .clicked()
        {
            history.clear();
        }
    });
}
