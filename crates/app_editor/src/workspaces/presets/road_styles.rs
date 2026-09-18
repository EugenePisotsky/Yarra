//! Shared cart-road style drafts. Applying is a normal road command; saving stays atomic with
//! ground presets, geometry and painted coverage.
use super::*;
use crate::{
    editing::EditorHistory,
    road_authoring::{
        commands::default_profile,
        working::{RoadChange, RoadWorkingSet, same},
    },
};
use environment::{PresetKind, roads::*};
use world_db::{RoadRecordKey, RoadSourceRecord};
mod limits;
mod relief;
pub(super) use limits::bounded_metres;
use limits::{GeometryField, GeometryLimits};

#[derive(Clone)]
pub(super) struct StyleDraft {
    base: Option<CartTrackProfile>,
    pub value: CartTrackProfile,
}
impl StyleDraft {
    pub fn dirty(&self) -> bool {
        !same(
            self.base.clone().map(RoadSourceRecord::Profile).as_ref(),
            Some(&RoadSourceRecord::Profile(self.value.clone())),
        )
    }
    pub fn apply(
        &mut self,
        dense: &mut DenseDomainWorkingSets,
        history: &mut EditorHistory,
    ) -> Result<(), String> {
        let key = RoadRecordKey::Profile(self.value.id);
        if !same(
            dense.roads.get(key),
            self.base.clone().map(RoadSourceRecord::Profile).as_ref(),
        ) {
            return Err("This shared style changed while its draft was open. Discard the draft to reload it.".into());
        }
        let before = dense.roads.changes([key]);
        dense.roads.apply(&[RoadChange {
            key,
            record: Some(RoadSourceRecord::Profile(self.value.clone())),
        }])?;
        history.record_roads(before, dense.roads.changes([key]));
        self.base = Some(self.value.clone());
        Ok(())
    }
}
#[derive(Default)]
pub(super) struct Styles {
    pub selected: Option<RoadProfileId>,
    pub draft: Option<StyleDraft>,
    requested: Option<RoadProfileId>,
    pub curved: bool,
    pub width: f32,
    pub underlay: Option<PresetId>,
    pub reference_initialized: bool,
}
impl Styles {
    pub fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(StyleDraft::dirty)
    }
    pub fn request(&mut self, id: Option<RoadProfileId>) {
        self.requested = id;
    }
    pub fn refresh(&mut self, roads: &RoadWorkingSet) -> bool {
        if self.width == 0.0 {
            self.width = 3.5;
        }
        if self.dirty() {
            return false;
        }
        if let Some(id) = self.requested.take() {
            self.selected = Some(id);
        }
        if self.selected.is_none() {
            self.selected = roads.entries.values().find_map(|e| match &e.current {
                Some(RoadSourceRecord::Profile(p)) => Some(p.id),
                _ => None,
            });
        }
        let value = self
            .selected
            .and_then(|id| match roads.get(RoadRecordKey::Profile(id)) {
                Some(RoadSourceRecord::Profile(p)) => Some(p.clone()),
                _ => None,
            });
        let changed = self.draft.as_ref().map(|d| &d.value) != value.as_ref();
        if changed {
            self.draft = value.map(|p| StyleDraft {
                base: Some(p.clone()),
                value: p,
            });
        }
        changed
    }
    pub fn discard(&mut self) {
        self.draft = None;
    }
}

pub(super) fn validate(
    p: &CartTrackProfile,
    roads: &RoadWorkingSet,
    library: &PresetLibrary,
    definitions: &[environment::EnvironmentDefinition],
) -> Result<(), String> {
    p.validate().map_err(|e| e.to_string())?;
    let Some(PresetKind::Ground(ground)) = library.get(p.ground).map(|p| &p.kind) else {
        return Err("Choose a Ground preset for the road surface.".into());
    };
    if let Some(usages) = roads.style_usage.get(&p.id) {
        for usage in usages {
            if usage.minimum_width.is_some_and(|w| w < p.minimum_width()) {
                return Err(format!(
                    "A saved road in World {} is {:.2} m wide, below this style's {:.2} m minimum. Reduce wheel spacing, track width or edge sizes; otherwise widen and save that road first, or duplicate the style.",
                    usage.space.0,
                    usage.minimum_width.unwrap(),
                    p.minimum_width()
                ));
            }
            let definition = definitions
                .iter()
                .find(|d| d.space == usage.space)
                .ok_or("A dependent world's surfaces are unavailable")?;
            if ground
                .surfaces
                .iter()
                .any(|s| !definition.surfaces.contains(&s.surface))
            {
                return Err(format!(
                    "Ground mixture uses a surface absent from World {}.",
                    usage.space.0
                ));
            }
        }
    }
    for e in roads.entries.values() {
        if let Some(RoadSourceRecord::Road(r)) = &e.current
            && r.road.profile == p.id
            && let Some(d) = definitions.iter().find(|d| d.space == r.space)
            && ground
                .surfaces
                .iter()
                .any(|s| !d.surfaces.contains(&s.surface))
        {
            return Err("Ground mixture is unavailable in a dependent world.".into());
        }
    }
    if limits::minimum_corridor(p, roads).is_some_and(|w| w < p.minimum_width()) {
        return Err(format!(
            "An edited road is too narrow for this style's {:.2} m minimum. Reduce wheel spacing, track width or edge sizes; otherwise widen and save the road first, or duplicate the style.",
            p.minimum_width()
        ));
    }
    Ok(())
}

pub(super) fn validate_in_project(
    p: &CartTrackProfile,
    dense: &DenseDomainWorkingSets,
    library: &PresetLibrary,
    project: &ProjectEditorStore,
    preview_space: Option<WorldSpaceId>,
) -> Result<(), String> {
    let definitions = project
        .environments()
        .iter()
        .map(|d| dense.definition(d.space).unwrap_or(d).clone())
        .collect::<Vec<_>>();
    validate(p, &dense.roads, library, &definitions)?;
    for space in dependent_spaces(p, &dense.roads, preview_space) {
        p.validate_relief_detail(
            project
                .terrain_height_step(space)
                .ok_or("Terrain height grid is loading")?,
        )
        .map_err(|e| format!("World {}: {e}", space.0))?;
        let d = definitions
            .iter()
            .find(|d| d.space == space)
            .ok_or("World definition is loading")?;
        let terrain = project
            .terrain_resources(space)
            .ok_or("Terrain resources for a dependent world are loading")?;
        environment_compile::RoadCompileProfile::default()
            .validate_detail(p, d.cell_size, terrain.profile.weight_resolution, 64)
            .map_err(|e| format!("World {}: {e}", space.0))?;
    }
    Ok(())
}

fn dependent_spaces(
    p: &CartTrackProfile,
    roads: &RoadWorkingSet,
    preview_space: Option<WorldSpaceId>,
) -> std::collections::BTreeSet<WorldSpaceId> {
    let mut spaces = roads
        .style_usage
        .get(&p.id)
        .into_iter()
        .flatten()
        .map(|u| u.space)
        .chain(preview_space)
        .collect::<std::collections::BTreeSet<_>>();
    for e in roads.entries.values() {
        if let Some(RoadSourceRecord::Road(r)) = &e.current
            && r.road.profile == p.id
        {
            spaces.insert(r.space);
        }
    }
    spaces
}

pub(super) fn panels(
    root: &mut egui::Ui,
    state: &mut PresetAuthoringState,
    dense: &DenseDomainWorkingSets,
    library: &PresetLibrary,
    project: &ProjectEditorStore,
    idle: bool,
    ready: bool,
) {
    let styles = dense
        .roads
        .entries
        .values()
        .filter_map(|e| match &e.current {
            Some(RoadSourceRecord::Profile(p)) => Some(p.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let before = state.styles.draft.as_ref().map(|d| d.value.clone());
    egui::Panel::left("road_style_library")
        .default_size(230.0)
        .resizable(true)
        .show(root, |ui| {
            ui.heading("Road styles");
            ui.small("Shared across every road using the style.");
            if !ready {
                ui.weak("Loading styles and road dependencies…");
            }
            ui.add_enabled_ui(
                idle && ready && !state.styles.dirty() && styles.len() < MAX_ROAD_RECORDS,
                |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("New style").clicked() {
                            if let Some(g) = library
                                .presets
                                .iter()
                                .find(|p| matches!(p.kind, PresetKind::Ground(_)))
                            {
                                let channel = library
                                    .presets
                                    .iter()
                                    .find_map(|p| match &p.kind {
                                        PresetKind::Foliage(f) => Some(f.channel),
                                        _ => None,
                                    })
                                    .unwrap_or(environment::ChannelId([0; 16]));
                                let p = default_profile(g.id, channel);
                                state.styles.selected = Some(p.id);
                                state.styles.draft = Some(StyleDraft {
                                    base: None,
                                    value: p,
                                });
                            } else {
                                state.message =
                                    Some("Create and apply a Ground preset first.".into());
                            }
                        }
                        if ui
                            .add_enabled(
                                state.styles.draft.is_some(),
                                egui::Button::new("Duplicate"),
                            )
                            .clicked()
                            && let Some(d) = &state.styles.draft
                        {
                            let mut p = d.value.clone();
                            p.id = RoadProfileId(*uuid::Uuid::new_v4().as_bytes());
                            p.revision = 1;
                            p.name = format!("{} copy", p.name);
                            state.styles.selected = Some(p.id);
                            state.styles.draft = Some(StyleDraft {
                                base: None,
                                value: p,
                            });
                        }
                    });
                },
            );
            ui.add(egui::TextEdit::singleline(&mut state.search).hint_text("Find styles"));
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!state.styles.dirty(), |ui| {
                    for p in &styles {
                        if p.name.to_lowercase().contains(&state.search.to_lowercase())
                            && ui
                                .selectable_label(state.styles.selected == Some(p.id), &p.name)
                                .clicked()
                        {
                            state.styles.selected = Some(p.id);
                        }
                    }
                });
            });
            if state.styles.dirty() {
                ui.small("Apply or discard this draft before choosing another style.");
            }
        });
    let mut edit_ground = None;
    egui::Panel::right("road_style_settings").default_size(340.0).resizable(true).show(root, |ui| {
        ui.heading("Shared road style");
        let Some(draft) = &mut state.styles.draft else { ui.weak("Choose or create a style."); return; };
        let p = &mut draft.value;
        let limits = GeometryLimits::in_project(p, dense, project, state.space);
        let count: u64 = dense.roads.style_usage.get(&p.id).into_iter().flatten().map(|u| u.road_count).sum();
        ui.small(format!("Used by {count} saved roads · includes distant roads"));
        egui::ScrollArea::vertical().id_salt("style_settings_scroll").show(ui, |ui| {
            if let Err(e) = &limits { ui.weak(e); }
            ui.add_enabled_ui(idle && ready && limits.is_ok(), |ui| {
                ui.text_edit_singleline(&mut p.name);
                ui.label("Ground mixture");
                egui::ComboBox::from_id_salt("style_ground").selected_text(library.get(p.ground).map_or("Missing Ground preset", |p| p.name.as_str())).show_ui(ui, |ui| {
                    for g in &library.presets { if matches!(g.kind, PresetKind::Ground(_)) { ui.selectable_value(&mut p.ground, g.id, &g.name); } }
                });
                if ui.button("Edit ground mixture…").clicked() { edit_ground = Some(p.ground); }
                ui.small("The linked Ground preset owns material weights and strength. Editing it affects all its uses.");
                ui.label("Foliage channel to thin");
                egui::ComboBox::from_id_salt("style_channel").selected_text(library.presets.iter().find(|g| matches!(&g.kind, PresetKind::Foliage(f) if f.channel == p.vegetation_channel)).map_or("No matching foliage preset", |p| p.name.as_str())).show_ui(ui, |ui| {
                    for g in &library.presets { if let PresetKind::Foliage(f) = &g.kind { ui.selectable_value(&mut p.vegetation_channel, f.channel, &g.name); } }
                });
                ui.separator(); ui.label("Wheel tracks");
                if let Ok(limits) = &limits {
                    GeometryField::Spacing.control(ui, p, *limits);
                    GeometryField::TrackWidth.control(ui, p, *limits);
                }
                ratio(ui, "Grass retained in tracks", &mut p.track_retention);
                ui.separator(); ui.label("Center strip");
                ratio(ui, "Ground exposure at center", &mut p.center_ground);
                ratio(ui, "Grass retained at center", &mut p.center_retention);
                ui.separator(); ui.label("Shoulders and edges");
                ratio(ui, "Ground exposure at shoulders", &mut p.shoulder_ground);
                ratio(ui, "Grass retained at shoulders", &mut p.shoulder_retention);
                if let Ok(limits) = &limits {
                    GeometryField::Softness.control(ui, p, *limits);
                    GeometryField::Variation.control(ui, p, *limits);
                    GeometryField::EdgePatch.control(ui, p, *limits);
                    if let Some(range) = limits.range(GeometryField::Softness, p) {
                        ui.small(format!("Softness range: {:.4}–{:.4} m. It cannot exceed track width; the road corridor must also fit both edges.", range.start(), range.end()));
                    }
                }
                ui.separator(); ui.label("Wear breakup");
                ratio(ui, "Unworn patches", &mut p.breakup);
                if let Ok(limits) = &limits { GeometryField::BreakupPatch.control(ui, p, *limits); }
                ui.small("Unworn patches preserve the surrounding ground and grass. Retention never adds grass where none was painted.");
                if let Ok(limits) = &limits { relief::controls(ui, p, limits.height_step); }
                ui.label(format!("Minimum road width: {:.2} m", p.minimum_width()));
                if let Ok(limits) = &limits && let Some(width) = limits.corridor {
                    ui.small(format!("Narrowest road using this style: {width:.2} m. For a wider style, widen and save its roads first, or duplicate the style."));
                }
            });
        });
    });
    if before != state.styles.draft.as_ref().map(|d| d.value.clone()) {
        state.bump();
    }
    if let Some(id) = edit_ground {
        state.mode = Mode::Environment;
        state.select(id);
    }
}
fn ratio(ui: &mut egui::Ui, label: &str, v: &mut f32) {
    ui.add(egui::Slider::new(v, 0.0..=1.0).text(label));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing::EditorObjectWorkingSet;
    use std::collections::BTreeSet;
    use world_db::*;
    #[test]
    fn style_draft_isolated_then_undoable_across_source_save_and_stale_edits_rejected() {
        let path =
            std::env::temp_dir().join(format!("yarra-style-{}.sqlite", uuid::Uuid::new_v4()));
        world_cook::create_road_demo_project(&path).unwrap();
        let reader = ProjectReader::open_read_only(&path).unwrap();
        let snapshot = reader
            .read_road_authoring_snapshot(
                WorldSpaceId(1),
                RoadCellBounds {
                    minimum: world::CellCoord::ZERO,
                    maximum: world::CellCoord::ZERO,
                },
                &[],
            )
            .unwrap();
        let mut dense = DenseDomainWorkingSets::default();
        dense.roads.reconcile(snapshot, &BTreeSet::new());
        let mut styles = Styles::default();
        styles.refresh(&dense.roads);
        let d = styles.draft.as_mut().unwrap();
        let original = d.value.clone();
        d.value.breakup = 0.2;
        assert_eq!(dense.roads.dirty_count(), 0);
        let mut stale = d.clone();
        stale.value.breakup = 0.4;
        let mut history = EditorHistory::default();
        d.apply(&mut dense, &mut history).unwrap();
        assert_eq!(dense.roads.dirty_count(), 1);
        assert!(stale.apply(&mut dense, &mut history).is_err());
        let mut writer = ProjectWriter::open(&path).unwrap();
        let outcome = writer
            .apply_environment_and_roads_transaction(
                reader.read_environment_presets().unwrap().revision,
                None,
                &[],
                &[],
                &dense.roads.writes(),
                &dense.roads.dependencies(),
            )
            .unwrap();
        let EnvironmentSourceWriteResult::Committed(commit) = outcome else {
            panic!()
        };
        dense.roads.saving = Some(1);
        dense.roads.finish_save(
            1,
            &crate::project_store::DenseSaveOutcome::Committed(commit),
        );
        styles.refresh(&dense.roads);
        assert!(!styles.dirty());
        let mut objects = EditorObjectWorkingSet::default();
        assert!(history.undo(&mut objects, &mut dense));
        let Some(RoadSourceRecord::Profile(p)) =
            dense.roads.get(RoadRecordKey::Profile(original.id))
        else {
            panic!()
        };
        assert_eq!(p.breakup, original.breakup);
        assert!(p.revision > original.revision);
        assert!(history.redo(&mut objects, &mut dense));
        styles.refresh(&dense.roads);
        assert_eq!(styles.draft.unwrap().value.breakup, 0.2);
        drop(writer);
        drop(reader);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn style_validation_includes_offscreen_widths_and_world_surfaces() {
        let p = default_profile(
            environment::fixtures::DRY_GROUND,
            environment::ChannelId([1; 16]),
        );
        let dry = world::TerrainSurfaceId([1; 16]);
        let green = world::TerrainSurfaceId([2; 16]);
        let library = environment::fixtures::meadow_library(dry, green, p.vegetation_channel);
        let d = environment::EnvironmentDefinition {
            space: WorldSpaceId(7),
            revision: 1,
            cell_size: 8.0,
            mask_resolution: 33,
            surfaces: vec![dry, green],
            base_surface: green,
            layers: vec![],
        };
        let mut roads = RoadWorkingSet::default();
        roads.style_usage.insert(
            p.id,
            vec![RoadStyleUsage {
                space: d.space,
                road_count: 100,
                minimum_width: Some(3.5),
            }],
        );
        assert!(validate(&p, &roads, &library, std::slice::from_ref(&d)).is_ok());
        assert_eq!(limits::minimum_corridor(&p, &roads), Some(3.5));
        let mut wide = p.clone();
        wide.track_spacing = 4.0;
        assert!(
            validate(&wide, &roads, &library, std::slice::from_ref(&d))
                .unwrap_err()
                .contains("saved road")
        );
        let mut d = d;
        d.surfaces = vec![green];
        assert!(
            validate(&p, &roads, &library, &[d])
                .unwrap_err()
                .contains("absent")
        );
    }

    #[test]
    fn reassigned_style_keeps_retained_route_width_limits_without_loading_its_knots() {
        let r = super::super::tests::road_request();
        let p = r.road.unwrap().profile;
        let road = Road {
            id: RoadId([7; 16]),
            revision: 1,
            name: "Reassigned route".into(),
            profile: p.id,
            seed: 1,
            enabled: true,
            order: 0,
            direction: TravelDirection::Bidirectional,
            travel_modes: vec![RoadTravelMode::Foot],
        };
        let mut roads = RoadWorkingSet::default();
        roads
            .apply(&[
                RoadChange {
                    key: RoadRecordKey::Profile(p.id),
                    record: Some(RoadSourceRecord::Profile(p.clone())),
                },
                RoadChange {
                    key: RoadRecordKey::Road(road.id),
                    record: Some(RoadSourceRecord::Road(SourceRoad {
                        space: WorldSpaceId(1),
                        road: road.clone(),
                    })),
                },
            ])
            .unwrap();
        roads.route_minimum_widths.insert(road.id, Some(3.0));
        assert!(roads.style_usage.is_empty());
        assert_eq!(limits::minimum_corridor(&p, &roads), Some(3.0));
        let mut wider = p;
        wider.edge_softness = 0.4;
        assert!(
            validate(&wider, &roads, &r.library, &[])
                .unwrap_err()
                .contains("edited road")
        );
    }
}
