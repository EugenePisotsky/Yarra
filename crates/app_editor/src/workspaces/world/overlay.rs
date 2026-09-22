//! World grid, source handles and selected-object bounds.
use crate::{
    editing::{EditorObjectWorkingSet, EditorSelection},
    project_store::ProjectEditorStore,
    workspaces::world::objects::{
        PromotedEditorObject, source_object_position, visual_bounds_corners,
    },
};
use bevy::prelude::*;
use engine::{WorldCatalog, WorldOrigin, WorldViewpoint};

const GRID_RADIUS_CELLS: i32 = 12;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub(crate) struct EditorOverlayGizmos;
pub(crate) fn draw_editor_grid(
    mut gizmos: Gizmos,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    viewpoint: Res<WorldViewpoint>,
) {
    let Some(position) = viewpoint.position() else {
        return;
    };
    let Some(space) = catalog.world_space(position.space) else {
        return;
    };
    let center_cell = position.cell;
    let minimum_x = relative_cell_offset(
        center_cell.x.saturating_sub(GRID_RADIUS_CELLS),
        origin.cell().x,
        space.cell_size,
    );
    let maximum_x = relative_cell_offset(
        center_cell.x.saturating_add(GRID_RADIUS_CELLS + 1),
        origin.cell().x,
        space.cell_size,
    );
    let minimum_z = relative_cell_offset(
        center_cell.z.saturating_sub(GRID_RADIUS_CELLS),
        origin.cell().z,
        space.cell_size,
    );
    let maximum_z = relative_cell_offset(
        center_cell.z.saturating_add(GRID_RADIUS_CELLS + 1),
        origin.cell().z,
        space.cell_size,
    );
    let y = 0.03;

    for offset in -GRID_RADIUS_CELLS..=GRID_RADIUS_CELLS + 1 {
        let cell_x = center_cell.x.saturating_add(offset);
        let x = relative_cell_offset(cell_x, origin.cell().x, space.cell_size);
        let color = grid_color(cell_x);
        gizmos.line(
            Vec3::new(x, y, minimum_z),
            Vec3::new(x, y, maximum_z),
            color,
        );

        let cell_z = center_cell.z.saturating_add(offset);
        let z = relative_cell_offset(cell_z, origin.cell().z, space.cell_size);
        let color = grid_color(cell_z);
        gizmos.line(
            Vec3::new(minimum_x, y, z),
            Vec3::new(maximum_x, y, z),
            color,
        );
    }
}

pub(crate) fn draw_promoted_editor_object(
    mut gizmos: Gizmos<EditorOverlayGizmos>,
    proxies: Query<(&PromotedEditorObject, &Transform)>,
    selection: Res<EditorSelection>,
    objects: Res<EditorObjectWorkingSet>,
) {
    for (proxy, transform) in &proxies {
        if !selection.contains(proxy.id) {
            continue;
        }
        let bounds = objects
            .current_view(proxy.id)
            .and_then(|selected| selected.visual_bounds)
            .unwrap_or([1.0, 1.0, 1.0]);
        let corners = visual_bounds_corners(transform, bounds);
        let color = if selection.selected_id() == Some(proxy.id) {
            Color::srgb(0.31, 0.67, 1.0)
        } else {
            Color::srgb(0.95, 0.72, 0.24)
        };
        for (start, end) in [
            (0, 1),
            (1, 3),
            (3, 2),
            (2, 0),
            (4, 5),
            (5, 7),
            (7, 6),
            (6, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            gizmos.line(corners[start], corners[end], color);
        }
    }
}

pub(crate) fn draw_source_object_handles(
    mut gizmos: Gizmos,
    project: Res<ProjectEditorStore>,
    selection: Res<EditorSelection>,
    objects: Res<EditorObjectWorkingSet>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    viewpoint: Res<WorldViewpoint>,
) {
    if project.desired_window() != project.loaded_window() {
        return;
    }
    let Some(position) = viewpoint.position() else {
        return;
    };
    let Some(space) = catalog.world_space(position.space) else {
        return;
    };
    let color = Color::srgba(0.25, 0.78, 0.92, 0.8);
    for record in project.objects().iter().filter_map(|record| {
        objects.resolve_source_view(record).filter(|record| {
            record.object.space == position.space && !selection.contains(record.object.id)
        })
    }) {
        let center = Vec3::from_array(
            source_object_position(&record).relative_to(origin.cell(), space.cell_size),
        );
        gizmos.line(center - Vec3::Y * 0.25, center + Vec3::Y * 0.75, color);
        gizmos.line(center - Vec3::X * 0.25, center + Vec3::X * 0.25, color);
    }
}

fn relative_cell_offset(cell: i32, origin: i32, cell_size: f32) -> f32 {
    (i64::from(cell) - i64::from(origin)) as f32 * cell_size
}

fn grid_color(coordinate: i32) -> Color {
    if coordinate == 0 {
        Color::srgba(0.75, 0.42, 0.18, 0.8)
    } else if coordinate.rem_euclid(4) == 0 {
        Color::srgba(0.42, 0.46, 0.54, 0.72)
    } else {
        Color::srgba(0.28, 0.31, 0.36, 0.55)
    }
}
