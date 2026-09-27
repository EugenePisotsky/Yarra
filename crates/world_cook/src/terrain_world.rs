//! Island worlds built from a height function: the procedural Phase 0 island and imported
//! heightfields share their catalog, cells, paint and start views.
use super::*;
use environment::CoverageTile;
use world::WorldViewBookmark;
use world_db::{ImportedTerrainCell, SourceEnvironmentCellRecord};

/// 1 m geometry and paint; ground weights are still compiled at 0.5 m.
pub(crate) const HEIGHT_SIDE: usize = 33;
pub(crate) const MASK_SIDE: usize = 33;
/// Composites stop at 128 m nodes (2 m texels); closer ground uses runtime near detail.
const COMPOSITE_MINIMUM_LEVEL: u8 = 2;
const VIEW_VISIBILITY: f32 = 20_000.;

/// Terrain height at a world XZ position.
pub(crate) type HeightFn<'a> = dyn Fn(Vec2) -> f32 + Sync + 'a;

/// The catalog and settings of an island world, without cells.
pub(crate) fn base_document(name: &str, bounds: [f32; 2], sea_level: f32) -> ProjectDocument {
    let mut project = road_demo::document();
    let space = project.default_world_space;
    project.world_spaces.retain(|s| s.id == space);
    let world = &mut project.world_spaces[0];
    world.name = name.into();
    world.cell_size = DEFAULT_CELL_SIZE;
    [world.minimum_y, world.maximum_y] = bounds;
    world.sea_level = Some(sea_level);
    project.environments.retain(|d| d.space == space);
    let definition = &mut project.environments[0];
    definition.cell_size = DEFAULT_CELL_SIZE;
    definition.mask_resolution = MASK_SIDE as u16;
    project.terrain_profiles.retain(|p| p.space == space);
    project.terrain_profiles[0].composite_minimum_level = COMPOSITE_MINIMUM_LEVEL;
    project.cells.clear();
    project.terrain_cell_heightfields.clear();
    project.environment_cells.clear();
    project.objects.clear();
    project.roads = Default::default();
    project
}

/// Adds imported cells to a whole document, as the import writer stores them.
pub(crate) fn push_cell(project: &mut ProjectDocument, imported: ImportedTerrainCell) {
    let space = project.default_world_space;
    let definition = &project.environments[0];
    project.cells.push(SourceCellRecord {
        space,
        cell: imported.cell,
        height: imported.height,
        source_revision: 1,
    });
    if let Some(heights) = imported.heights {
        project
            .terrain_cell_heightfields
            .push(SourceTerrainCellHeightfieldRecord {
                space,
                cell: imported.cell,
                resolution: imported.resolution,
                heights,
                source_revision: 1,
            });
    }
    if !imported.tiles.is_empty() {
        project.environment_cells.push(SourceEnvironmentCellRecord {
            space,
            cell: imported.cell,
            source_revision: 1,
            definition_revision: definition.revision,
            tiles: imported.tiles,
        });
    }
}

/// Dry meadow on land, green meadow in the lowlands and bare ground on the beach, steep
/// slopes and summits. There are no sand or rock textures yet.
fn paint(p: Vec2, height: f32, slope: f32) -> [f32; 3] {
    let land = smoothstep(0.6, 2.0, height);
    let moist = (0.5 + 0.9 * terrain_fbm(p / 450., 0x6b8e_2d17)).clamp(0., 1.);
    let green = land * (1. - smoothstep(110., 320., height)) * moist;
    let bare = land
        * (1. - smoothstep(1.8, 4.5, height))
            .max(smoothstep(0.62, 0.95, slope))
            .max(smoothstep(520., 640., height));
    [land, green, bare]
}

/// Heights and paint of one cell, sampled every metre on the world-wide grid, so neighbours
/// share their edges. Heights sit on the cooked 1/1024 m grid, so an exactly flat cell (open
/// sea) is detected and stores no heightfield. `edge` fades paint out towards the border of a
/// square world whose surroundings are unbuilt.
pub(crate) fn terrain_cell(
    cell: CellCoord,
    height: &HeightFn,
    layers: &[environment::LayerId],
    edge: Option<f32>,
) -> ImportedTerrainCell {
    let step = DEFAULT_CELL_SIZE / (HEIGHT_SIDE - 1) as f32;
    // One sample of halo on each side gives central-difference slopes that agree across
    // cell borders.
    let side = HEIGHT_SIDE + 2;
    let point = |i: usize, j: usize| {
        Vec2::new(
            cell.x as f32 * DEFAULT_CELL_SIZE + (i as f32 - 1.) * step,
            cell.z as f32 * DEFAULT_CELL_SIZE + (j as f32 - 1.) * step,
        )
    };
    let halo: Vec<f32> = (0..side)
        .flat_map(|j| (0..side).map(move |i| (i, j)))
        .map(|(i, j)| world::quantize_terrain_height(height(point(i, j))))
        .collect();
    let heights: Vec<f32> = (1..=HEIGHT_SIDE)
        .flat_map(|j| (1..=HEIGHT_SIDE).map(move |i| (i, j)))
        .map(|(i, j)| halo[j * side + i])
        .collect();
    let mut tiles: Vec<_> = layers
        .iter()
        .map(|&layer| CoverageTile {
            layer,
            samples: Vec::with_capacity(MASK_SIDE * MASK_SIDE),
        })
        .collect();
    for j in 1..=MASK_SIDE {
        for i in 1..=MASK_SIDE {
            let dx = (halo[j * side + i + 1] - halo[j * side + i - 1]) / (2. * step);
            let dz = (halo[(j + 1) * side + i] - halo[(j - 1) * side + i]) / (2. * step);
            let p = point(i, j);
            let fade = edge.map_or(1., |extent| {
                smoothstep(0., 48., extent - p.abs().max_element())
            });
            for (tile, weight) in tiles
                .iter_mut()
                .zip(paint(p, halo[j * side + i], dx.hypot(dz)))
            {
                tile.samples.push((weight * fade * 255.).round() as u8);
            }
        }
    }
    tiles.retain(|t| t.samples.iter().any(|&v| v != 0));
    let flat = heights.iter().all(|h| h.to_bits() == heights[0].to_bits());
    ImportedTerrainCell {
        cell,
        height: heights[0],
        heights: (!flat).then_some(heights),
        resolution: HEIGHT_SIDE as u16,
        tiles,
    }
}

/// The shore point nearest `from`, along 72 headings in 4 m steps up to 8 km.
pub(crate) fn nearest_shore(height: &HeightFn, from: Vec2) -> Option<Vec2> {
    let above = height(from) >= 1.5;
    (0..72)
        .filter_map(|i| {
            let direction = Vec2::from_angle((i as f32 * 5.).to_radians());
            (1..2000)
                .map(|step| from + direction * step as f32 * 4.)
                .find(|&p| (height(p) < 1.5) == above)
        })
        .min_by(|a, b| a.distance(from).total_cmp(&b.distance(from)))
}

/// Highest point within `radius` of `centre`: 41×41 searches, each 20 times finer, down to 2 m.
pub(crate) fn highest(height: &HeightFn, centre: Vec2, radius: f32) -> Vec2 {
    let search = |centre: Vec2, spacing: f32| {
        (0..=40)
            .flat_map(|j| {
                (0..=40).map(move |i| centre + Vec2::new(i as f32 - 20., j as f32 - 20.) * spacing)
            })
            .max_by(|a, b| height(*a).total_cmp(&height(*b)))
            .unwrap()
    };
    let (mut p, mut spacing) = (centre, radius / 20.);
    loop {
        p = search(p, spacing);
        if spacing <= 2. {
            return p;
        }
        spacing = (spacing / 20.).max(2.);
    }
}

/// The heading from `p` whose terrain rises least above eye level within 1.5 km: the most
/// open view.
pub(crate) fn open_heading(height: &HeightFn, p: Vec2) -> Vec2 {
    let eye = height(p) + 2.;
    (0..72)
        .map(|i| Vec2::from_angle((i as f32 * 5.).to_radians()))
        .min_by(|a, b| {
            let rise = |direction: Vec2| {
                (1..=75)
                    .map(|step| step as f32 * 20.)
                    .map(|d| (height(p + direction * d) - eye) / d)
                    .fold(f32::NEG_INFINITY, f32::max)
            };
            rise(*a).total_cmp(&rise(*b))
        })
        .unwrap()
}

/// A start view standing at `p` and looking along `look`. Low pitches keep the horizon in
/// frame: these views are about distance.
pub(crate) fn view(
    height: &HeightFn,
    p: Vec2,
    look: Vec2,
    pitch_degrees: f32,
    distance: f32,
) -> WorldViewBookmark {
    WorldViewBookmark {
        position: [p.x, height(p), p.y],
        // Camera yaw that looks along `look` from behind the player.
        yaw_degrees: (-look.x).atan2(-look.y).to_degrees(),
        pitch_degrees,
        distance,
        fog_visibility: VIEW_VISIBILITY,
        route: Vec::new(),
    }
}

/// Writes start views beside a project, replacing views of the same name.
pub(crate) fn write_views(project: &Path, views: &[(&str, WorldViewBookmark)]) -> Result<()> {
    let directory = project.with_extension("views");
    fs::create_dir_all(&directory)?;
    for (name, view) in views {
        view.validate().map_err(anyhow::Error::msg)?;
        fs::write(
            directory.join(format!("{name}.ron")),
            ron::ser::to_string_pretty(view, Default::default())?,
        )?;
    }
    Ok(())
}
