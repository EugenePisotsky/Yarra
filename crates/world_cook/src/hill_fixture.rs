//! Repeatable, editable hill/valley landscape with authored meadow coverage and roads.
//! Independent database: creating this fixture never replaces the user's normal world.
use super::*;
use environment::{CoverageTile, roads::*};
use world::WorldViewBookmark;
use world_db::{
    RoadSourceRecord as R, SourceEnvironmentCellRecord, SourceRoad, SourceRoadKnot, SourceRoadSpan,
};

const HALF_CELLS: i32 = 24; // 1,536 m square, using the existing 32 m source grid.
const HEIGHT_SIDE: u16 = 33; // 1 m geometry; ground/vegetation masks retain 0.5 m detail.
const SUMMIT: f32 = 192.;
const ROAD_POINTS: [[f32; 2]; 10] = [
    [5., 3.],
    [-24., -14.],
    [-75., 15.],
    [-140., 65.],
    [-205., 25.],
    [-230., -75.],
    [-165., -150.],
    [-235., -220.],
    [-345., -260.],
    [-520., -190.],
];

pub fn create_hill_fixture(path: &Path) -> Result<()> {
    let views = path.with_extension("views");
    if path.exists() || views.exists() {
        bail!(
            "hill fixture needs a new project and viewpoint directory: {}",
            path.display()
        );
    }
    let project = document(HALF_CELLS);
    write_project_database(path, &project)?;
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

fn hill_height(p: Vec2) -> f32 {
    let gaussian = |center: Vec2, scale: Vec2, height| {
        let q = (p - center) / scale;
        height * (-q.length_squared()).exp()
    };
    let main = gaussian(Vec2::ZERO, Vec2::new(175., 205.), 180.);
    let ridge = gaussian(Vec2::new(-505., -465.), Vec2::new(110., 220.), 215.)
        + gaussian(Vec2::new(-300., -570.), Vec2::new(170., 90.), 95.);
    let rolling = terrain_fbm(p / 115. + Vec2::splat(5.), 0x591c_e328) * 9.
        + terrain_fbm(p / 38., 0x89ab_3097) * 1.2;
    let landscape = 12. + main + ridge + rolling;
    // A small rounded crown gives the spawn a stable lookout without a hard plateau edge.
    let blend = smoothstep(8., 28., p.length());
    SUMMIT + (landscape - SUMMIT) * blend
}

fn road_knots() -> Vec<RoadKnot> {
    ROAD_POINTS
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let before = Vec2::from_array(ROAD_POINTS[i.saturating_sub(1)]);
            let after = Vec2::from_array(ROAD_POINTS[(i + 1).min(ROAD_POINTS.len() - 1)]);
            let tangent = (after - before) / 6.;
            RoadKnot {
                id: RoadKnotId(stable_id(&format!("hill-road-knot-{i}"))),
                revision: 1,
                position: RoadPoint::from_relative(
                    CellCoord::ZERO,
                    p.map(f64::from),
                    DEFAULT_CELL_SIZE as f64,
                )
                .unwrap(),
                incoming: (-tangent).to_array().map(f64::from),
                outgoing: tangent.to_array().map(f64::from),
                width: 4.2,
            }
        })
        .collect()
}

fn route() -> Vec<[f32; 3]> {
    let knots = road_knots();
    let mut route = Vec::new();
    // Start at the hilltop lookout (the second knot); the first road segment
    // still reaches the crown behind it but isn't part of the descent.
    for pair in knots.windows(2).skip(1) {
        let a = Vec2::from_array(
            pair[0]
                .position
                .relative_to(CellCoord::ZERO, DEFAULT_CELL_SIZE as f64)
                .map(|v| v as f32),
        );
        let d = Vec2::from_array(
            pair[1]
                .position
                .relative_to(CellCoord::ZERO, DEFAULT_CELL_SIZE as f64)
                .map(|v| v as f32),
        );
        let b = a + Vec2::from_array(pair[0].outgoing.map(|v| v as f32));
        let c = d + Vec2::from_array(pair[1].incoming.map(|v| v as f32));
        // Frequent waypoints follow the actual cubic road rather than cutting its corners.
        let steps = ((a.distance(b) + b.distance(c) + c.distance(d)) / 2.).ceil() as usize;
        for step in 0..steps {
            let t = step as f32 / steps as f32;
            let s = 1. - t;
            let p = a * s.powi(3) + b * (3. * s * s * t) + c * (3. * s * t * t) + d * t.powi(3);
            route.push([p.x, hill_height(p), p.y]);
        }
    }
    let p = Vec2::from_array(*ROAD_POINTS.last().unwrap());
    route.push([p.x, hill_height(p), p.y]);
    route
}

fn viewpoints() -> Vec<(&'static str, WorldViewBookmark)> {
    let lookout = Vec2::from_array(ROAD_POINTS[1]);
    let summit = WorldViewBookmark {
        position: [lookout.x, hill_height(lookout), lookout.y],
        yaw_degrees: 45.,
        pitch_degrees: 35.,
        distance: 17.6,
        fog_visibility: 2500.,
        route: route(),
    };
    let at = |point: [f32; 2]| WorldViewBookmark {
        position: [point[0], hill_height(Vec2::from_array(point)), point[1]],
        pitch_degrees: 18.,
        distance: 9.7,
        route: Vec::new(),
        ..summit.clone()
    };
    vec![
        ("summit", summit.clone()),
        ("slope", at([-145., 62.])),
        ("valley", at([-345., -260.])),
    ]
}

fn document(half_cells: i32) -> ProjectDocument {
    let mut project = road_demo::document();
    let space = project.default_world_space;
    project.world_spaces.retain(|s| s.id == space);
    let world = &mut project.world_spaces[0];
    world.name = "Hill and valley".into();
    world.cell_size = DEFAULT_CELL_SIZE;
    world.minimum_y = -8.;
    world.maximum_y = 350.;
    project.environments.retain(|d| d.space == space);
    let definition = &mut project.environments[0];
    definition.cell_size = DEFAULT_CELL_SIZE;
    definition.mask_resolution = 65;
    project.terrain_profiles.retain(|p| p.space == space);
    project.cells.clear();
    project.terrain_cell_heightfields.clear();
    project.environment_cells.clear();
    project.objects.clear();
    let extent = half_cells as f32 * DEFAULT_CELL_SIZE;
    for x in -half_cells..half_cells {
        for z in -half_cells..half_cells {
            let cell = CellCoord { x, z };
            let point = |i: usize, j: usize, side: usize| {
                Vec2::new(
                    x as f32 * DEFAULT_CELL_SIZE + i as f32 * DEFAULT_CELL_SIZE / (side - 1) as f32,
                    z as f32 * DEFAULT_CELL_SIZE + j as f32 * DEFAULT_CELL_SIZE / (side - 1) as f32,
                )
            };
            project.cells.push(SourceCellRecord {
                space,
                cell,
                height: hill_height(point(0, 0, 2)),
                source_revision: 1,
            });
            let n = HEIGHT_SIDE as usize;
            let heights = (0..n)
                .flat_map(|j| (0..n).map(move |i| hill_height(point(i, j, n))))
                .collect();
            project
                .terrain_cell_heightfields
                .push(SourceTerrainCellHeightfieldRecord {
                    space,
                    cell,
                    resolution: HEIGHT_SIDE,
                    heights,
                    source_revision: 1,
                });
            let mut tiles: Vec<_> = definition
                .layers
                .iter()
                .map(|l| CoverageTile {
                    layer: l.id,
                    samples: Vec::with_capacity(65 * 65),
                })
                .collect();
            for j in 0..65 {
                for i in 0..65 {
                    let p = point(i, j, 65);
                    let edge = smoothstep(0., 48., extent - p.abs().max_element());
                    let valley = 1. - smoothstep(100., 340., p.distance(Vec2::new(-260., -220.)));
                    let fields =
                        (0.5 + 0.3 * (p.x / 95. + (p.y / 135.).sin()).sin() + valley * 0.35)
                            .clamp(0., 1.);
                    let clearing = (1. - smoothstep(22., 45., p.distance(Vec2::new(-315., -215.))))
                        .max(1. - smoothstep(12., 25., p.distance(Vec2::new(-125., 60.))));
                    for (tile, weight) in
                        tiles.iter_mut().zip([edge, fields * edge, clearing * edge])
                    {
                        tile.samples.push((weight * 255.).round() as u8);
                    }
                }
            }
            tiles.retain(|t| t.samples.iter().any(|&v| v != 0));
            project.environment_cells.push(SourceEnvironmentCellRecord {
                space,
                cell,
                source_revision: 1,
                definition_revision: definition.revision,
                tiles,
            });
        }
    }
    let mut profile = project
        .roads
        .records
        .iter()
        .find_map(|r| {
            if let R::Profile(p) = r {
                Some(p.clone())
            } else {
                None
            }
        })
        .unwrap();
    profile.name = "Hill cart track".into();
    // The larger source cells resolve 0.5 m ground samples: use broad worn
    // wheel paths instead of undersampling the demo's narrow ruts.
    profile.track_width = 1.2;
    profile.edge_softness = 0.25;
    profile.breakup_patch_size = 2.4;
    profile.breakup = 0.28;
    profile.center_ground = 0.12;
    profile.shoulder_ground = 0.45;
    profile.relief.road_depth = 0.08;
    profile.relief.shoulder_falloff = 1.0;
    // Narrow wheel ruts need finer geometry; this distance fixture keeps wheel
    // wear in the surface/grass masks and only grades the whole corridor.
    profile.relief.track_depth = 0.0;
    let road = Road {
        id: RoadId(stable_id("hill-descent-road")),
        revision: 1,
        name: "Summit to valley".into(),
        profile: profile.id,
        seed: 917,
        enabled: true,
        order: 0,
        direction: TravelDirection::Bidirectional,
        travel_modes: vec![
            RoadTravelMode::Foot,
            RoadTravelMode::Mounted,
            RoadTravelMode::Cart,
        ],
    };
    project.roads.records = vec![
        R::Profile(profile),
        R::Road(SourceRoad {
            space,
            road: road.clone(),
        }),
    ];
    let knots = road_knots();
    project.roads.records.extend(knots.iter().map(|k| {
        R::Knot(SourceRoadKnot {
            road: road.id,
            knot: k.clone(),
        })
    }));
    project
        .roads
        .records
        .extend(knots.windows(2).enumerate().map(|(i, k)| {
            R::Span(SourceRoadSpan {
                id: RoadSpanId(stable_id(&format!("hill-road-span-{i}"))),
                revision: 1,
                road: road.id,
                start: k[0].id,
                end: k[1].id,
            })
        }));
    project
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hill_has_a_shared_summit_and_a_continuous_road_to_the_valley() {
        let views = viewpoints();
        let summit = &views[0].1;
        for (_, view) in &views {
            view.validate().unwrap();
        }
        assert_eq!(hill_height(Vec2::ZERO), SUMMIT);
        assert_eq!(
            summit.position[1],
            hill_height(Vec2::from_array(ROAD_POINTS[1]))
        );
        assert!(
            SUMMIT - summit.position[1] < 5.,
            "spawn stays at the hilltop"
        );
        assert!(summit.position[1] - views[2].1.position[1] > 140.);
        assert!(hill_height(Vec2::new(-505., -465.)) > 200.);
        for pair in summit.route.windows(2) {
            let a = glam::Vec3::from_array(pair[0]);
            let b = glam::Vec3::from_array(pair[1]);
            assert!(a.distance(b) < 4.);
            assert!((a.y - hill_height(Vec2::new(a.x, a.z))).abs() < 0.001);
        }
        // Adjacent source pages evaluate exactly the same endpoint coordinates.
        let project = document(1);
        let left = project
            .terrain_cell_heightfields
            .iter()
            .find(|h| h.cell == CellCoord { x: -1, z: 0 })
            .unwrap();
        let right = project
            .terrain_cell_heightfields
            .iter()
            .find(|h| h.cell == CellCoord::ZERO)
            .unwrap();
        let n = HEIGHT_SIDE as usize;
        for j in 0..n {
            assert_eq!(left.heights[j * n + n - 1], right.heights[j * n]);
        }
        let build = build_runtime(project).unwrap();
        assert!(
            build
                .pages
                .iter()
                .any(|p| p.key.domain == PageDomain::Vegetation)
        );
    }
}
