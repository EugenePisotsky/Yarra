//! Phase 0 stress world, currently the default: a ~28 km² island with a 745 m massif in an
//! 8 km square of sea. Heights and paint are procedural. Deep sea is exactly flat, so those cells
//! store neither heights nor paint.
use super::*;
use environment::CoverageTile;
use world::WorldViewBookmark;
use world_db::SourceEnvironmentCellRecord;

/// 8,192 m square of 32 m cells, so the hierarchy closes into four level-7 roots.
const HALF_CELLS: i32 = 128;
/// 1 m geometry and paint; ground weights are still compiled at 0.5 m.
const HEIGHT_SIDE: usize = 33;
const MASK_SIDE: usize = 33;
const SEA_LEVEL: f32 = 0.0;
const SEABED: f32 = -60.0;
const MINIMUM_HEIGHT: f32 = -80.0;
const MAXIMUM_HEIGHT: f32 = 1000.0;
/// Mean coast distance before the coastline noise, stretched along X.
const COAST_RADIUS: f32 = 3150.0;
const COAST_STRETCH: Vec2 = Vec2::new(1.1, 0.92);
const MASSIF: Vec2 = Vec2::new(-1350., -1250.);
const PEAK: Vec2 = Vec2::new(-1500., -1400.);
const HILLS: Vec2 = Vec2::new(1600., 500.);
/// Composites stop at 128 m nodes (2 m texels); closer ground uses runtime near detail.
const COMPOSITE_MINIMUM_LEVEL: u8 = 2;
const VIEW_VISIBILITY: f32 = 20_000.;

pub fn create_island_world(path: &Path) -> Result<()> {
    let views = path.with_extension("views");
    if path.exists() || views.exists() {
        bail!(
            "the island needs a new project and viewpoint directory: {}",
            path.display()
        );
    }
    write_project_database(path, &document(HALF_CELLS))
        .with_context(|| format!("failed to create the island at {}", path.display()))?;
    fs::create_dir_all(&views)?;
    for (name, view) in viewpoints() {
        view.validate().map_err(anyhow::Error::msg)?;
        fs::write(
            views.join(format!("{name}.ron")),
            ron::ser::to_string_pretty(&view, Default::default())?,
        )?;
    }
    Ok(())
}

/// Positive on land, zero at the coast. Low-frequency noise bends the coast into bays,
/// headlands and a lagoon.
fn land_index(p: Vec2) -> f32 {
    let bend = terrain_fbm(p / 1700. + Vec2::new(3.7, -1.9), 0x1a2b_3c4d) * 0.45
        + terrain_fbm(p / 520. + Vec2::new(-8.2, 4.4), 0x5e6f_7081) * 0.12;
    1. - (p / COAST_STRETCH).length() / COAST_RADIUS + bend
}

fn island_height(p: Vec2) -> f32 {
    let e = land_index(p);
    if e <= -0.3 {
        return SEABED;
    }
    // Every term below is zero at the coast, so land and sea meet at sea level.
    let shore = if e >= 0. {
        3. * smoothstep(0., 0.025, e) + 42. * smoothstep(0.025, 0.4, e)
    } else {
        SEABED * smoothstep(0., 0.3, -e)
    };
    let rolling = (terrain_fbm(p / 280., 0x2c1b_3a49) * 16.
        + terrain_fbm(p / 85., 0x77a1_c3e5) * 2.5)
        * smoothstep(0.03, 0.25, e);
    // Ridges from folded noise, with rounded crests: knife edges are unrealistic and alias
    // badly in coarse terrain levels.
    let fold = terrain_fbm(p / 900. + Vec2::new(5.5, -2.2), 0x3d4e_5f60) * 1.7;
    let ridged = (1.12 - (fold * fold + 0.12 * 0.12).sqrt())
        .clamp(0., 1.)
        .powi(2);
    let massif = (1. - smoothstep(700., 2100., p.distance(MASSIF))) * smoothstep(0.12, 0.5, e);
    let peak = 560. * (-(p - PEAK).length_squared() / (700. * 700.)).exp();
    let hills = (1. - smoothstep(300., 1200., p.distance(HILLS)))
        * 140.
        * (0.6 + 0.4 * ridged)
        * smoothstep(0.1, 0.4, e);
    let ripples = terrain_fbm(p / 60., 0x1357_2468)
        * 0.8
        * smoothstep(0.02, 0.1, -e)
        * (1. - smoothstep(0.2, 0.3, -e));
    shore + rolling + massif * (380. * ridged + peak) + hills + ripples
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

fn document(half_cells: i32) -> ProjectDocument {
    let mut project = road_demo::document();
    let space = project.default_world_space;
    project.world_spaces.retain(|s| s.id == space);
    let world = &mut project.world_spaces[0];
    world.name = "Island".into();
    world.cell_size = DEFAULT_CELL_SIZE;
    world.minimum_y = MINIMUM_HEIGHT;
    world.maximum_y = MAXIMUM_HEIGHT;
    world.sea_level = Some(SEA_LEVEL);
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
    let step = DEFAULT_CELL_SIZE / (HEIGHT_SIDE - 1) as f32;
    let extent = half_cells as f32 * DEFAULT_CELL_SIZE;
    for x in -half_cells..half_cells {
        for z in -half_cells..half_cells {
            let cell = CellCoord { x, z };
            // One sample of halo on each side gives central-difference slopes that agree
            // across cell borders.
            let side = HEIGHT_SIDE + 2;
            let point = |i: usize, j: usize| {
                Vec2::new(
                    x as f32 * DEFAULT_CELL_SIZE + (i as f32 - 1.) * step,
                    z as f32 * DEFAULT_CELL_SIZE + (j as f32 - 1.) * step,
                )
            };
            let halo: Vec<f32> = (0..side)
                .flat_map(|j| (0..side).map(move |i| island_height(point(i, j))))
                .collect();
            let heights: Vec<f32> = (1..=HEIGHT_SIDE)
                .flat_map(|j| (1..=HEIGHT_SIDE).map(move |i| (i, j)))
                .map(|(i, j)| halo[j * side + i])
                .collect();
            project.cells.push(SourceCellRecord {
                space,
                cell,
                height: heights[0],
                source_revision: 1,
            });
            if heights.iter().all(|&h| h == SEABED) {
                continue;
            }
            let mut tiles: Vec<_> = definition
                .layers
                .iter()
                .map(|l| CoverageTile {
                    layer: l.id,
                    samples: Vec::with_capacity(MASK_SIDE * MASK_SIDE),
                })
                .collect();
            for j in 1..=MASK_SIDE {
                for i in 1..=MASK_SIDE {
                    let dx = (halo[j * side + i + 1] - halo[j * side + i - 1]) / (2. * step);
                    let dz = (halo[(j + 1) * side + i] - halo[(j - 1) * side + i]) / (2. * step);
                    let p = point(i, j);
                    let weights = paint(p, halo[j * side + i], dx.hypot(dz));
                    // Paint must match across borders, including cells beyond the world. The
                    // full island's edge is sea; smaller test squares fade out instead.
                    let edge = smoothstep(0., 48., extent - p.abs().max_element());
                    for (tile, weight) in tiles.iter_mut().zip(weights) {
                        tile.samples.push((weight * edge * 255.).round() as u8);
                    }
                }
            }
            project
                .terrain_cell_heightfields
                .push(SourceTerrainCellHeightfieldRecord {
                    space,
                    cell,
                    resolution: HEIGHT_SIDE as u16,
                    heights,
                    source_revision: 1,
                });
            tiles.retain(|t| t.samples.iter().any(|&v| v != 0));
            if !tiles.is_empty() {
                project.environment_cells.push(SourceEnvironmentCellRecord {
                    space,
                    cell,
                    source_revision: 1,
                    definition_revision: definition.revision,
                    tiles,
                });
            }
        }
    }
    project
}

/// The nearest shore from the spawn, found along 72 headings in 4 m steps.
fn nearest_shore() -> Vec2 {
    (0..72)
        .filter_map(|i| {
            let direction = Vec2::from_angle((i as f32 * 5.).to_radians());
            (1..2000)
                .map(|step| direction * step as f32 * 4.)
                .find(|&p| island_height(p) < 1.5)
        })
        .min_by(|a, b| a.length().total_cmp(&b.length()))
        .unwrap()
}

/// Highest point within 800 m of `centre`: a 40 m search, refined at 2 m.
fn highest(centre: Vec2) -> Vec2 {
    let search = |centre: Vec2, spacing: f32| {
        (0..=40)
            .flat_map(|j| {
                (0..=40).map(move |i| centre + Vec2::new(i as f32 - 20., j as f32 - 20.) * spacing)
            })
            .max_by(|a, b| island_height(*a).total_cmp(&island_height(*b)))
            .unwrap()
    };
    search(search(centre, 40.), 2.)
}

/// The heading from `p` whose terrain rises least above eye level within 1.5 km: the most
/// open view.
fn open_heading(p: Vec2) -> Vec2 {
    let eye = island_height(p) + 2.;
    (0..72)
        .map(|i| Vec2::from_angle((i as f32 * 5.).to_radians()))
        .min_by(|a, b| {
            let rise = |direction: Vec2| {
                (1..=75)
                    .map(|step| step as f32 * 20.)
                    .map(|d| (island_height(p + direction * d) - eye) / d)
                    .fold(f32::NEG_INFINITY, f32::max)
            };
            rise(*a).total_cmp(&rise(*b))
        })
        .unwrap()
}

/// Camera yaw that looks along `direction` from behind the player.
fn facing(direction: Vec2) -> f32 {
    (-direction.x).atan2(-direction.y).to_degrees()
}

fn viewpoints() -> Vec<(&'static str, WorldViewBookmark)> {
    let at = |p: Vec2, look: Vec2, pitch_degrees: f32, distance: f32| WorldViewBookmark {
        position: [p.x, island_height(p), p.y],
        yaw_degrees: facing(look),
        pitch_degrees,
        distance,
        fog_visibility: VIEW_VISIBILITY,
        route: Vec::new(),
    };
    let shore = nearest_shore();
    let inland = shore - shore.normalize() * 60.;
    let summit = highest(PEAK);
    let hills = highest(HILLS);
    // Low pitches keep the horizon in frame: these views are about distance.
    vec![
        ("spawn", at(Vec2::ZERO, open_heading(Vec2::ZERO), 8., 9.7)),
        ("beach", at(inland, shore, 8., 9.7)),
        ("summit", at(summit, -summit, 6., 14.)),
        ("hills", at(hills, summit - hills, 6., 14.)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_fits_its_sea_and_meets_it_at_sea_level() {
        let extent = HALF_CELLS as f32 * DEFAULT_CELL_SIZE;
        // The square's edge is under water, so the coast never touches the unbuilt world.
        for i in 0..=512 {
            let t = i as f32 / 512. * 2. - 1.;
            for p in [
                Vec2::new(t * extent, -extent),
                Vec2::new(t * extent, extent),
                Vec2::new(-extent, t * extent),
                Vec2::new(extent, t * extent),
            ] {
                assert!(island_height(p) < SEA_LEVEL - 2., "{p}");
            }
        }
        assert_eq!(island_height(Vec2::splat(-extent)), SEABED);
        let shore = nearest_shore();
        assert!(island_height(shore) < 1.5 && island_height(shore) > -1.);
        assert!(island_height(Vec2::ZERO) > 20.);
        let summit = viewpoints()[2].1.position[1];
        assert!(summit > 700. && summit < MAXIMUM_HEIGHT);
        for (_, view) in viewpoints() {
            view.validate().unwrap();
        }
    }

    #[test]
    fn island_cells_share_edges_skip_deep_sea_and_cook() {
        let project = document(1);
        assert_eq!(project.cells.len(), 4);
        assert_eq!(project.terrain_cell_heightfields.len(), 4);
        assert!(!project.environment_cells.is_empty());
        let field = |x, z| {
            project
                .terrain_cell_heightfields
                .iter()
                .find(|h| h.cell == CellCoord { x, z })
                .unwrap()
        };
        let n = HEIGHT_SIDE;
        for j in 0..n {
            assert_eq!(
                field(-1, 0).heights[j * n + n - 1],
                field(0, 0).heights[j * n]
            );
        }
        assert_eq!(project.world_spaces[0].sea_level, Some(SEA_LEVEL));
        let build = build_runtime(project).unwrap();
        assert!(
            build
                .pages
                .iter()
                .any(|p| p.key.domain == PageDomain::Vegetation)
        );
    }
}
