use super::*;
use crate::{
    project_store::ProjectEditorStore,
    saving::EditorSaveCoordinator,
    shell::EditorUiFrame,
    tools::EditorToolRegistry,
    vegetation_authoring::{draw_population_colors, draw_vegetation_authoring},
};

#[allow(clippy::too_many_arguments)]
pub(super) fn ui(
    mut frame: ResMut<EditorUiFrame>,
    mut state: ResMut<StudyState>,
    mut tools: ResMut<EditorToolRegistry>,
    mut authoring: ResMut<VegetationAuthoringState>,
    diagnostics: Res<VegetationDiagnostics>,
    mut settings: ResMut<VegetationDebugSettings>,
    mut lighting: ResMut<VegetationLighting>,
    mut save: ResMut<EditorSaveCoordinator>,
    mut project: ResMut<ProjectEditorStore>,
    ground: Res<ground::StudyGroundAssets>,
) {
    let Some(root) = frame.0.as_mut() else {
        return;
    };
    let context = root.ctx().clone();
    let workspace = root.available_rect_before_wrap();
    let enabled = state.pending == 0;
    if enabled {
        for drop in context.input(|i| i.raw.dropped_files.clone()) {
            if let Some(path) = drop.path {
                state.refs.error = state.refs.import(&path).err();
                if state.refs.error.is_none() {
                    let index = state.refs.selected;
                    state.match_reference(index);
                }
            }
        }
    } else {
        root.style_mut().visuals.disabled_alpha = 1.0;
        root.disable();
    }
    state.refs.prepare_textures(&context);

    egui::Panel::top("study-toolbar").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Grass study");
            egui::ComboBox::from_id_salt("study-field-size").selected_text(format!("{} × {} m", state.field_size, state.field_size)).show_ui(ui, |ui| {
                for size in stage::FIELD_SIZES {
                    ui.selectable_value(&mut state.field_size, size, format!("{size} × {size} m{}", if size >= 64.0 { " · LOD field" } else { "" }));
                }
            });
            egui::ComboBox::from_id_salt("study-ground").selected_text(state.ground.label()).show_ui(ui, |ui| {
                for mode in [GroundMode::Meadow, GroundMode::Dried, GroundMode::Neutral,
                    GroundMode::OriginalStudy, GroundMode::DarkenedStudy,
                    GroundMode::UnderstoryStudy, GroundMode::CoverageStudy,
                    GroundMode::CanopyGroundStudy] {
                    ui.selectable_value(&mut state.ground, mode, mode.label());
                }
            });
            let mut edge_enabled = state.edge.is_some();
            if ui.checkbox(&mut edge_enabled, "Grass edge").changed() {
                state.edge = edge_enabled.then(GrassEdge::default);
                state.camera_label = "Custom setup".into();
            }
            ui.separator();
            ui.toggle_value(&mut state.show_picker, "References…");
            ui.toggle_value(&mut state.show_inspector, "Inspector…");
            ui.toggle_value(&mut state.show_colors, "Colors…");
            if ui.toggle_value(&mut state.show_canopy, "Canopy…").clicked() && state.show_canopy { state.ground = GroundMode::CanopyGroundStudy; }
            if ui.button("Hide windows").clicked() { state.show_picker = false; state.show_inspector = false; state.show_colors = false; state.show_canopy = false; }
            ui.checkbox(&mut state.show_reference, "Compare");
            ui.checkbox(&mut state.show_character, "Character");
            ui.checkbox(&mut state.show_ruler, "2 m ruler");
            ui.separator();
            if ui.button("Save study").on_hover_text("Save local reproduction data, including linked zoom. Catalog Save/Publish is in the inspector.").clicked() { state.save_study = true; }
            if ui.button("Capture").clicked() {
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                state.capture_dir = Some(directory().join("captures").join(stamp.to_string()));
                state.ready_frames = 0; state.playing = false;
            }
        });
        ui.horizontal_wrapped(|ui| {
            let mut zoom = state.comparison.zoom;
            if ui.add(egui::Slider::new(&mut zoom, 1.0..=6.0).text("Linked zoom").suffix("×")).changed() {
                state.comparison.zoom_at(zoom, egui::Vec2::splat(0.5));
            }
            if ui.button("Fit both").clicked() { state.comparison = ComparisonView::default(); }
            if ui.button("Match reference setup").clicked() { let index = state.refs.selected; state.match_reference(index); }
            ui.weak("Scroll either image to zoom both · drag reference / Shift-drag to pan both");
        });
    });
    egui::Panel::top("study-shape-inspection").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Blade bands");
            let previous_bands = settings.blade_bands;
            egui::ComboBox::from_id_salt("study-blade-bands")
                .selected_text(previous_bands.label())
                .show_ui(ui, |ui| {
                    for mode in VegetationBladeBands::ALL {
                        ui.selectable_value(&mut settings.blade_bands, mode, mode.label());
                    }
                }).response.on_hover_text("Experimental moving shadow marks. Approximate height/density gating; no real occluder test. Ground is unchanged.");
            if previous_bands != settings.blade_bands { state.ready_frames = 0; }
            ui.separator();
            ui.label("Shape diagnosis");
            let previous = settings.shape_inspection;
            egui::ComboBox::from_id_salt("shape-inspection")
                .selected_text(previous.label())
                .show_ui(ui, |ui| {
                    for mode in VegetationShapeInspection::ALL {
                        ui.selectable_value(&mut settings.shape_inspection, mode, mode.label());
                    }
                });
            if settings.shape_inspection != previous {
                state.playing = false;
                state.ready_frames = 0;
                if settings.shape_inspection != VegetationShapeInspection::Off {
                    state.field_size = state.field_size.min(16.0);
                    settings.mode = vegetation_render::VegetationDebugMode::ProceduralGeometry;
                }
            }
            if ui.button("Game close camera").clicked() {
                state.camera = StudyCamera::preset("game-close").unwrap();
                state.camera_label = "Game close · default pose".into();
                state.character = CharacterPlacement {
                    xz: [0.0, 0.0],
                    yaw: 0.0,
                };
                state.show_character = true;
                state.playing = false;
                state.ready_frames = 0;
            }
            if settings.shape_inspection != VegetationShapeInspection::Off {
                ui.checkbox(
                    &mut settings.inspection_disable_opening,
                    "Disable view opening",
                );
                ui.weak(
                    "Same retained roots · high topology in all comparison views · 16 m maximum",
                );
            }
        });
        match settings.shape_inspection {
            VegetationShapeInspection::Morph => {
                ui.label("Green = full shape · yellow = intermediate · red = low shape");
            }
            VegetationShapeInspection::Cause => {
                ui.label(
                    "Green = full shape · orange = budget limited · blue = screen-size limited",
                );
            }
            _ => {}
        }
    });
    // A reference selection or field change can leave the bounded inspection domain.
    if state.field_size > 16.0 && settings.shape_inspection != VegetationShapeInspection::Off {
        settings.shape_inspection = VegetationShapeInspection::Off;
        state.message = "Returned to production LOD for the larger field".into();
    }
    egui::Panel::bottom("study-transport").show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Wind");
            ui.checkbox(&mut state.wind.enabled, "Enabled");
            let label = if state.playing { "Pause" } else { "Play" };
            if ui.button(label).clicked() {
                state.playing = !state.playing;
            }
            if ui.button("Reset").clicked() {
                state.wind.time = 0.0;
                state.playing = false;
            }
            if ui.button("+1/60 s").clicked() {
                state.wind.time = (state.wind.time + 1.0 / 60.0).rem_euclid(4096.0);
                state.playing = false;
            }
            if ui
                .add(
                    egui::DragValue::new(&mut state.wind.time)
                        .speed(0.01)
                        .range(0.0..=4095.99)
                        .suffix(" s"),
                )
                .changed()
            {
                state.playing = false;
            }
            ui.add(egui::Slider::new(&mut state.playback_speed, 0.1..=2.0).text("Speed"));
        });
        let stats = diagnostics.snapshot();
        let dropped: u32 = stats.capacity_dropped_instances.iter().sum();
        ui.horizontal_wrapped(|ui| {
            ui.small(format!(
                "{} × {} · MSAA {}× · {}",
                state.render_size[0],
                state.render_size[1],
                state.msaa_samples,
                settings.shape_inspection.label()
            ));
            if state.ready_frames >= 65 && stats.gpu_samples > 1 {
                ui.small(format!(
                    "{} source candidates · {} emitted units · {dropped} capacity drops",
                    state.candidates,
                    stats.emitted_instances.iter().sum::<u32>()
                ));
            } else {
                ui.small("Waiting for GPU counts…");
            }
            if dropped > 0 {
                ui.colored_label(egui::Color32::LIGHT_RED, "Capacity clipped");
            }
            if state.field_size >= 64.0 && stats.gpu_samples > 1 {
                let emitted = stats.emitted_instances;
                ui.small(format!(
                    "High {} · Low {} · {} pages",
                    emitted[0] + emitted[2],
                    emitted[1] + emitted[3],
                    stats.source_pages
                ));
            }
        });
        if state.ground != GroundMode::Neutral {
            if let Some(error) = &ground.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            } else if !ground.ready {
                ui.small("Loading production ground textures…");
            }
        }
        if let Some(error) = state.error.as_ref().or(state.refs.error.as_ref()) {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        } else if !state.message.is_empty() {
            ui.small(&state.message);
        }
    });
    egui::CentralPanel::default().show(root, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(&state.camera_label);
            ui.separator(); ui.weak("Camera:");
            for (key, label) in [("low", "Low"), ("overhead", "Overhead"), ("top", "Top"), ("scale", "Scale")] {
                if ui.small_button(label).clicked() {
                    state.camera = StudyCamera::preset(key).unwrap(); state.camera_label = format!("{label} · inspection camera");
                    state.comparison = ComparisonView::default();
                    if key == "scale" { state.show_character = true; }
                }
            }
            let fov_changed = ui.add(egui::DragValue::new(&mut state.camera.fov).range(15.0..=100.0).suffix("° FOV")).changed();
            let distance_changed = ui.add(egui::DragValue::new(&mut state.camera.distance).speed(0.02).range(0.3..=30.0).suffix(" m distance")).changed();
            if fov_changed || distance_changed { state.camera_label = "Custom camera".into(); }
            ui.weak(format!("eye {:.2} m", state.camera.transform().translation.y));
        });
        ui.small("Drag Yarra to orbit · right-drag Yarra to move camera target · linked zoom magnifies the images");
        let mut area = ui.available_rect_before_wrap();
        area.min.y += 34.0;
        area.max.y -= 18.0;
        let aspect = state.render_size[0] as f32 / state.render_size[1] as f32;
        let rects = comparison::image_rects(area, aspect, state.show_reference);
        let mut responses = Vec::new();
        for (index, rect) in rects.iter().enumerate() {
            responses.push(ui.interact(*rect, ui.id().with(("comparison-image", index)), egui::Sense::click_and_drag()));
        }
        // Apply input once, then paint both images with the very same updated crop in this frame.
        for (index, response) in responses.iter().enumerate() {
            let reference = state.show_reference && index == 0;
            if response.hovered() && enabled {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    let anchor = (response.hover_pos().unwrap_or(response.rect.center()) - response.rect.min) / response.rect.size();
                    let zoom = state.comparison.zoom * (scroll * 0.003).exp();
                    state.comparison.zoom_at(zoom, anchor);
                }
            }
            let shift = ui.input(|i| i.modifiers.shift);
            if response.dragged_by(egui::PointerButton::Primary) && enabled {
                let delta = ui.input(|i| i.pointer.delta());
                if reference || shift { state.comparison.pan(delta, response.rect.size()); }
                else {
                    state.camera.yaw -= delta.x * 0.3;
                    state.camera.pitch = (state.camera.pitch + delta.y * 0.3).clamp(3.0, 89.5);
                    state.camera_label = "Custom camera".into();
                }
            }
            if !reference && response.dragged_by(egui::PointerButton::Secondary) && enabled {
                let delta = ui.input(|i| i.pointer.delta());
                let transform = state.camera.transform();
                let change = (-transform.right() * delta.x + transform.up() * delta.y) * (state.camera.distance * 0.0015);
                state.camera.target = (Vec3::from_array(state.camera.target) + change).clamp(Vec3::splat(-20.0), Vec3::splat(20.0)).to_array();
                state.camera_label = "Custom camera".into();
            }
        }
        for (index, rect) in rects.iter().enumerate() {
            let reference = state.show_reference && index == 0;
            ui.painter().rect_filled(*rect, 0.0, egui::Color32::from_gray(12));
            let source = if reference {
                state.refs.texture().map(|t| (t.id(), t.size_vec2()))
            } else { Some((state.texture, egui::vec2(state.render_size[0] as f32, state.render_size[1] as f32))) };
            if let Some((texture, size)) = source {
                ui.painter().image(texture, *rect, state.comparison.uv(size, rect.size()), egui::Color32::WHITE);
            } else {
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Choose a reference image", egui::FontId::proportional(18.0), egui::Color32::GRAY);
            }
            let title = if reference { format!("REFERENCE  ·  {}", state.refs.title()) } else { "YARRA  ·  LIVE".into() };
            ui.painter().text(rect.left_top() - egui::vec2(0.0, 12.0), egui::Align2::LEFT_BOTTOM, title, egui::FontId::proportional(16.0), egui::Color32::from_gray(210));
            ui.painter().rect_stroke(*rect, 0.0, egui::Stroke::new(1.0, egui::Color32::from_gray(60)), egui::StrokeKind::Inside);
        }
    });

    // Independent egui windows do not reserve canvas space and can be closed or moved aside.
    let mut picker_open = state.show_picker;
    let mut selection = None;
    egui::Window::new("Reference picker")
        .id(egui::Id::new("study-reference-picker"))
        .open(&mut picker_open)
        .default_pos(workspace.left_top() + egui::vec2(20.0, 100.0))
        .default_size(egui::vec2(380.0, 550.0))
        .max_height((workspace.height() - 160.0).max(140.0))
        .constrain_to(workspace)
        .resizable(true)
        .vscroll(false)
        .show(&context, |ui| {
            if !enabled {
                ui.style_mut().visuals.disabled_alpha = 1.0;
                ui.disable();
            }
            selection = state.refs.picker_ui(ui);
            ui.separator();
            ui.label(format!("Current: {}", state.refs.title()));
            if ui
                .button("Save setup for this reference")
                .on_hover_text("Save camera, character placement, field size and ground locally.")
                .clicked()
            {
                let setup = state.reference_setup();
                match state.refs.save_setup(&setup) {
                    Ok(()) => {
                        state.camera_label = format!("{} · saved setup", state.refs.title());
                        state.message = "Reference setup saved locally".into();
                    }
                    Err(error) => state.refs.error = Some(error),
                }
            }
        });
    state.show_picker = picker_open;
    if let Some(index) = selection {
        state.match_reference(index);
        state.show_picker = false;
    }

    let mut colors_open = state.show_colors;
    egui::Window::new("Grass colors")
        .id(egui::Id::new("study-floating-colors"))
        .open(&mut colors_open)
        .default_pos(egui::pos2(workspace.right() - 400.0, workspace.top() + 110.0))
        .default_width(380.0)
        .max_height((workspace.height() - 170.0).max(200.0))
        .constrain_to(workspace)
        .vscroll(true)
        .show(&context, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                draw_population_colors(ui, &mut authoring);
                ui.separator();
                ui.small("Colors preview live. Save study keeps this experiment locally. Inspector → Save & Publish applies catalog edits to the game.");
                if ui.button("Save study").clicked() { state.save_study = true; }
            });
        });
    state.show_colors = colors_open;

    let mut inspector_open = state.show_inspector;
    egui::Window::new("Vegetation inspector")
        .id(egui::Id::new("study-floating-inspector"))
        .open(&mut inspector_open)
        .default_pos(egui::pos2(
            workspace.right() - 430.0,
            workspace.top() + 100.0,
        ))
        .default_size(egui::vec2(410.0, (workspace.height() - 170.0).max(200.0)))
        .max_height((workspace.height() - 170.0).max(200.0))
        .constrain_to(workspace)
        .resizable(true)
        .vscroll(true)
        .show(&context, |ui| {
            if !enabled {
                ui.style_mut().visuals.disabled_alpha = 1.0;
                ui.disable();
            }
            ui.horizontal(|ui| {
                ui.label("Study seed");
                ui.add(egui::DragValue::new(&mut state.seed));
            });
            ui.collapsing("Camera and scale character", |ui| {
                let original = state.reference_setup();
                ui.label("Character uses its real imported size. Place its feet to match the reference; use camera distance/FOV to match apparent scale.");
                ui.horizontal(|ui| {
                    ui.label("Feet X / Z");
                    ui.add(egui::DragValue::new(&mut state.character.xz[0]).speed(0.01).range(-64.0..=64.0));
                    ui.add(egui::DragValue::new(&mut state.character.xz[1]).speed(0.01).range(-64.0..=64.0));
                });
                ui.add(egui::DragValue::new(&mut state.character.yaw).speed(1.0).suffix("° facing"));
                ui.add(egui::DragValue::new(&mut state.camera.pitch).speed(0.1).range(3.0..=89.5).suffix("° pitch"));
                ui.horizontal(|ui| {
                    ui.label("Camera target X / Y / Z");
                    for value in &mut state.camera.target { ui.add(egui::DragValue::new(value).speed(0.01).range(-20.0..=20.0)); }
                });
                if state.reference_setup() != original { state.camera_label = "Custom setup".into(); }
                if ui.button("Save setup for this reference").clicked() {
                    let setup = state.reference_setup();
                    match state.refs.save_setup(&setup) {
                        Ok(()) => {
                            state.message = "Reference setup saved locally".into();
                            state.camera_label = format!("{} · saved setup", state.refs.title());
                        },
                        Err(error) => state.refs.error = Some(error),
                    }
                }
            });
            if let Some(edge) = &mut state.edge {
                let original = *edge;
                egui::CollapsingHeader::new("Grass boundary").default_open(true).show(ui, |ui| {
                    ui.label("Roots stop at the boundary; complete blades can overhang the clear ground.");
                    ui.add(egui::DragValue::new(&mut edge.offset).speed(0.02).range(-64.0..=64.0).suffix(" m position"));
                    ui.add(egui::DragValue::new(&mut edge.angle).speed(0.5).range(-180.0..=180.0).suffix("° angle"));
                    ui.add(egui::DragValue::new(&mut edge.softness).speed(0.01).range(0.05..=2.0).suffix(" m transition"));
                });
                if *edge != original { state.camera_label = "Custom setup".into(); }
            }
            draw_vegetation_authoring(
                ui,
                &mut tools,
                &mut authoring,
                diagnostics.snapshot(),
                &mut settings,
                &mut lighting,
                &mut save,
                &mut project,
                false,
            );
            ui.collapsing("Study wind field", |ui| {
                ui.add(egui::Slider::new(&mut state.wind.strength, 0.0..=2.0).text("Strength"));
                ui.add(egui::Slider::new(&mut state.wind.gustiness, 0.0..=1.0).text("Gustiness"));
                ui.add(egui::Slider::new(&mut state.wind.flutter, 0.0..=1.0).text("Flutter"));
            });
        });
    state.show_inspector = inspector_open;
    canopy_window(&context, &mut state, &mut lighting, &mut settings);
}

fn canopy_window(
    context: &egui::Context,
    state: &mut StudyState,
    lighting: &mut VegetationLighting,
    settings: &mut VegetationDebugSettings,
) {
    let mut open = state.show_canopy;
    egui::Window::new("Canopy · ground and grass").id(egui::Id::new("canopy-look"))
        .open(&mut open).default_width(335.0).default_pos(egui::pos2(1040.0, 150.0))
        .vscroll(true).show(context, |ui| {
            ui.label("Shared shade under the grass, fading upward. An artistic approximation, not real shadows.");
            if state.ground != GroundMode::CanopyGroundStudy {
                if ui.button("Use canopy ground").clicked() { state.ground = GroundMode::CanopyGroundStudy; }
            }
            crate::canopy::draw_controls(ui, &mut lighting.canopy, &mut state.canopy_message);
            crate::canopy::density_controls(ui, settings);
            ui.label("Save study also keeps these controls with the camera and catalog. Colors are in Colors…");
            if let Some(message) = &state.canopy_message { ui.label(message); }
        });
    state.show_canopy = open;
}
