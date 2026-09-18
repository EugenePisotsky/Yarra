//! World-space composition painting. Each drag is one bounded, reversible source command.
mod coverage;
mod inspector;
mod layer_browser;
pub(crate) use layer_browser::EnvironmentLayerBrowser;
pub(crate) mod preset_controls;
pub(crate) mod preview;
pub(crate) use inspector::inspector;
pub(crate) use preview::EnvironmentPreview;

use crate::{
    domain_editing::{DenseDomainWorkingSets, reconcile_dense_working_sets},
    editing::{EditorHistory, EditorObjectWorkingSet},
    preview::{EditorPreviewMode, PreviewModeState},
    project_store::{ProjectEditorStore, ProjectStoreUpdate},
    publication::RuntimePublicationState,
    saving::{EditorSaveCoordinator, drive_editor_save},
    shell::EditorInputCapture,
    tools::{ENVIRONMENT_TOOL, EditorToolRegistry},
    vegetation_authoring::{VegetationAuthoringState, VegetationPreviewSync},
    workspaces::{
        EditorWorkspace,
        world_impl::{EditorOverlayGizmos, handle_editor_shortcuts, update_editor_camera},
    },
};
use bevy::{
    ecs::system::SystemParam,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_egui::egui;
use engine::{StreamedTerrainSurface, WorldOrigin, WorldViewCamera};
use environment::{
    EnvironmentDefinition, LayerId,
    brush::{BrushOperation, CoverageBrush},
};
use std::collections::BTreeMap;
use world::{CellCoord, WorldSpaceId};
use world_db::SourceEnvironmentCellRecord;

pub(crate) struct EnvironmentPaintPlugin;
impl Plugin for EnvironmentPaintPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EnvironmentPaintState>()
            .init_resource::<EnvironmentLayerBrowser>()
            .add_systems(Update, layer_browser::update.after(ProjectStoreUpdate))
            .init_resource::<EnvironmentPreview>()
            .init_resource::<preview::PreviewMaterials>()
            .init_resource::<coverage::CoverageOverlays>()
            .add_plugins(MaterialPlugin::<coverage::CoverageMaterial>::default())
            .add_systems(
                Update,
                (
                    preview::receive_preview,
                    paint_input,
                    preview::queue_preview,
                    preview::apply_ground_preview,
                    coverage::update_coverage,
                )
                    .chain()
                    .after(ProjectStoreUpdate)
                    .after(reconcile_dense_working_sets)
                    .after(update_editor_camera)
                    .before(handle_editor_shortcuts)
                    .before(drive_editor_save)
                    .before(VegetationPreviewSync)
                    .before(terrain_render::TerrainMaterialPreparation),
            )
            .add_systems(PostUpdate, draw_brush);
    }
}

struct Stroke {
    space: WorldSpaceId,
    layer: LayerId,
    brush: CoverageBrush,
    before: BTreeMap<CellCoord, SourceEnvironmentCellRecord>,
    last_point: Option<[f64; 2]>,
}
#[derive(Clone, Copy)]
struct BrushHit {
    point: Vec3,
    cell: CellCoord,
}
#[derive(Resource)]
pub(crate) struct EnvironmentPaintState {
    definition_form: Option<inspector::DefinitionForm>,
    pub(crate) preset_request: Option<(WorldSpaceId, Option<environment::PresetId>)>,
    pub(crate) show_coverage: bool,
    pub(crate) coverage_visible: bool,
    space: Option<WorldSpaceId>,
    selected: Option<LayerId>,
    brush: CoverageBrush,
    stroke: Option<Stroke>,
    hover: Option<BrushHit>,
    status: Option<String>,
    ready: bool,
}
impl Default for EnvironmentPaintState {
    fn default() -> Self {
        Self {
            definition_form: None,
            preset_request: None,
            show_coverage: false,
            coverage_visible: false,
            space: None,
            selected: None,
            brush: CoverageBrush {
                radius: 4.0,
                falloff: 0.6,
                strength: 0.75,
                operation: BrushOperation::Paint,
            },
            stroke: None,
            hover: None,
            status: None,
            ready: false,
        }
    }
}
impl EnvironmentPaintState {
    fn finish(
        &mut self,
        dense: &mut DenseDomainWorkingSets,
        history: &mut EditorHistory,
        cancel: bool,
    ) {
        if let Some(stroke) = self.stroke.take() {
            dense.gesture_active = false;
            let before = stroke.before.into_values().collect::<Vec<_>>();
            if cancel {
                if let Err(error) = dense.apply_environment_records(&before) {
                    self.status = Some(error);
                }
            } else {
                let after = before
                    .iter()
                    .filter_map(|record| {
                        dense.environment_record(record.space, record.cell).cloned()
                    })
                    .collect();
                history.record_environment_stroke(before, after);
            }
        }
    }
}

#[derive(SystemParam)]
struct PaintInput<'w, 's> {
    window: Single<'w, 's, &'static Window, With<PrimaryWindow>>,
    camera: Single<'w, 's, (&'static Camera, &'static GlobalTransform), With<WorldViewCamera>>,
    terrain: Query<'w, 's, (Entity, &'static StreamedTerrainSurface)>,
    origin: Res<'w, WorldOrigin>,
    workspace: Res<'w, State<EditorWorkspace>>,
    mode: Res<'w, PreviewModeState>,
    tools: Res<'w, EditorToolRegistry>,
    capture: Res<'w, EditorInputCapture>,
    buttons: Res<'w, ButtonInput<MouseButton>>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    save: Res<'w, EditorSaveCoordinator>,
    publication: Res<'w, RuntimePublicationState>,
    objects: Res<'w, EditorObjectWorkingSet>,
    vegetation: Res<'w, VegetationAuthoringState>,
}
fn paint_input(
    input: PaintInput,
    mut raycast: MeshRayCast,
    mut project: ResMut<ProjectEditorStore>,
    mut dense: ResMut<DenseDomainWorkingSets>,
    mut history: ResMut<EditorHistory>,
    mut paint: ResMut<EnvironmentPaintState>,
) {
    let enabled = *input.workspace.get() == EditorWorkspace::World
        && input.mode.active() == Some(EditorPreviewMode::Authoring)
        && input
            .tools
            .active(EditorWorkspace::World)
            .is_some_and(|tool| tool.id == ENVIRONMENT_TOOL.id)
        && input.window.focused;
    let blocked = paint.has_unapplied_changes()
        || input.capture.wants_pointer
        || input.buttons.pressed(MouseButton::Right)
        || input.buttons.pressed(MouseButton::Middle)
        || [
            KeyCode::AltLeft,
            KeyCode::AltRight,
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            KeyCode::SuperLeft,
            KeyCode::SuperRight,
        ]
        .iter()
        .any(|key| input.keys.pressed(*key))
        || input.save.active()
        || input.publication.active()
        || project.save_in_flight()
        || dense.saving()
        || dense.has_any_conflict()
        || input.objects.saving()
        || input.vegetation.saving();
    if input.keys.just_pressed(KeyCode::Escape) && paint.stroke.is_some() {
        paint.finish(&mut dense, &mut history, true);
        paint.status = Some("Stroke cancelled".into());
    }
    if !enabled || blocked || !input.buttons.pressed(MouseButton::Left) {
        paint.finish(&mut dense, &mut history, false);
    }
    paint.hover = None;
    paint.ready = false;
    if !enabled || blocked {
        return;
    }
    let Some(space) = input.origin.space() else {
        return;
    };
    let Some(definition) = dense
        .definition(space)
        .or_else(|| project.environments().iter().find(|d| d.space == space))
        .cloned()
    else {
        paint.status = Some("This world has no environment definition".into());
        return;
    };
    if paint
        .stroke
        .as_ref()
        .is_some_and(|stroke| stroke.space != space)
    {
        paint.finish(&mut dense, &mut history, false);
    }
    if paint.space != Some(space) {
        paint.space = Some(space);
        paint.selected = None;
    }
    if !definition
        .layers
        .iter()
        .any(|layer| Some(layer.id) == paint.selected)
    {
        paint.selected = definition
            .layers
            .iter()
            .filter(|layer| layer.enabled)
            .max_by_key(|layer| (layer.order, layer.id))
            .map(|layer| layer.id);
    }
    let Some(layer) = paint.selected else {
        paint.status = Some("This world has no enabled paint layers".into());
        return;
    };
    if definition
        .layers
        .iter()
        .find(|l| l.id == layer)
        .is_some_and(|l| !l.enabled)
    {
        paint.status = Some("Enable this layer before painting".into());
        return;
    }
    let Some(cursor) = input.window.cursor_position() else {
        if let Some(stroke) = &mut paint.stroke {
            stroke.last_point = None;
        }
        return;
    };
    let Ok(ray) = input.camera.0.viewport_to_world(input.camera.1, cursor) else {
        return;
    };
    let filter = |entity| {
        input
            .terrain
            .get(entity)
            .is_ok_and(|(_, terrain)| terrain.key.space == space)
    };
    let settings = MeshRayCastSettings::default()
        .with_filter(&filter)
        .always_early_exit();
    let hit = raycast
        .cast_ray(ray, &settings)
        .first()
        .map(|(entity, hit)| (*entity, hit.point));
    let Some((entity, point)) = hit else {
        if let Some(stroke) = &mut paint.stroke {
            stroke.last_point = None;
        }
        return;
    };
    let Ok((_, terrain)) = input.terrain.get(entity) else {
        return;
    };
    let origin = input.origin.cell().origin(definition.cell_size);
    let logical = [
        origin[0] + f64::from(point.x),
        origin[1] + f64::from(point.z),
    ];
    paint.hover = Some(BrushHit {
        point,
        cell: terrain.key.cell,
    });
    project.focus_environment(space, terrain.key.cell);
    let brush = paint
        .stroke
        .as_ref()
        .map_or(paint.brush, |stroke| stroke.brush);
    let from = paint
        .stroke
        .as_ref()
        .and_then(|stroke| stroke.last_point)
        .unwrap_or(logical);
    let ready = brush
        .cells(definition.cell_size, from, logical)
        .and_then(|cells| {
            let snapshot = project
                .environment_snapshot()
                .ok_or("Loading paint area…")?;
            if snapshot.definition.space != space
                || snapshot.definition.revision != definition.revision
            {
                return Err("Loading paint area…");
            }
            if !cells.iter().all(|cell| {
                dense
                    .environment_record(space, *cell)
                    .is_some_and(|r| r.definition_revision == definition.revision)
            }) {
                return Err("Loading paint area… Move closer or use a smaller brush.");
            }
            let halo = world_db::environment_dependency_cells(&cells)
                .map_err(|_| "Brush covers too much area")?;
            if !halo.iter().all(|cell| {
                snapshot
                    .coverage
                    .cells
                    .iter()
                    .any(|loaded| loaded.cell == *cell)
            }) {
                return Err("Loading neighboring paint cells…");
            }
            Ok(cells)
        });
    let cells = match ready {
        Ok(cells) => {
            paint.ready = true;
            cells
        }
        Err(error) => {
            paint.status = Some(error.into());
            if let Some(stroke) = &mut paint.stroke {
                stroke.last_point = None;
            }
            return;
        }
    };
    if input.buttons.just_pressed(MouseButton::Left) {
        let mut brush = paint.brush;
        if input.keys.pressed(KeyCode::ShiftLeft) || input.keys.pressed(KeyCode::ShiftRight) {
            brush.operation = BrushOperation::Erase;
        }
        paint.stroke = Some(Stroke {
            space,
            layer,
            brush,
            before: BTreeMap::new(),
            last_point: None,
        });
        dense.gesture_active = true;
    }
    let Some(stroke) = &mut paint.stroke else {
        paint.status = None;
        return;
    };
    match apply_segment(stroke, &definition, &cells, logical, &mut dense) {
        Ok(()) => paint.status = None,
        Err(error) => {
            stroke.last_point = None;
            paint.status = Some(error);
        }
    }
}

fn apply_segment(
    stroke: &mut Stroke,
    definition: &EnvironmentDefinition,
    cells: &[CellCoord],
    to: [f64; 2],
    dense: &mut DenseDomainWorkingSets,
) -> Result<(), String> {
    let from = stroke.last_point.unwrap_or(to);
    let mut originals = Vec::new();
    let mut replacements = Vec::new();
    for &cell in cells {
        let current = dense
            .environment_record(stroke.space, cell)
            .ok_or("Loading paint area…")?;
        let before = stroke.before.get(&cell).unwrap_or(current);
        let result = stroke.brush.apply_segment(
            definition,
            stroke.layer,
            &before.coverage(),
            &current.coverage(),
            from,
            to,
        )?;
        if result.tiles != current.tiles {
            originals.push(before.clone());
            replacements.push(SourceEnvironmentCellRecord {
                tiles: result.tiles,
                ..current.clone()
            });
        }
    }
    // One drag always fits in the history budget, even when it revisits previously saved cells.
    let mut stroke_bytes = stroke
        .before
        .values()
        .map(|r| r.sample_bytes() + 128)
        .sum::<usize>();
    let new_cells = originals
        .iter()
        .filter(|r| !stroke.before.contains_key(&r.cell))
        .collect::<Vec<_>>();
    stroke_bytes += new_cells
        .iter()
        .map(|r| r.sample_bytes() + 128)
        .sum::<usize>();
    let after_bytes = stroke
        .before
        .values()
        .chain(
            new_cells
                .into_iter()
                .map(|r| r as &SourceEnvironmentCellRecord),
        )
        .map(|r| {
            replacements
                .iter()
                .find(|new| new.cell == r.cell)
                .or_else(|| dense.environment_record(r.space, r.cell))
                .map_or(0, |r| r.sample_bytes() + 128)
        })
        .sum::<usize>();
    if stroke.before.len()
        + originals
            .iter()
            .filter(|r| !stroke.before.contains_key(&r.cell))
            .count()
        > 64
        || stroke_bytes + after_bytes > 6 * 1024 * 1024
    {
        return Err("Release the brush to finish this stroke before painting more area".into());
    }
    dense.apply_environment_records(&replacements)?;
    for before in originals {
        stroke.before.entry(before.cell).or_insert(before);
    }
    stroke.last_point = Some(to);
    Ok(())
}

fn draw_brush(
    paint: Res<EnvironmentPaintState>,
    origin: Res<WorldOrigin>,
    terrain: Query<&StreamedTerrainSurface>,
    mut gizmos: Gizmos<EditorOverlayGizmos>,
) {
    let Some(hit) = paint.hover else {
        return;
    };
    let brush = paint
        .stroke
        .as_ref()
        .map_or(paint.brush, |stroke| stroke.brush);
    let color = if !paint.ready {
        Color::srgb(1.0, 0.7, 0.1)
    } else if brush.operation == BrushOperation::Erase {
        Color::srgb(1.0, 0.4, 0.2)
    } else {
        Color::srgb(0.2, 1.0, 0.7)
    };
    for radius in [brush.radius, brush.radius * (1.0 - brush.falloff)] {
        if radius < 0.01 {
            continue;
        }
        let points = (0..=64).map(|i| {
            let angle = i as f32 * std::f32::consts::TAU / 64.0;
            let xz = Vec2::new(
                hit.point.x + angle.cos() * radius as f32,
                hit.point.z + angle.sin() * radius as f32,
            );
            let height =
                engine::sample_resident_terrain_surface(&origin, terrain.iter(), xz.into())
                    .map_or(hit.point.y, |sample| sample.height);
            Vec3::new(xz.x, height + 0.06, xz.y)
        });
        gizmos.linestrip(points, color);
    }
}

#[cfg(test)]
mod tests;
