use super::*;

use crate::{
    activity::{SourceActions, SourceActivity},
    editing::{EditorHistory, EditorObjectWorkingSet, merge_preset_changes},
    environment_paint::{EnvironmentPaintState, preset_controls as controls},
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::EditorUiFrame,
};

use bevy::ecs::system::SystemParam;
use environment::{Preset, PresetKind};

#[derive(SystemParam)]
pub(super) struct Resources<'w> {
    listings: ResMut<'w, crate::listings::ProjectListings>,
    state: ResMut<'w, PresetAuthoringState>,
    preview: Res<'w, viewport::PreviewState>,
    roads: Res<'w, crate::road_authoring::RoadToolState>,
    dense: ResMut<'w, SourceWorkingSets>,
    project: Res<'w, ProjectEditorStore>,
    history: ResMut<'w, EditorHistory>,
    objects: ResMut<'w, EditorObjectWorkingSet>,
    save: ResMut<'w, EditorSaveCoordinator>,
    publication: Res<'w, RuntimePublicationState>,
    paint: Res<'w, EnvironmentPaintState>,
    next: ResMut<'w, NextState<EditorWorkspace>>,
}

/// What the drafts, saving and publication are doing, under the toolbar.
#[allow(clippy::too_many_arguments)]
fn status(
    ui: &mut egui::Ui,
    state: &PresetAuthoringState,
    dense: &SourceWorkingSets,
    objects: &EditorObjectWorkingSet,
    save: &EditorSaveCoordinator,
    publication: &RuntimePublicationState,
    project: &ProjectEditorStore,
    paint: &EnvironmentPaintState,
    drafts: bool,
) {
    if drafts {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Previewing an unapplied draft. Apply updates every use of these shared presets. \
             Drafts stay here when you change workspaces; apply both Environment and Road \
             styles drafts before saving.",
        );
    }

    if let Some(message) = &state.message {
        ui.label(message);
    }

    ui.weak(save.status(dense.dirty_count() + objects.dirty_count()));
    ui.weak(publication.status());

    if let Some(error) = project.write_error() {
        ui.colored_label(egui::Color32::LIGHT_RED, error);
    }

    if let Some(status) = dense.status() {
        ui.colored_label(egui::Color32::YELLOW, status);
    }

    if paint.has_unapplied_changes() {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Apply or discard the layer settings in World before saving.",
        );
    }
}

pub(super) fn draw(mut frame: ResMut<EditorUiFrame>, resources: Resources) -> Result {
    let Resources {
        mut listings,
        mut state,
        preview,
        roads,
        mut dense,
        project,
        mut history,
        mut objects,
        mut save,
        publication,
        paint,
        mut next,
    } = resources;
    let Some(root) = frame.0.as_mut() else {
        return Ok(());
    };
    state.refresh(&dense, &project);
    let Some(mut draft) = state.draft.take() else {
        root.label("Loading preset library…");
        return Ok(());
    };
    let before = draft.library.clone();
    let activity = SourceActivity::of(&project, &save, &publication, &objects, &dense);

    // Applying a draft needs what saving needs, the drafts themselves aside.
    let idle = activity.can_save();
    let road_mode = state.mode == Mode::Roads;
    let active_dirty = if road_mode {
        state.styles.dirty()
    } else {
        draft.dirty()
    };
    let all_dirty = draft.dirty() || state.styles.dirty();
    let mut apply = false;
    let mut discard = false;
    egui::Panel::top("preset_toolbar").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Back to World").clicked() {
                next.set(EditorWorkspace::World);
            }

            let road_ready = roads.ready && preview.matches(&state) && !draft.dirty();
            let can_apply = idle && active_dirty && (!road_mode || road_ready);
            (apply, discard) =
                mode_and_draft_buttons(ui, &mut state, road_mode, can_apply, active_dirty);
            SourceActions {
                activity: activity.with_drafts(all_dirty || paint.has_unapplied_changes()),
                saved_before: project.source_epoch() > 0,
                history: &mut history,
                objects: &mut objects,
                dense: &mut dense,
                save: &mut save,
            }
            .buttons(ui);
        });
        status(
            ui,
            &state,
            &dense,
            &objects,
            &save,
            &publication,
            &project,
            &paint,
            all_dirty,
        );
    });

    // A mode change renders on the next frame. Do not copy preview controls from the
    // destination mode back into the mode whose panels are still being drawn.
    if (state.mode == Mode::Roads) != road_mode {
        state.draft = Some(draft);
        return Ok(());
    }

    if road_mode {
        road_styles::panels(
            root,
            &mut state,
            &dense,
            &draft.library,
            &project,
            idle,
            roads.ready,
        );

        if let Some(error) = &roads.load_error {
            root.colored_label(egui::Color32::LIGHT_RED, error);
        }
    } else {
        library_panel(root, &mut state, &mut draft, idle);
        settings_panel(
            root,
            &mut state,
            &mut draft,
            &dense,
            &project,
            &mut listings,
            &preview,
            idle,
        );
    }

    if (state.mode == Mode::Roads) != road_mode {
        state.draft = Some(draft);
        return Ok(());
    }

    let underlay = preview_panel(root, &mut state, &draft, &dense, &project, &preview);

    if road_mode {
        state.styles.underlay = underlay;
    } else {
        state.underlay = underlay;
    }

    if road_mode {
        if apply && preview.matches(&state) {
            apply_road_style(&mut state, &draft, &mut dense, &project, &mut history);
        } else if discard {
            state.styles.discard();
            state.styles.refresh(&dense.roads);
            state.bump();
            state.message = None;
        }
    } else if apply {
        apply_presets(&mut state, &mut draft, &mut dense, &mut history);
    } else if (discard || (!draft.dirty() && dense.presets().is_some_and(|p| p != &draft.base)))
        && let Some(library) = dense.presets()
    {
        draft = Draft {
            base: library.clone(),
            library: library.clone(),
        };
        state.message = None;
    }

    if !visual_changes::same_library(&before, &draft.library) {
        state.bump();
    }

    state.draft = Some(draft);
    Ok(())
}

/// The Environment and Road styles switch, and Apply and Discard for the shown draft;
/// whether Apply and Discard were clicked.
fn mode_and_draft_buttons(
    ui: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    road_mode: bool,
    can_apply: bool,
    dirty: bool,
) -> (bool, bool) {
    let old_mode = state.mode;
    ui.selectable_value(&mut state.mode, Mode::Environment, "Environment");
    ui.selectable_value(&mut state.mode, Mode::Roads, "Road styles");

    if state.mode != old_mode {
        state.bump();
    }

    let label = if road_mode {
        "Apply road style"
    } else {
        "Apply preset changes"
    };
    let apply = ui
        .add_enabled(can_apply, egui::Button::new(label))
        .clicked();
    let discard = ui
        .add_enabled(dirty, egui::Button::new("Discard draft"))
        .clicked();
    (apply, discard)
}

/// The preset list, with New and Duplicate.
fn library_panel(
    root: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    draft: &mut Draft,
    idle: bool,
) {
    egui::Panel::left("preset_library")
        .default_size(230.0)
        .resizable(true)
        .show(root, |ui| {
            ui.heading("Preset library");
            ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Find presets"));
            let room = draft.library.presets.len() < environment::MAX_PRESETS;
            let added = ui
                .add_enabled_ui(idle && room, |ui| {
                    ui.horizontal(|ui| new_preset_buttons(ui, state, draft))
                        .inner
                })
                .inner;

            if let Some(preset) = added {
                state.select(preset.id);
                draft.library.presets.push(preset);
            }

            egui::ScrollArea::vertical()
                .id_salt("presets_list")
                .show(ui, |ui| {
                    let filter = state.search.to_lowercase();

                    for preset in &draft.library.presets {
                        if !preset.name.to_lowercase().contains(&filter)
                            && state.selected != Some(preset.id)
                        {
                            continue;
                        }

                        let label =
                            format!("{}\n{}", preset.name, controls::kind_name(&preset.kind));

                        if ui
                            .selectable_label(state.selected == Some(preset.id), label)
                            .clicked()
                        {
                            state.select(preset.id);
                        }
                    }
                });
        });
}

/// The New menu and Duplicate; the preset either makes, to add to the library.
fn new_preset_buttons(
    ui: &mut egui::Ui,
    state: &PresetAuthoringState,
    draft: &Draft,
) -> Option<Preset> {
    let mut added = None;
    ui.menu_button("New", |ui| {
        if let Some(base) = state.base
            && ui.button("Ground").clicked()
        {
            added = Some(editor::new_ground(base));
            ui.close();
        }

        if ui.button("Asset collection").clicked() {
            added = Some(editor::new_collection());
            ui.close();
        }

        if ui.button("Exclusion").clicked() {
            added = Some(editor::new_exclusion());
            ui.close();
        }

        if ui.button("Composition").clicked() {
            added = Some(Preset {
                id: PresetId(*uuid::Uuid::new_v4().as_bytes()),
                revision: 1,
                name: "New composition".into(),
                kind: PresetKind::Composition(vec![]),
            });
            ui.close();
        }
    });

    if ui
        .add_enabled(state.selected.is_some(), egui::Button::new("Duplicate"))
        .on_hover_text("Referenced child presets remain shared")
        .clicked()
        && let Some(p) = state.selected.and_then(|id| draft.library.get(id))
    {
        let mut copy = p.clone();
        copy.id = PresetId(*uuid::Uuid::new_v4().as_bytes());
        copy.revision = 1;
        copy.name = format!("{} copy", p.name);
        added = Some(copy);
    }

    added
}

/// The selected preset's settings and where it is used.
#[allow(clippy::too_many_arguments)]
fn settings_panel(
    root: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    draft: &mut Draft,
    dense: &SourceWorkingSets,
    project: &ProjectEditorStore,
    listings: &mut crate::listings::ProjectListings,
    preview: &viewport::PreviewState,
    idle: bool,
) {
    egui::Panel::right("preset_settings")
        .default_size(330.0)
        .resizable(true)
        .show(root, |ui| {
            ui.heading("Shared preset");

            if !state.navigation.is_empty()
                && ui.small_button("Back to parent / previous").clicked()
                && let Some(id) = state.navigation.pop()
            {
                state.selected = Some(id);
                state.bump();
            }

            let (Some(mut selected), Some(definition)) = (
                state.selected,
                state.space.and_then(|space| dense.definition(space)),
            ) else {
                ui.weak("Choose or create a preset.");
                return;
            };
            let usages = usages(selected, &draft.library, dense, project);
            ui.collapsing(format!("Used by {} map layers", usages.len()), |ui| {
                for usage in usages {
                    ui.label(usage);
                }
            });
            ui.small("Defaults are shared. Explicit map overrides keep their values.");
            egui::ScrollArea::vertical()
                .id_salt("preset_details")
                .show(ui, |ui| {
                    ui.add_enabled_ui(idle, |ui| {
                        editor::preset_editor(
                            ui,
                            &mut draft.library,
                            &mut selected,
                            definition,
                            project,
                            listings,
                            &preview.assets,
                        );
                    })
                });

            if Some(selected) != state.selected {
                state.select(selected);
            }
        });
}

/// The map layers that use `preset`, directly or through a composition.
fn usages(
    preset: PresetId,
    library: &environment::PresetLibrary,
    dense: &SourceWorkingSets,
    project: &ProjectEditorStore,
) -> Vec<String> {
    let mut usages = vec![];

    for world in project.environments() {
        let world = dense.definition(world.space).unwrap_or(world);

        for layer in &world.layers {
            let through = library
                .resolve(layer.preset, &[])
                .is_ok_and(|resolved| resolved.dependencies.contains_key(&preset));

            if through || layer.preset == preset {
                usages.push(format!("World {} · {}", world.space.0, layer.name));
            }
        }
    }

    usages
}

/// The preview with its controls; returns the reference ground chosen for it.
fn preview_panel(
    root: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    draft: &Draft,
    dense: &SourceWorkingSets,
    project: &ProjectEditorStore,
    preview: &viewport::PreviewState,
) -> Option<PresetId> {
    let road_mode = state.mode == Mode::Roads;
    let mut underlay = state.preview_underlay();
    egui::CentralPanel::default().show(root, |ui| {
        ui.heading(if road_mode {
            "Road style preview"
        } else {
            "Preset preview"
        });

        if road_mode {
            road_preview_controls(ui, state, draft, dense, project);
        }

        ui.small("Flat test patch · fixed seed · no map overrides. Drag to orbit; scroll to zoom.");
        patch_controls(ui, state);
        ground_controls(ui, state, draft, project, &mut underlay);

        if underlay.is_some() {
            ui.small(if road_mode {
                "Coverage shape applies to the reference ground. Roads only thin the grass \
                 already present."
            } else {
                "Reference ground fills the patch underneath this preset, to test clearing and \
                 blending."
            });
        }

        preview_image(ui, state, preview);
    });
    underlay
}

/// The preview corridor's width and traffic, and what the style needs of them.
fn road_preview_controls(
    ui: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    draft: &Draft,
    dense: &SourceWorkingSets,
    project: &ProjectEditorStore,
) {
    let styles = &mut state.styles;
    let shape =
        |s: &road_styles::Styles| (s.curved, s.width.to_bits(), s.traffic.map(f32::to_bits));
    let before = shape(styles);
    let minimum_width = styles
        .draft
        .as_ref()
        .map_or(0.1, |d| d.value.minimum_width());
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut styles.curved, "Curved road");
        ui.label("Preview corridor width");
        road_styles::bounded_metres(ui, &mut styles.width, minimum_width..=64.0, 0.05);

        if styles.width < minimum_width && ui.button("Fit preview width").clicked() {
            styles.width = minimum_width;
        }

        let mut traffic = styles
            .traffic
            .unwrap_or(environment::roads::DEFAULT_TRAFFIC);
        ui.label("Preview traffic");

        if ui.add(egui::Slider::new(&mut traffic, 0.0..=1.0)).changed() {
            styles.traffic = Some(traffic);
        }
    });
    let changed = before != shape(&state.styles);

    if changed {
        state.bump();
    }

    ui.small(
        "Width, traffic and reference ground are preview-only. Map roads keep their own \
         widths, traffic and painted coverage.",
    );

    if state.styles.width < minimum_width {
        ui.colored_label(
            egui::Color32::YELLOW,
            format!(
                "Preview corridor is {:.2} m wide; this style needs at least \
                 {minimum_width:.2} m. Use Fit preview width.",
                state.styles.width
            ),
        );
    }

    if let Some(d) = &state.styles.draft {
        let valid =
            road_styles::validate_in_project(&d.value, dense, &draft.library, project, state.space);

        if let Err(e) = valid {
            ui.colored_label(egui::Color32::YELLOW, e);
        }

        if d.value.vegetation_channel != environment::GRASS_CHANNEL {
            ui.colored_label(
                egui::Color32::YELLOW,
                "This style clears another channel, so it does not thin grass.",
            );
        }
    }
}

/// Patch size and shape, wind, and the view.
fn patch_controls(ui: &mut egui::Ui, state: &mut PresetAuthoringState) {
    ui.horizontal_wrapped(|ui| {
        let previous_size = state.size;
        egui::ComboBox::from_id_salt("patch_size")
            .selected_text(format!("{} m", state.size))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.size, 8, "8 m");
                ui.selectable_value(&mut state.size, 16, "16 m");
            });

        if state.size != previous_size {
            state.distance *= f32::from(state.size) / f32::from(previous_size);
        }

        egui::ComboBox::from_id_salt("patch_shape")
            .selected_text(state.footprint.label())
            .show_ui(ui, |ui| {
                for f in [Footprint::Full, Footprint::Patch, Footprint::Hole] {
                    ui.selectable_value(&mut state.footprint, f, f.label());
                }
            });
        let wind = if state.playing {
            "Pause wind"
        } else {
            "Play wind"
        };

        if ui.button(wind).clicked() {
            state.playing = !state.playing;
        }

        if ui.button("Reset view").clicked() {
            state.yaw = 35.0;
            state.pitch = 40.0;
            state.distance = state.fit_distance;
            state.phase = 0.0;
        }
    });
}

/// The ground under the patch and the reference preset drawn beneath it.
fn ground_controls(
    ui: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    draft: &Draft,
    project: &ProjectEditorStore,
    underlay: &mut Option<PresetId>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Base ground");

        if let Some(space) = state.space
            && let Some(resources) = project.terrain_resources(space)
        {
            let name = resources
                .surfaces
                .iter()
                .find(|s| Some(s.surface.id) == state.base)
                .map_or("Choose", |s| s.surface.display_name.as_str());
            egui::ComboBox::from_id_salt("preset_base")
                .selected_text(name)
                .show_ui(ui, |ui| {
                    for surface in &resources.surfaces {
                        ui.selectable_value(
                            &mut state.base,
                            Some(surface.surface.id),
                            &surface.surface.display_name,
                        );
                    }
                });
        }

        ui.label("Reference ground");
        let name = underlay
            .and_then(|id| draft.library.get(id))
            .map_or("None", |p| p.name.as_str());
        egui::ComboBox::from_id_salt("preset_underlay")
            .selected_text(name)
            .show_ui(ui, |ui| {
                ui.selectable_value(underlay, None, "None");

                for p in &draft.library.presets {
                    if matches!(p.kind, PresetKind::Ground(_) | PresetKind::Composition(_)) {
                        ui.selectable_value(underlay, Some(p.id), &p.name);
                    }
                }
            });
    });
}

/// The rendered patch: its status line and the image, which orbits on drag and zooms on
/// scroll.
fn preview_image(
    ui: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    preview: &viewport::PreviewState,
) {
    let status = if let Some(error) = &preview.error {
        format!("Preview unavailable: {error}")
    } else if !preview.has_image() {
        "Preparing preview…".into()
    } else if preview.slow_update(std::time::Instant::now()) {
        "Updating preview · showing previous result".into()
    } else {
        format!(
            "Preview · {} objects · up to {} grass candidates",
            preview.objects, preview.candidates
        )
    };
    let text = egui::RichText::new(&status);
    let text = if preview.error.is_some() {
        text.color(egui::Color32::LIGHT_RED)
    } else {
        text.weak()
    };
    ui.add(egui::Label::new(text).truncate())
        .on_hover_text(status);
    let available = ui.available_size();
    let width = available
        .x
        .min((available.y - 8.0).max(1.0) * 4.0 / 3.0)
        .max(1.0);
    let response = ui.add(
        egui::Image::new((state.texture, egui::vec2(width, width * 0.75)))
            .sense(egui::Sense::drag()),
    );

    if preview.error.is_some() && preview.has_image() {
        ui.painter().text(
            response.rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            "Last valid preview",
            egui::FontId::proportional(16.0),
            egui::Color32::WHITE,
        );
    }

    if response.dragged() {
        let delta = ui.input(|i| i.pointer.delta());
        state.yaw -= delta.x * 0.4;
        state.pitch = (state.pitch + delta.y * 0.3).clamp(5.0, 89.0);
    }

    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        state.distance = (state.distance * (-scroll * 0.002).exp()).clamp(2.0, 80.0);
    }
}

fn apply_road_style(
    state: &mut PresetAuthoringState,
    draft: &Draft,
    dense: &mut SourceWorkingSets,
    project: &ProjectEditorStore,
    history: &mut EditorHistory,
) {
    let space = state.space;
    let Some(d) = &mut state.styles.draft else {
        return;
    };
    let result = road_styles::validate_in_project(&d.value, dense, &draft.library, project, space)
        .and_then(|_| d.apply(dense, history));
    state.message = Some(match result {
        Ok(()) => "Road style applied to all its uses. Save persists it; Save & Publish updates \
                   the game."
            .into(),
        Err(e) => e,
    });
}

fn apply_presets(
    state: &mut PresetAuthoringState,
    draft: &mut Draft,
    dense: &mut SourceWorkingSets,
    history: &mut EditorHistory,
) {
    let applied = merge_preset_changes(dense.presets(), &draft.base, &draft.library)
        .and_then(|library| history.edit_presets(dense, &library));

    match applied {
        Ok(()) => {
            let library = dense.presets().unwrap().clone();
            *draft = Draft {
                base: library.clone(),
                library,
            };
            state.message = Some(
                "Preset changes applied. Save persists them; Save & Publish updates the game."
                    .into(),
            );
        }
        Err(error) => state.message = Some(error),
    }
}
