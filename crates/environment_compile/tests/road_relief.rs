#[path = "support/roads.rs"]
mod road_support;
mod support;
use environment::roads::*;
use road_support::*;
use support::*;
use world::{CellCoord, TerrainHeightfield};
use yarra_environment_compile::{CompilePlan, TerrainSource, TerrainSourceCell};

fn terrain(cell: CellCoord, resolution: u16) -> TerrainSource {
    let loaded_cells = (-1..=1)
        .flat_map(|x| {
            (-1..=1).map(move |z| CellCoord {
                x: cell.x + x,
                z: cell.z + z,
            })
        })
        .collect::<Vec<_>>();
    TerrainSource {
        space: world::WorldSpaceId(1),
        cell_size: 8.0,
        minimum_height: -8.0,
        maximum_height: 8.0,
        cells: loaded_cells
            .iter()
            .map(|&cell| TerrainSourceCell {
                flat: false,
                cell,
                revision: 1,
                resolution,
                heights: vec![0.0; usize::from(resolution).pow(2)],
            })
            .collect(),
        loaded_cells,
    }
}
fn setup() -> (CompilePlan, environment::CoverageSnapshot, RoadSnapshot) {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    r.profiles[0].relief.road_depth = 0.15;
    r.profiles[0].relief.track_depth = 0.08;
    (
        CompilePlan::new(&d, &p, &l, Default::default()).unwrap(),
        c,
        r,
    )
}
fn compile(
    plan: &CompilePlan,
    c: &environment::CoverageSnapshot,
    r: &RoadSnapshot,
    cell: CellCoord,
) -> yarra_environment_compile::CompiledCell {
    plan.compile_cell_with_terrain(cell, c, r, &terrain(cell, 33), Default::default())
        .unwrap()
}
fn max_delta(a: &TerrainHeightfield, b: &TerrainHeightfield) -> f32 {
    (0..usize::from(a.resolution).pow(2))
        .map(|i| {
            (a.height_at(i % usize::from(a.resolution), i / usize::from(a.resolution))
                - b.height_at(i % usize::from(b.resolution), i / usize::from(b.resolution)))
            .abs()
        })
        .fold(0.0, f32::max)
}
#[test]
fn depths_combine_without_changing_wear_and_zero_depth_restores_the_source() {
    let (plan, c, mut r) = setup();
    let rel = &mut r.profiles[0].relief;
    rel.road_variation = 0.0;
    rel.track_variation = 0.0;
    let shaped = compile(&plan, &c, &r, CellCoord::ZERO);
    let h = shaped.terrain.as_ref().unwrap();
    assert!((h.height_at(16, 16) + 0.15).abs() < 0.001);
    assert!((h.height_at(16, 13) + 0.23).abs() < 0.002);
    assert!(h.height_at(16, 0).abs() < 0.001);
    r.profiles[0].relief.road_depth = 0.0;
    let ruts = compile(&plan, &c, &r, CellCoord::ZERO);
    assert!(ruts.terrain.as_ref().unwrap().height_at(16, 16).abs() < 0.001);
    assert!((ruts.terrain.as_ref().unwrap().height_at(16, 13) + 0.08).abs() < 0.002);
    r.profiles[0].relief.track_depth = 0.0;
    let flat = compile(&plan, &c, &r, CellCoord::ZERO);
    assert_eq!(shaped.ground, flat.ground);
    assert_eq!(shaped.vegetation, flat.vegetation);
    assert!(
        flat.terrain
            .as_ref()
            .unwrap()
            .heights
            .windows(2)
            .all(|v| v[0] == v[1])
    );
    assert_eq!(flat, compile(&plan, &c, &r, CellCoord::ZERO));
}
#[test]
fn seeded_variation_is_bounded_deterministic_and_survives_a_knot_split() {
    let (plan, c, mut r) = setup();
    let a = compile(&plan, &c, &r, CellCoord::ZERO);
    assert_eq!(a, compile(&plan, &c, &r, CellCoord::ZERO));
    let h = a.terrain.as_ref().unwrap();
    let center = (0..33).map(|x| h.height_at(x, 16)).collect::<Vec<_>>();
    assert!(
        center.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - center.iter().copied().fold(f32::INFINITY, f32::min)
            > 0.003
    );
    assert!(center.iter().all(|h| (-0.181..=-0.119).contains(h)));
    assert!((0..33).any(|x| (h.height_at(x, 13) - h.height_at(x, 19)).abs() > 0.001));
    let (left, right) = split_span(
        &r.spans[0],
        8.0,
        0.37,
        RoadKnotId([8; 16]),
        RoadSpanId([8; 16]),
    )
    .unwrap();
    r.spans = vec![left, right];
    let split = compile(&plan, &c, &r, CellCoord::ZERO);
    assert!(max_delta(h, split.terrain.as_ref().unwrap()) < 0.0003);
    r.roads[0].seed += 1;
    assert!(
        max_delta(
            h,
            compile(&plan, &c, &r, CellCoord::ZERO)
                .terrain
                .as_ref()
                .unwrap()
        ) > 0.003
    );
}
#[test]
fn independently_compiled_borders_share_heights_and_normals_on_curved_roads() {
    let (d, l, p, c, mut r) = cart_tracks();
    r.profiles[0].relief.road_depth = 0.15;
    r.profiles[0].relief.track_depth = 0.08;
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let a = compile(&plan, &c, &r, CELLS[0]).terrain.unwrap();
    let b = compile(&plan, &c, &r, CELLS[1]).terrain.unwrap();
    for z in 0..33 {
        assert_eq!(a.heights[z * 33 + 32], b.heights[z * 33]);
        assert_eq!(a.normals_oct[z * 33 + 32], b.normals_oct[z * 33]);
    }
}
#[test]
fn relief_ignores_grass_retention_and_cosmetic_breakup_and_does_not_stack_overlaps() {
    let (plan, c, mut r) = setup();
    let original = compile(&plan, &c, &r, CELLS[0]).terrain.unwrap();
    let p = &mut r.profiles[0];
    p.breakup = 1.0;
    p.track_retention = 1.0;
    p.center_retention = 0.0;
    p.edge_variation = 0.12;
    assert_eq!(original, compile(&plan, &c, &r, CELLS[0]).terrain.unwrap());
    let mut other = r.roads[0].clone();
    other.id = RoadId([9; 16]);
    let mut span = r.spans[0].clone();
    span.id = RoadSpanId([9; 16]);
    span.road = other.id;
    span.start.id = RoadKnotId([10; 16]);
    span.end.id = RoadKnotId([11; 16]);
    // Parallel roads with overlapping shoulders; do not invent a junction.
    span.start.position.local[1] += 3.0;
    span.end.position.local[1] += 3.0;
    r.roads.push(other);
    r.spans.push(span);
    let overlapped = compile(&plan, &c, &r, CELLS[0]).terrain.unwrap();
    assert!((0..33).all(|x| overlapped.height_at(x, 22) > -0.20));
}
#[test]
fn invalid_height_halos_unresolved_ruts_and_out_of_range_depths_fail_explicitly() {
    let (plan, c, r) = setup();
    let check =
        |t: &TerrainSource| plan.compile_cell_with_terrain(CELLS[0], &c, &r, t, Default::default());
    let mut t = terrain(CELLS[0], 33);
    t.loaded_cells.pop();
    assert!(check(&t).is_err());
    let mut t = terrain(CELLS[0], 33);
    t.cells[0].heights[0] = f32::NAN;
    assert!(check(&t).is_err());
    let mut t = terrain(CELLS[0], 33);
    t.cells[4].heights[0] = 0.2;
    assert!(check(&t).is_err());
    assert!(
        check(&terrain(CELLS[0], 17))
            .unwrap_err()
            .to_string()
            .contains("two terrain height intervals")
    );
    let mut t = terrain(CELLS[0], 33);
    t.minimum_height = -0.05;
    assert!(check(&t).unwrap_err().to_string().contains("height bounds"));
}

#[test]
fn flat_source_cells_keep_four_vertices_until_relief_needs_a_mesh() {
    let (plan, c, mut r) = setup();
    let mut t = terrain(CELLS[0], 33);
    for page in &mut t.cells {
        page.flat = true;
    }
    let shaped = plan
        .compile_cell_with_terrain(CELLS[0], &c, &r, &t, Default::default())
        .unwrap();
    assert_eq!(shaped.terrain.unwrap().resolution, 33);
    r.profiles[0].relief.road_depth = 0.0;
    r.profiles[0].relief.track_depth = 0.0;
    let flat = plan
        .compile_cell_with_terrain(CELLS[0], &c, &r, &t, Default::default())
        .unwrap();
    assert_eq!(flat.terrain.unwrap().resolution, 2);
}
