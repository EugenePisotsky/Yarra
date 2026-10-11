//! Pure CPU scene packing, LOD budget and conservative visibility bounds.
use super::gpu_types::{GrassTypeGpu, HIGH_CAPACITY, SurfaceSampleGpu, WorkItemGpu};
use crate::VegetationWind;
use bevy::prelude::*;
use vegetation::{GrassType, candidate_domain, decode_octahedral_normal};

// Keep high-detail blades below the arena's hard guard even for a top-down view of a fully covered
// field. The remaining headroom absorbs placement variance, clump pull, page boundaries and the
// smooth high/low transition annulus.
const HIGH_DETAIL_BUDGET_UTILIZATION: f32 = 0.5;
const MAX_HIGH_DETAIL_RADIUS: f32 = 96.0;

/// Farthest a blade reaches sideways from its root at rest, as a share of its height: full
/// tilt plus its arch.
const REST_REACH: f32 = 1.25;
/// Bounds wind displacement per metre of blade height at unit wind strength, as the shaders'
/// `WIND_REACH`.
const WIND_REACH: f32 = 1.65;

/// The reference packing, compared with the incremental page layout in tests.
#[cfg(test)]
pub(super) struct PackedScene {
    pub(super) work_items: Vec<WorkItemGpu>,
    pub(super) coverage: Vec<f32>,
    pub(super) surfaces: Vec<SurfaceSampleGpu>,
    pub(super) types: Vec<GrassTypeGpu>,
    pub(super) maximum_candidate_count: u32,
    pub(super) high_detail_radius: f32,
}

pub(super) fn pack_lod_focus(focus: Option<Vec3>, camera: Vec3, forward: Vec3) -> [f32; 4] {
    let Some(focus) = focus.filter(|v| v.is_finite()) else {
        return [camera.x, camera.z, 0.0, 0.0];
    };
    // A vertical view has no preferred ground direction and keeps a circular footprint.
    let horizontal = forward.xz();
    let direction = if horizontal.length_squared() > 1e-4 {
        horizontal.normalize()
    } else {
        Vec2::ZERO
    };
    let center = focus.xz() + direction * 2.0;
    [center.x, center.y, direction.x, direction.y]
}

/// The radius of high-detail blades around the detail centre: what the high bin holds at the
/// scene's densest grass, with headroom.
fn high_detail_radius(scene: &vegetation::VegetationScene) -> f32 {
    let density = scene
        .pages
        .iter()
        .map(|page| {
            page.fields
                .iter()
                .map(|field| {
                    let peak = field.coverage.iter().copied().max().unwrap_or(0);
                    scene.catalog.grass(field.grass).map_or(0.0, |t| t.density) * f32::from(peak)
                        / 255.0
                })
                .sum::<f32>()
        })
        .fold(0.0_f32, f32::max);
    if density <= f32::EPSILON {
        return MAX_HIGH_DETAIL_RADIUS;
    }
    ((HIGH_CAPACITY as f32 * HIGH_DETAIL_BUDGET_UTILIZATION) / (std::f32::consts::PI * density))
        .sqrt()
        .min(MAX_HIGH_DETAIL_RADIUS)
}

/// Farthest a type's blades reach sideways from their roots at rest.
fn horizontal_reach(grass: &GrassType) -> f32 {
    grass.height[1] * REST_REACH + grass.width[1]
}

/// Conservative CPU counterpart of the GPU root range, including blade and wind reach.
pub fn terrain_contact_radius(
    catalog: &vegetation::VegetationCatalog,
    wind: &VegetationWind,
) -> f32 {
    let strength = if wind.enabled {
        wind.strength.max(0.)
    } else {
        0.
    };
    crate::PROCEDURAL_DISTANCE_METERS
        + catalog
            .grass
            .iter()
            .map(|g| horizontal_reach(g) + g.height[1] * strength * WIND_REACH)
            .fold(0., f32::max)
}

pub(super) fn pack_type(grass: &GrassType) -> GrassTypeGpu {
    let c = grass.clumps;
    let colors = grass.colors;
    GrassTypeGpu {
        size: [
            grass.height[0],
            grass.height[1],
            grass.width[0],
            grass.width[1],
        ],
        shape: [grass.tilt[0], grass.tilt[1], grass.bend[0], grass.bend[1]],
        clumps: [c.size, c.pull, c.height, c.facing],
        placement: [
            c.splay,
            grass.spacing(),
            f32::from_bits(grass.id().seed()),
            horizontal_reach(grass),
        ],
        root: [colors.root[0], colors.root[1], colors.root[2], colors.dry],
        tip: [
            colors.tip[0],
            colors.tip[1],
            colors.tip[2],
            colors.variation,
        ],
        dry_tip: [
            colors.dry_tip[0],
            colors.dry_tip[1],
            colors.dry_tip[2],
            c.patches,
        ],
        material: [
            grass.gloss,
            grass.roughness,
            grass.translucency,
            grass.stiffness,
        ],
        form: [f32::from(u8::from(grass.twin)), 0.0, 0.0, 0.0],
    }
}

#[cfg(test)]
pub(super) fn pack_scene(scene: &vegetation::VegetationScene) -> PackedScene {
    pack_scene_with_gate(scene, &default(), [0.; 2])
}

/// Where one page's samples live in the surface and coverage buffers: the surface offset and
/// one coverage offset per field, in elements.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageLayout {
    pub(super) surface: u32,
    pub(super) coverage: Vec<u32>,
}

/// Everything except the page samples. Work items and types depend on the whole resident scene,
/// so these are rebuilt for every revision; they are small.
pub(super) struct PackedItems {
    pub(super) work_items: Vec<WorkItemGpu>,
    pub(super) types: Vec<GrassTypeGpu>,
    pub(super) maximum_candidate_count: u32,
    pub(super) high_detail_radius: f32,
}

/// A page's surface samples as uploaded. They do not depend on the render origin.
pub(super) fn pack_surface(
    page: &vegetation::VegetationFieldPage,
) -> impl Iterator<Item = SurfaceSampleGpu> + '_ {
    page.surface
        .heights
        .iter()
        .zip(&page.surface.normals_oct)
        .zip(&page.surface.validity)
        .map(|((height, normal), validity)| {
            let normal = decode_octahedral_normal(*normal);
            SurfaceSampleGpu {
                height_validity: [*height, f32::from(*validity) / 255.0, 0.0, 0.0],
                normal: [normal[0], normal[1], normal[2], 0.0],
            }
        })
}

/// A field's coverage samples as uploaded.
pub(super) fn pack_coverage(field: &vegetation::GrassField) -> impl Iterator<Item = f32> + '_ {
    field.coverage.iter().map(|value| f32::from(*value) / 255.0)
}

/// The reference packing: every page's samples in order, followed by its work items.
#[cfg(test)]
pub(super) fn pack_scene_with_gate(
    scene: &vegetation::VegetationScene,
    gate: &crate::VegetationTerrainGate,
    origin: [f64; 2],
) -> PackedScene {
    let mut coverage = Vec::new();
    let mut surfaces = Vec::new();
    let layouts: Vec<_> = scene
        .pages
        .iter()
        .map(|page| {
            let surface = surfaces.len() as u32;
            surfaces.extend(pack_surface(page));
            let coverage_offsets = page
                .fields
                .iter()
                .map(|field| {
                    let offset = coverage.len() as u32;
                    coverage.extend(pack_coverage(field));
                    offset
                })
                .collect();
            PageLayout {
                surface,
                coverage: coverage_offsets,
            }
        })
        .collect();
    let items = pack_items(scene, gate, origin, &layouts);
    PackedScene {
        work_items: items.work_items,
        coverage,
        surfaces,
        types: items.types,
        maximum_candidate_count: items.maximum_candidate_count,
        high_detail_radius: items.high_detail_radius,
    }
}

/// Work items and grass types for `scene`, addressing each page's samples at `layouts[page]`.
pub(super) fn pack_items(
    scene: &vegetation::VegetationScene,
    gate: &crate::VegetationTerrainGate,
    origin: [f64; 2],
    layouts: &[PageLayout],
) -> PackedItems {
    debug_assert_eq!(layouts.len(), scene.pages.len());
    let types = scene.catalog.grass.iter().map(pack_type).collect();
    let mut work_items = Vec::new();
    let mut maximum_candidate_count = 0;
    for (page, layout) in scene.pages.iter().zip(layouts) {
        debug_assert_eq!(layout.coverage.len(), page.fields.len());
        let (minimum_height, maximum_height) = page.surface.heights.iter().fold(
            (f32::INFINITY, f32::NEG_INFINITY),
            |(minimum, maximum), height| (minimum.min(*height), maximum.max(*height)),
        );
        for (field, &coverage_offset) in page.fields.iter().zip(&layout.coverage) {
            let index = scene
                .catalog
                .index(field.grass)
                .expect("validated scene grass");
            // Lattices are world-aligned in canonical coordinates, independent of the render
            // origin the page is drawn relative to.
            let domain = candidate_domain(
                [
                    (f64::from(page.origin_xz[0]) + origin[0]) as f32,
                    (f64::from(page.origin_xz[1]) + origin[1]) as f32,
                ],
                page.size,
                &scene.catalog.grass[index],
            );
            maximum_candidate_count = maximum_candidate_count.max(domain.candidate_count());
            work_items.push(WorkItemGpu {
                page: [page.origin_xz[0], page.origin_xz[1], page.size, 0.0],
                domain: [
                    domain.cell_min[0],
                    domain.cell_min[1],
                    domain.cell_count[0] as i32,
                    domain.cell_count[1] as i32,
                ],
                field: [
                    domain.candidate_count(),
                    coverage_offset,
                    u32::from(field.resolution),
                    index as u32,
                ],
                surface: [
                    layout.surface,
                    u32::from(page.surface.resolution),
                    minimum_height.to_bits(),
                    maximum_height.to_bits(),
                ],
            });
        }
    }
    apply_terrain_gate(&mut work_items, gate);
    PackedItems {
        work_items,
        types,
        maximum_candidate_count,
        high_detail_radius: high_detail_radius(scene),
    }
}

/// A gate is a draw/scheduling input, never a change to the source grass.
/// Keeping work-item indices stable also preserves candidate masks.
pub(super) fn apply_terrain_gate(
    items: &mut [WorkItemGpu],
    gate: &crate::VegetationTerrainGate,
) -> bool {
    let mut changed = false;
    for item in items {
        let id = [
            item.page[0].to_bits(),
            item.page[1].to_bits(),
            item.page[2].to_bits(),
        ];
        let blocked = if gate.block_all || gate.blocked_pages.contains(&id) {
            1.
        } else {
            0.
        };
        changed |= item.page[3] != blocked;
        item.page[3] = blocked;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameplay_detail_centre_tracks_subject_instead_of_orbit_distance() {
        let subject = Vec3::new(32.0, 4.0, -64.0);
        let near = pack_lod_focus(
            Some(subject),
            subject + Vec3::new(0.0, 1.6, 4.0),
            Vec3::new(0.0, -0.2, -1.0),
        );
        let far = pack_lod_focus(
            Some(subject),
            subject + Vec3::new(0.0, 12.0, 18.0),
            Vec3::new(0.0, -0.7, -0.7),
        );
        assert_eq!(near, far);
        assert_eq!(near, [32.0, -66.0, 0.0, -1.0]);
        assert_eq!(
            pack_lod_focus(None, Vec3::new(5.0, 8.0, 9.0), Vec3::NEG_Z),
            [5.0, 9.0, 0.0, 0.0]
        );
        assert_eq!(
            pack_lod_focus(
                Some(subject),
                subject + Vec3::Y * 20.0,
                Vec3::new(0.0, -1.0, -1e-7)
            ),
            [32.0, -64.0, 0.0, 0.0]
        );
    }

    #[test]
    fn reference_fixture_packs_one_item_per_field() {
        let scene = vegetation::fixtures::reference_scene();
        let packed = pack_scene(&scene);
        assert_eq!(packed.work_items.len(), 4);
        assert_eq!(packed.types.len(), 2);
        assert!(packed.maximum_candidate_count > 0);
        assert_eq!(packed.surfaces.len(), scene.pages.len() * 4);
        assert_eq!(packed.coverage.len(), scene.pages.len() * 2 * 16 * 16);
        assert!(packed.work_items.iter().all(|i| i.field[3] < 2));
        // Densest point: full meadow (48/m²) plus the 20% of it left under the tuft band's
        // peak, beside the band's tufts (24/m²); the radius is what the high bin holds there.
        assert!((8.0..96.0).contains(&packed.high_detail_radius));
        let seed = packed.types[0].placement[2].to_bits();
        assert_eq!(seed, vegetation::fixtures::meadow().id().seed());
    }

    #[test]
    fn lattices_stay_world_aligned_when_the_render_origin_moves() {
        let scene = vegetation::fixtures::reference_scene();
        let here = pack_scene_with_gate(&scene, &default(), [0.0; 2]).work_items;
        let there = pack_scene_with_gate(&scene, &default(), [64.0, -32.0]).work_items;
        let cells = |items: &[WorkItemGpu]| items[0].domain;
        let spacing = vegetation::fixtures::meadow().spacing();
        let shift = [
            (64.0 / spacing).round() as i32,
            (-32.0 / spacing).round() as i32,
        ];
        let (a, b) = (cells(&here), cells(&there));
        assert!((b[0] - a[0] - shift[0]).abs() <= 1 && (b[1] - a[1] - shift[1]).abs() <= 1);
    }
}
