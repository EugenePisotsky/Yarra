//! Phase 0 stress world, currently the default: a ~28 km² island with a 745 m massif in an
//! 8 km square of sea. Heights and paint are procedural. Deep sea is exactly flat, so those cells
//! store neither heights nor paint.
use super::*;
use terrain_world::{base_document, highest, nearest_shore, open_heading, view};
use world::WorldViewBookmark;

/// 8,192 m square of 32 m cells, so the hierarchy closes into four level-7 roots.
const HALF_CELLS: i32 = 128;
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

pub fn create_island_world(path: &Path) -> Result<()> {
    let views = path.with_extension("views");
    if path.exists() || views.exists() {
        bail!(
            "the island needs a new project and viewpoint directory: {}",
            path.display()
        );
    }
    let views = viewpoints();
    let mut project = document(HALF_CELLS);
    project.start_view = Some(views[0].1.clone());
    write_project_database(path, &project)
        .with_context(|| format!("failed to create the island at {}", path.display()))?;
    terrain_world::write_views(path, &views)
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

fn document(half_cells: i32) -> ProjectDocument {
    let mut project = base_document("Island", [MINIMUM_HEIGHT, MAXIMUM_HEIGHT], SEA_LEVEL);
    let layers = terrain_world::imported_layers(&project.environments[0], &[])
        .expect("the island's layers read generated masks");
    let bounds = Vec2::splat(half_cells as f32 * DEFAULT_CELL_SIZE);
    let cells: Vec<_> = (-half_cells..half_cells)
        .flat_map(|x| (-half_cells..half_cells).map(move |z| CellCoord { x, z }))
        .collect();
    // Paint must match across borders, including cells beyond the world. The full island's
    // edge is sea; smaller test squares fade out instead.
    for cell in parallel::map(&cells, |&cell| {
        terrain_world::terrain_cell(cell, &island_height, &layers, [-bounds, bounds])
    }) {
        terrain_world::push_cell(&mut project, cell);
    }
    project
}

fn viewpoints() -> Vec<(&'static str, WorldViewBookmark)> {
    let shore = nearest_shore(&island_height, Vec2::ZERO).expect("the island has a shore");
    let inland = shore - shore.normalize() * 60.;
    let summit = highest(&island_height, PEAK, 800.);
    let hills = highest(&island_height, HILLS, 800.);
    let spawn = open_heading(&island_height, Vec2::ZERO);
    vec![
        ("spawn", view(&island_height, Vec2::ZERO, spawn, 8., 9.7)),
        ("beach", view(&island_height, inland, shore, 8., 9.7)),
        ("summit", view(&island_height, summit, -summit, 6., 14.)),
        (
            "hills",
            view(&island_height, hills, summit - hills, 6., 14.),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrain_world::HEIGHT_SIDE;

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
        let shore = nearest_shore(&island_height, Vec2::ZERO).unwrap();
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
