//! Region-oriented ground-cover painting over the bounded dense-source working set.

use std::collections::HashMap;

use bevy::{prelude::*, window::PrimaryWindow};
use engine::{WorldCatalog, WorldOrigin, WorldViewCamera};
use world::{CellCoord, GroundCoverRegionId, WorldSpaceId};
use world_db::{DenseSourceRecordKey, SourceGroundCoverCellMaskRecord};

use crate::{
    catalog_editing::GroundCoverRegionWorkingSet,
    domain_editing::{DenseDomainWorkingSets, GroundCoverMaskPatch},
    editing::EditorHistory,
    project_store::ProjectEditorStore,
    shell::EditorInputCapture,
    tools::{EditorToolRegistry, GROUND_COVER_TOOL},
    workspaces::{EditorWorkspace, world_impl::EditorOverlayGizmos},
};

const DEFAULT_MASK_RESOLUTION: u8 = 16;
const MINIMUM_BRUSH_RADIUS: f32 = 0.25;
const MAXIMUM_BRUSH_RADIUS: f32 = 16.0;
const DEFAULT_BRUSH_HARDNESS: f32 = 0.65;
const BRUSH_AREA_SAMPLES_PER_AXIS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum GroundCoverBrushMode {
    #[default]
    Paint,
    Erase,
}

impl GroundCoverBrushMode {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Paint => "Paint",
            Self::Erase => "Erase",
        }
    }

    fn apply_influence(self, current: u8, influence: u8) -> u8 {
        match self {
            Self::Paint => current.max(influence),
            Self::Erase => current.min(u8::MAX - influence),
        }
    }
}

#[derive(Resource, Debug)]
pub(crate) struct GroundCoverToolState {
    pub(crate) selected_region: Option<GroundCoverRegionId>,
    pub(crate) mode: GroundCoverBrushMode,
    pub(crate) radius: f32,
    /// Fraction of the radius that receives full influence; the remainder is a smooth falloff.
    pub(crate) hardness: f32,
    hover: Option<Vec3>,
}

impl Default for GroundCoverToolState {
    fn default() -> Self {
        Self {
            selected_region: None,
            mode: GroundCoverBrushMode::Paint,
            radius: 2.0,
            hardness: DEFAULT_BRUSH_HARDNESS,
            hover: None,
        }
    }
}

#[derive(Resource, Default)]
pub(crate) struct GroundCoverBrushGesture {
    active: bool,
    region: Option<GroundCoverRegionId>,
    before: HashMap<DenseSourceRecordKey, Option<SourceGroundCoverCellMaskRecord>>,
    previous_sample: Option<Vec2>,
}

impl GroundCoverBrushGesture {
    fn clear(&mut self) {
        self.active = false;
        self.region = None;
        self.before.clear();
        self.previous_sample = None;
    }
}

pub(crate) fn reconcile_ground_cover_tool_state(
    project: Res<ProjectEditorStore>,
    regions: Res<GroundCoverRegionWorkingSet>,
    mut tool: ResMut<GroundCoverToolState>,
) {
    let current_regions = regions.current_records(&project);
    let selected_is_valid = tool
        .selected_region
        .is_some_and(|selected| current_regions.iter().any(|region| region.id == selected));
    if !selected_is_valid {
        tool.selected_region = current_regions
            .iter()
            .find(|region| region.enabled)
            .or_else(|| current_regions.first())
            .map(|region| region.id);
    }
    tool.radius = tool
        .radius
        .clamp(MINIMUM_BRUSH_RADIUS, MAXIMUM_BRUSH_RADIUS);
    tool.hardness = tool.hardness.clamp(0.0, 1.0);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update_ground_cover_brush(
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mouse: Res<ButtonInput<MouseButton>>,
    capture: Res<EditorInputCapture>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    project: Res<ProjectEditorStore>,
    regions: Res<GroundCoverRegionWorkingSet>,
    tools: Res<EditorToolRegistry>,
    mut tool: ResMut<GroundCoverToolState>,
    mut gesture: ResMut<GroundCoverBrushGesture>,
    mut dense_domains: ResMut<DenseDomainWorkingSets>,
    mut history: ResMut<EditorHistory>,
) {
    let ground_cover_active = tools
        .active(EditorWorkspace::World)
        .is_some_and(|active| active.id == GROUND_COVER_TOOL.id);
    let selected_region = tool.selected_region.and_then(|id| {
        regions
            .current(&project, id)
            .filter(|region| region.enabled)
    });

    let hit = if ground_cover_active && !capture.wants_pointer {
        selected_region.as_ref().and_then(|region| {
            let cursor = window.cursor_position()?;
            let ray = camera.0.viewport_to_world(camera.1, cursor).ok()?;
            ground_cover_hit(&ray, region.space, &catalog, &origin, project.cells())
        })
    } else {
        None
    };
    tool.hover = hit.map(|hit| hit.position);

    if gesture.active
        && (!ground_cover_active
            || mouse.just_released(MouseButton::Left)
            || gesture.region != tool.selected_region)
    {
        finish_gesture(&mut gesture, &dense_domains, &mut history);
    }
    if !ground_cover_active || selected_region.is_none() {
        return;
    }

    if mouse.just_pressed(MouseButton::Left) && hit.is_some() {
        gesture.clear();
        gesture.active = true;
        gesture.region = tool.selected_region;
    }
    if !gesture.active || !mouse.pressed(MouseButton::Left) {
        return;
    }
    let Some(hit) = hit else {
        return;
    };
    let region = selected_region.expect("an active gesture has a selected region");
    let resolution = dense_domains
        .ground_cover_region_resolution(region.id)
        .or_else(|| {
            project
                .ground_cover_masks()
                .iter()
                .find(|mask| mask.region == region.id)
                .map(|mask| mask.resolution)
        })
        .unwrap_or(DEFAULT_MASK_RESOLUTION);
    let current = Vec2::new(hit.position.x, hit.position.z);
    let samples = interpolated_samples(
        gesture.previous_sample,
        current,
        (tool.radius * 0.45).max(0.1),
    );
    for sample in samples {
        paint_sample(
            sample,
            region.id,
            region.space,
            tool.mode,
            tool.radius,
            tool.hardness,
            resolution,
            &catalog,
            &origin,
            &project,
            &mut gesture,
            &mut dense_domains,
        );
    }
    gesture.previous_sample = Some(current);
}

pub(crate) fn draw_ground_cover_brush(
    tool: Res<GroundCoverToolState>,
    tools: Res<EditorToolRegistry>,
    mut gizmos: Gizmos<EditorOverlayGizmos>,
) {
    if !tools
        .active(EditorWorkspace::World)
        .is_some_and(|active| active.id == GROUND_COVER_TOOL.id)
    {
        return;
    }
    let Some(position) = tool.hover else {
        return;
    };
    let color = match tool.mode {
        GroundCoverBrushMode::Paint => Color::srgb(0.25, 0.95, 0.45),
        GroundCoverBrushMode::Erase => Color::srgb(1.0, 0.35, 0.25),
    };
    gizmos.circle(
        Isometry3d::new(
            position + Vec3::Y * 0.04,
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
        ),
        tool.radius,
        color,
    );
    if tool.hardness > 0.0 && tool.hardness < 1.0 {
        gizmos.circle(
            Isometry3d::new(
                position + Vec3::Y * 0.045,
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
            tool.radius * tool.hardness,
            color.with_alpha(0.55),
        );
    }
}

pub(crate) fn cancel_ground_cover_brush(
    mut gesture: ResMut<GroundCoverBrushGesture>,
    mut tool: ResMut<GroundCoverToolState>,
    mut dense_domains: ResMut<DenseDomainWorkingSets>,
) {
    if gesture.active {
        let patches = gesture_patches(&gesture, &dense_domains);
        if !patches.is_empty() {
            dense_domains.apply_ground_cover_patches(&patches, false);
        }
    }
    gesture.clear();
    tool.hover = None;
}

#[derive(Debug, Clone, Copy)]
struct GroundHit {
    position: Vec3,
}

fn ground_cover_hit(
    ray: &Ray3d,
    space: WorldSpaceId,
    catalog: &WorldCatalog,
    origin: &WorldOrigin,
    cells: &[world_db::SourceCellRecord],
) -> Option<GroundHit> {
    let world_space = catalog.world_space(space)?;
    let direction = *ray.direction;
    if direction.y.abs() < 1.0e-5 {
        return None;
    }
    cells
        .iter()
        .filter(|cell| cell.space == space)
        .filter_map(|cell| {
            let distance = (cell.height - ray.origin.y) / direction.y;
            if distance <= 0.0 {
                return None;
            }
            let position = ray.get_point(distance);
            let minimum = cell_render_minimum(cell.cell, origin.cell(), world_space.cell_size);
            (position.x >= minimum.x
                && position.x < minimum.x + world_space.cell_size
                && position.z >= minimum.y
                && position.z < minimum.y + world_space.cell_size)
                .then_some((distance, GroundHit { position }))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, hit)| hit)
}

#[allow(clippy::too_many_arguments)]
fn paint_sample(
    center: Vec2,
    region: GroundCoverRegionId,
    space: WorldSpaceId,
    mode: GroundCoverBrushMode,
    radius: f32,
    hardness: f32,
    default_resolution: u8,
    catalog: &WorldCatalog,
    origin: &WorldOrigin,
    project: &ProjectEditorStore,
    gesture: &mut GroundCoverBrushGesture,
    dense_domains: &mut DenseDomainWorkingSets,
) {
    let Some(world_space) = catalog.world_space(space) else {
        return;
    };
    let radius_squared = radius * radius;
    for cell in project.cells().iter().filter(|cell| cell.space == space) {
        let minimum = cell_render_minimum(cell.cell, origin.cell(), world_space.cell_size);
        if squared_distance_to_cell(center, minimum, world_space.cell_size) > radius_squared {
            continue;
        }
        let existing = dense_domains.ground_cover_mask(region, space, cell.cell);
        if existing.is_none() && mode == GroundCoverBrushMode::Erase {
            continue;
        }
        let resolution = existing
            .as_ref()
            .map_or(default_resolution, |mask| mask.resolution);
        let resolution_usize = usize::from(resolution);
        if resolution_usize == 0 {
            continue;
        }
        let mut coverage = existing.as_ref().map_or_else(
            || vec![0; resolution_usize * resolution_usize],
            |mask| mask.coverage.clone(),
        );
        let sample_size = world_space.cell_size / f32::from(resolution);
        let mut changed = false;
        for z in 0..resolution_usize {
            for x in 0..resolution_usize {
                let sample_minimum =
                    minimum + Vec2::new(x as f32 * sample_size, z as f32 * sample_size);
                let influence =
                    brush_sample_coverage(center, sample_minimum, sample_size, radius, hardness);
                if influence == 0 {
                    continue;
                }
                let index = z * resolution_usize + x;
                let value = mode.apply_influence(coverage[index], influence);
                if coverage[index] != value {
                    coverage[index] = value;
                    changed = true;
                }
            }
        }
        if !changed {
            continue;
        }
        let key = DenseSourceRecordKey::GroundCoverMask {
            region,
            space,
            cell: cell.cell,
        };
        gesture.before.entry(key).or_insert(existing);
        dense_domains.set_ground_cover_coverage(region, space, cell.cell, resolution, coverage);
    }
}

/// Integrates the circular brush over one square mask sample instead of testing only its center.
///
/// The runtime deliberately keeps one cluster per mask sample. Fractional coverage makes a curved
/// stroke resolve into stable density at that boundary without refining or multiplying the cluster
/// grid. Regular stratified samples are deterministic, so repainting the same stroke cannot
/// introduce noise.
fn brush_sample_coverage(
    brush_center: Vec2,
    sample_minimum: Vec2,
    sample_size: f32,
    radius: f32,
    hardness: f32,
) -> u8 {
    if !sample_size.is_finite() || sample_size <= 0.0 || !radius.is_finite() || radius <= 0.0 {
        return 0;
    }

    let step = sample_size / BRUSH_AREA_SAMPLES_PER_AXIS as f32;
    let mut total = 0.0;
    for sample_z in 0..BRUSH_AREA_SAMPLES_PER_AXIS {
        for sample_x in 0..BRUSH_AREA_SAMPLES_PER_AXIS {
            let point = sample_minimum
                + Vec2::new(
                    (sample_x as f32 + 0.5) * step,
                    (sample_z as f32 + 0.5) * step,
                );
            total += radial_brush_influence(point.distance(brush_center), radius, hardness);
        }
    }
    let sample_count = (BRUSH_AREA_SAMPLES_PER_AXIS * BRUSH_AREA_SAMPLES_PER_AXIS) as f32;
    (total / sample_count * f32::from(u8::MAX)).round() as u8
}

fn radial_brush_influence(distance: f32, radius: f32, hardness: f32) -> f32 {
    if distance >= radius {
        return 0.0;
    }
    let hard_radius = radius * hardness.clamp(0.0, 1.0);
    if distance <= hard_radius || hard_radius >= radius - f32::EPSILON {
        return 1.0;
    }
    let inward = ((radius - distance) / (radius - hard_radius)).clamp(0.0, 1.0);
    inward * inward * (3.0 - 2.0 * inward)
}

fn finish_gesture(
    gesture: &mut GroundCoverBrushGesture,
    dense_domains: &DenseDomainWorkingSets,
    history: &mut EditorHistory,
) {
    let patches = gesture_patches(gesture, dense_domains);
    history.record_ground_cover_previews(patches);
    gesture.clear();
}

fn gesture_patches(
    gesture: &GroundCoverBrushGesture,
    dense_domains: &DenseDomainWorkingSets,
) -> Vec<GroundCoverMaskPatch> {
    let mut patches = gesture
        .before
        .iter()
        .filter_map(|(key, before)| {
            let DenseSourceRecordKey::GroundCoverMask {
                region,
                space,
                cell,
            } = key
            else {
                return None;
            };
            let after = dense_domains.ground_cover_mask(*region, *space, *cell)?;
            GroundCoverMaskPatch::between(before.as_ref(), &after)
        })
        .collect::<Vec<_>>();
    patches.sort_by_key(|patch| (patch.cell.x, patch.cell.z, patch.region.0));
    patches
}

fn interpolated_samples(previous: Option<Vec2>, current: Vec2, spacing: f32) -> Vec<Vec2> {
    let Some(previous) = previous else {
        return vec![current];
    };
    let distance = previous.distance(current);
    let segments = (distance / spacing).ceil().max(1.0) as usize;
    (1..=segments)
        .map(|step| previous.lerp(current, step as f32 / segments as f32))
        .collect()
}

fn cell_render_minimum(cell: CellCoord, origin: CellCoord, cell_size: f32) -> Vec2 {
    Vec2::new(
        (i64::from(cell.x) - i64::from(origin.x)) as f32 * cell_size,
        (i64::from(cell.z) - i64::from(origin.z)) as f32 * cell_size,
    )
}

fn squared_distance_to_cell(point: Vec2, minimum: Vec2, cell_size: f32) -> f32 {
    let maximum = minimum + Vec2::splat(cell_size);
    let closest = point.clamp(minimum, maximum);
    point.distance_squared(closest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_interpolation_never_leaves_more_than_the_requested_spacing() {
        let samples = interpolated_samples(Some(Vec2::ZERO), Vec2::new(3.0, 0.0), 0.75);
        assert_eq!(samples.len(), 4);
        let mut previous = Vec2::ZERO;
        for sample in samples {
            assert!(sample.distance(previous) <= 0.75 + f32::EPSILON);
            previous = sample;
        }
    }

    #[test]
    fn cell_distance_is_zero_inside_and_squared_outside() {
        assert_eq!(
            squared_distance_to_cell(Vec2::new(2.0, 3.0), Vec2::ZERO, 4.0),
            0.0
        );
        assert_eq!(
            squared_distance_to_cell(Vec2::new(7.0, 4.0), Vec2::ZERO, 4.0),
            9.0
        );
    }

    #[test]
    fn brush_falloff_has_a_full_core_and_smooth_edge() {
        assert_eq!(radial_brush_influence(0.9, 2.0, 0.5), 1.0);
        let feather = radial_brush_influence(1.5, 2.0, 0.5);
        assert!(feather > 0.0 && feather < 1.0);
        assert_eq!(radial_brush_influence(2.0, 2.0, 0.5), 0.0);
    }

    #[test]
    fn brush_area_sampling_produces_partial_boundary_coverage() {
        let full = brush_sample_coverage(Vec2::ZERO, Vec2::splat(-0.25), 0.5, 2.0, 0.5);
        let edge = brush_sample_coverage(Vec2::ZERO, Vec2::new(1.0, -0.5), 1.0, 2.0, 0.5);
        let outside = brush_sample_coverage(Vec2::ZERO, Vec2::new(2.1, 0.0), 0.5, 2.0, 0.5);
        assert_eq!(full, u8::MAX);
        assert!(edge > 0 && edge < u8::MAX);
        assert_eq!(outside, 0);
    }

    #[test]
    fn brush_modes_apply_partial_influence_idempotently() {
        assert_eq!(GroundCoverBrushMode::Paint.apply_influence(0, 96), 96);
        assert_eq!(GroundCoverBrushMode::Paint.apply_influence(128, 96), 128);
        assert_eq!(GroundCoverBrushMode::Erase.apply_influence(255, 96), 159);
        assert_eq!(GroundCoverBrushMode::Erase.apply_influence(128, 96), 128);
    }
}
