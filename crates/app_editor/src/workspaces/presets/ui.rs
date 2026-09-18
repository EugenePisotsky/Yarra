use super::*;
use crate::{
    editing::{EditorHistory, EditorObjectWorkingSet, merge_preset_changes},
    environment_paint::{EnvironmentPaintState, preset_controls as controls},
    publication::RuntimePublicationState,
    saving::EditorSaveCoordinator,
    shell::EditorUiFrame,
    vegetation_authoring::VegetationAuthoringState,
};
use bevy::ecs::system::SystemParam;
use environment::{Preset, PresetKind};

#[derive(SystemParam)]
pub(super) struct Resources<'w> {
    state: ResMut<'w, PresetAuthoringState>,
    preview: Res<'w, viewport::PreviewState>,
    roads: Res<'w, crate::road_authoring::RoadToolState>,
    dense: ResMut<'w, DenseDomainWorkingSets>,
    project: Res<'w, ProjectEditorStore>,
    plants: Res<'w, VegetationAuthoringState>,
    history: ResMut<'w, EditorHistory>,
    objects: ResMut<'w, EditorObjectWorkingSet>,
    save: ResMut<'w, EditorSaveCoordinator>,
    publication: Res<'w, RuntimePublicationState>,
    paint: Res<'w, EnvironmentPaintState>,
    next: ResMut<'w, NextState<EditorWorkspace>>,
}
pub(super) fn draw(mut frame: ResMut<EditorUiFrame>, resources: Resources) -> Result {
    let Resources {
        mut state,
        preview,
        roads,
        mut dense,
        project,
        plants,
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
    let idle = !save.active()
        && !project.save_in_flight()
        && !dense.saving()
        && !dense.gesture_active
        && !dense.has_any_conflict()
        && !objects.saving()
        && !objects.has_any_conflict()
        && !plants.saving()
        && !plants.has_conflict()
        && !publication.active()
        && project.write_error().is_none();
    let road_mode = state.mode == Mode::Roads;
    let active_dirty = if road_mode {
        state.styles.dirty()
    } else {
        draft.dirty()
    };
    let all_dirty = draft.dirty() || state.styles.dirty();
    let mut apply = false;
    let mut discard = false;
    egui::Panel::top("preset_toolbar").show(root,|ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Back to World").clicked(){next.set(EditorWorkspace::World);}
            let old_mode=state.mode;
            ui.selectable_value(&mut state.mode,Mode::Environment,"Environment");
            ui.selectable_value(&mut state.mode,Mode::Roads,"Road styles");
            if state.mode!=old_mode { state.bump(); }
            apply=ui.add_enabled(idle && active_dirty && (!road_mode || (roads.ready && preview.matches(&state) && !draft.dirty())),egui::Button::new(if road_mode {"Apply road style"} else {"Apply preset changes"})).clicked();
            discard=ui.add_enabled(active_dirty,egui::Button::new("Discard draft")).clicked();
            let committed=idle && !all_dirty && !paint.has_unapplied_changes();
            if ui.add_enabled(committed && history.undo_len()>0,egui::Button::new("Undo")).clicked(){history.undo(&mut objects,&mut dense);}
            if ui.add_enabled(committed && history.redo_len()>0,egui::Button::new("Redo")).clicked(){history.redo(&mut objects,&mut dense);}
            let dirty=dense.dirty_count()+objects.dirty_count()+plants.dirty_count()>0;
            if ui.add_enabled(committed && dirty,egui::Button::new("Save")).clicked(){save.request_save();}
            if ui.add_enabled(committed,egui::Button::new(if dirty {"Save & Publish"} else {"Publish"})).clicked(){save.request_publish();}
        });
        if all_dirty{ui.colored_label(egui::Color32::YELLOW,"Previewing an unapplied draft. Apply updates every use of these shared presets. Drafts stay here when you change workspaces; apply both Environment and Road styles drafts before saving.");}
        if let Some(message)=&state.message {ui.label(message);}
        ui.weak(save.status(dense.dirty_count()+objects.dirty_count()+plants.dirty_count()));
        ui.weak(publication.status());
        if let Some(error)=project.write_error() {ui.colored_label(egui::Color32::LIGHT_RED,error);}
        if let Some(status)=dense.status() {ui.colored_label(egui::Color32::YELLOW,status);}
        if paint.has_unapplied_changes() {ui.colored_label(egui::Color32::YELLOW,"Apply or discard the layer settings in World before saving.");}
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
        egui::Panel::left("preset_library")
            .default_size(230.0)
            .resizable(true)
            .show(root, |ui| {
                ui.heading("Preset library");
                ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Find presets"));
                let mut added = None;
                ui.add_enabled_ui(
                    idle && draft.library.presets.len() < environment::MAX_PRESETS,
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.menu_button("New", |ui| {
                                if let Some(base) = state.base
                                    && ui.button("Ground").clicked()
                                {
                                    added = Some(controls::new_ground(base));
                                    ui.close();
                                }
                                if let Some((catalog, _, _)) = plants.study_source()
                                    && let Some(a) = catalog.assemblages.first()
                                    && ui.button("Foliage").clicked()
                                {
                                    added = Some(controls::new_foliage(a.id));
                                    ui.close();
                                }
                                if ui.button("Exclusion").clicked() {
                                    added = Some(controls::new_exclusion());
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
                                .add_enabled(
                                    state.selected.is_some(),
                                    egui::Button::new("Duplicate"),
                                )
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
                        })
                    },
                );
                if let Some(p) = added {
                    state.select(p.id);
                    draft.library.presets.push(p);
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
                            if ui
                                .selectable_label(
                                    state.selected == Some(preset.id),
                                    format!(
                                        "{}\n{}",
                                        preset.name,
                                        controls::kind_name(&preset.kind)
                                    ),
                                )
                                .clicked()
                            {
                                state.select(preset.id);
                            }
                        }
                    });
            });
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
                if let (Some(mut selected), Some(definition)) = (
                    state.selected,
                    state.space.and_then(|space| dense.definition(space)),
                ) {
                    let mut usages = vec![];
                    for world in project.environments() {
                        let world = dense.definition(world.space).unwrap_or(world);
                        for layer in &world.layers {
                            if draft
                                .library
                                .resolve(layer.preset, &[])
                                .is_ok_and(|resolved| resolved.dependencies.contains_key(&selected))
                                || layer.preset == selected
                            {
                                usages.push(format!("World {} · {}", world.space.0, layer.name));
                            }
                        }
                    }
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
                                controls::preset_editor(
                                    ui,
                                    &mut draft.library,
                                    &mut selected,
                                    definition,
                                    &project,
                                    plants.study_source().map(|(c, _, _)| c),
                                );
                            })
                        });
                    if Some(selected) != state.selected {
                        state.select(selected);
                    }
                } else {
                    ui.weak("Choose or create a preset.");
                }
            });
    }
    if (state.mode == Mode::Roads) != road_mode {
        state.draft = Some(draft);
        return Ok(());
    }
    let mut preview_underlay = state.preview_underlay();
    egui::CentralPanel::default().show(root,|ui| {
        ui.heading(if road_mode {"Road style preview"} else {"Preset preview"});
        if road_mode {
            let before=(state.styles.curved,state.styles.width.to_bits());
            let minimum_width = state.styles.draft.as_ref().map_or(0.1, |d| d.value.minimum_width());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut state.styles.curved,"Curved road");
                ui.label("Preview corridor width");
                road_styles::bounded_metres(ui, &mut state.styles.width, minimum_width..=64.0, 0.05);
                if state.styles.width < minimum_width && ui.button("Fit preview width").clicked() {
                    state.styles.width = minimum_width;
                }
            });
            if before!=(state.styles.curved,state.styles.width.to_bits()) {state.bump();}
            ui.small("Width and reference foliage are preview-only. Map roads keep their own widths and painted coverage.");
            if state.styles.width < minimum_width {
                ui.colored_label(egui::Color32::YELLOW,format!("Preview corridor is {:.2} m wide; this style needs at least {minimum_width:.2} m. Use Fit preview width.",state.styles.width));
            }
            if let Some(d)=&state.styles.draft {
                if let Err(e)=road_styles::validate_in_project(&d.value,&dense,&draft.library,&project,state.space) {ui.colored_label(egui::Color32::YELLOW,e);}
                if let Some(p)=preview_underlay.and_then(|id|draft.library.get(id)) && matches!(&p.kind,PresetKind::Foliage(f) if f.channel!=d.value.vegetation_channel) {ui.colored_label(egui::Color32::YELLOW,"Reference foliage uses another channel; this style will not thin it.");}
            }
        }
        ui.small("Flat test patch · fixed seed · no map overrides. Drag to orbit; scroll to zoom.");
        ui.horizontal_wrapped(|ui| {
            let previous_size=state.size;
            egui::ComboBox::from_id_salt("patch_size").selected_text(format!("{} m",state.size)).show_ui(ui,|ui|{ui.selectable_value(&mut state.size,8,"8 m");ui.selectable_value(&mut state.size,16,"16 m");});
            if state.size!=previous_size {state.distance*=f32::from(state.size)/f32::from(previous_size);}
            egui::ComboBox::from_id_salt("patch_shape").selected_text(state.footprint.label()).show_ui(ui,|ui|{for f in [Footprint::Full,Footprint::Patch,Footprint::Hole]{ui.selectable_value(&mut state.footprint,f,f.label());}});
            if ui.button(if state.playing {"Pause wind"} else {"Play wind"}).clicked(){state.playing= !state.playing;}
            if ui.button("Reset view").clicked(){state.yaw=35.0;state.pitch=40.0;state.distance=f32::from(state.size)*1.375;state.phase=0.0;}
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Base ground");
            if let Some(space)=state.space && let Some(resources)=project.terrain_resources(space) {
                let name=resources.surfaces.iter().find(|s|Some(s.surface.id)==state.base).map(|s|s.surface.display_name.as_str()).unwrap_or("Choose");
                egui::ComboBox::from_id_salt("preset_base").selected_text(name).show_ui(ui,|ui|{for surface in &resources.surfaces {ui.selectable_value(&mut state.base,Some(surface.surface.id),&surface.surface.display_name);}});
            }
            ui.label("Reference foliage");
            egui::ComboBox::from_id_salt("preset_underlay").selected_text(preview_underlay.and_then(|id|draft.library.get(id)).map_or("None",|p|p.name.as_str())).show_ui(ui,|ui| {
                ui.selectable_value(&mut preview_underlay,None,"None");
                for p in &draft.library.presets {if matches!(p.kind,PresetKind::Foliage(_)){ui.selectable_value(&mut preview_underlay,Some(p.id),&p.name);}}
            });
        });
        if preview_underlay.is_some(){ui.small(if road_mode {"Coverage shape applies to the reference foliage. Roads only thin the grass already present."} else {"Reference foliage fills the patch underneath this preset, to test clearing and blending."});}
        if let Some(error)=&preview.error{ui.colored_label(egui::Color32::LIGHT_RED,format!("Preview unavailable: {error}"));}
        else if preview.pending(){ui.weak("Compiling preview…");}
        else {ui.weak(format!("Preview current · up to {} foliage candidates",preview.candidates));}
        let available=ui.available_size();let width=available.x.min((available.y-8.0).max(1.0)*4.0/3.0).max(1.0);
        let response=ui.add(egui::Image::new((state.texture,egui::vec2(width,width*0.75))).sense(egui::Sense::drag()).tint(if preview.current{egui::Color32::WHITE}else{egui::Color32::from_gray(100)}));
        if !preview.current {ui.painter().text(response.rect.center(),egui::Align2::CENTER_CENTER,"Preview is not current",egui::FontId::proportional(18.0),egui::Color32::WHITE);}
        if response.dragged(){let delta=ui.input(|i|i.pointer.delta());state.yaw-=delta.x*0.4;state.pitch=(state.pitch+delta.y*0.3).clamp(5.0,89.0);}
        if response.hovered(){let scroll=ui.input(|i|i.smooth_scroll_delta.y);state.distance=(state.distance*(-scroll*0.002).exp()).clamp(2.0,40.0);}
    });
    if road_mode {
        state.styles.underlay = preview_underlay;
    } else {
        state.underlay = preview_underlay;
    }
    if road_mode {
        if apply && preview.matches(&state) {
            let space = state.space;
            if let Some(d) = &mut state.styles.draft {
                let result = road_styles::validate_in_project(
                    &d.value,
                    &dense,
                    &draft.library,
                    &project,
                    space,
                )
                .and_then(|_| d.apply(&mut dense, &mut history));
                state.message=Some(match result {Ok(())=>"Road style applied to all its uses. Save persists it; Save & Publish updates the game.".into(), Err(e)=>e});
            }
        } else if discard {
            state.styles.discard();
            state.styles.refresh(&dense.roads);
            state.bump();
            state.message = None;
        }
    } else if apply {
        match merge_preset_changes(dense.presets(), &draft.base, &draft.library)
            .and_then(|library| history.edit_presets(&mut dense, &library))
        {
            Ok(()) => {
                let library = dense.presets().unwrap().clone();
                draft = Draft {
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
    } else if (discard || (!draft.dirty() && dense.presets().is_some_and(|p| p != &draft.base)))
        && let Some(library) = dense.presets()
    {
        draft = Draft {
            base: library.clone(),
            library: library.clone(),
        };
        state.message = None;
    }
    if before != draft.library {
        state.bump();
    }
    state.draft = Some(draft);
    Ok(())
}
