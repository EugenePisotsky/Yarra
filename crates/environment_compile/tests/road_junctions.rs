#[path = "support/roads.rs"]
mod road_support;
mod support;
use environment::roads::*;
use road_support::*;
use support::*;
use world::CellCoord;
use yarra_environment_compile::{CompilePlan, TerrainSource, TerrainSourceCell};
fn setup() -> (CompilePlan, environment::CoverageSnapshot, RoadSnapshot) {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    let (a, b) = split_span(
        &r.spans[0],
        8.0,
        0.5,
        RoadKnotId([3; 16]),
        RoadSpanId([2; 16]),
    )
    .unwrap();
    let position = a.end.position;
    let mut road = r.roads[0].clone();
    road.id = RoadId([2; 16]);
    road.seed += 17;
    let mut start = a.end.clone();
    start.id = RoadKnotId([4; 16]);
    start.incoming = [0.0; 2];
    start.outgoing = [0.0, 10.0 / 3.0];
    let mut end = start.clone();
    end.id = RoadKnotId([5; 16]);
    end.position = RoadPoint::from_relative(CellCoord::ZERO, [8.0, 14.0], 8.0).unwrap();
    end.incoming = [0.0, -10.0 / 3.0];
    end.outgoing = [0.0; 2];
    r.spans = vec![
        a,
        b,
        RoadSpan {
            id: RoadSpanId([3; 16]),
            revision: 1,
            road: road.id,
            start,
            end,
        },
    ];
    r.roads.push(road);
    r.junctions = vec![RoadJunction {
        id: RoadJunctionId([1; 16]),
        revision: 1,
        position,
        radius: 3.5,
        profile: r.profiles[0].id,
        seed: 700,
        knots: vec![RoadKnotId([3; 16]), RoadKnotId([4; 16])],
    }];
    r.profiles[0].relief.road_depth = 0.15;
    r.profiles[0].relief.track_depth = 0.08;
    r.profiles[0].relief.road_variation = 0.0;
    r.profiles[0].relief.track_variation = 0.0;
    (
        CompilePlan::new(&d, &p, &l, Default::default()).unwrap(),
        c,
        r,
    )
}
fn terrain(cell: CellCoord) -> TerrainSource {
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
            .map(|cell| TerrainSourceCell {
                cell: *cell,
                flat: false,
                revision: 1,
                resolution: 33,
                heights: vec![0.0; 33 * 33],
            })
            .collect(),
        loaded_cells,
    }
}
fn compile(
    plan: &CompilePlan,
    c: &environment::CoverageSnapshot,
    r: &RoadSnapshot,
    cell: CellCoord,
) -> yarra_environment_compile::CompiledCell {
    let mut r = r.clone();
    r.loaded_bounds = RoadCellBounds {
        minimum: CellCoord {
            x: cell.x - 1,
            z: cell.z - 1,
        },
        maximum: CellCoord {
            x: cell.x + 1,
            z: cell.z + 1,
        },
    };
    plan.compile_cell_with_terrain(cell, c, &r, &terrain(cell), Default::default())
        .unwrap()
}
#[test]
fn t_junction_wears_the_center_and_blends_ruts_without_adding_grass_or_excavating_twice() {
    let (plan, c, r) = setup();
    let a = compile(&plan, &c, &r, CELLS[0]);
    let h = a.terrain.as_ref().unwrap();
    assert!((h.height_at(32, 16) + 0.19).abs() < 0.001);
    assert!((0..33).all(|x| (0..33).all(|z| h.height_at(x, z) > -0.231)));
    let center = ground_weights(&a.ground, 64, 32);
    assert_eq!(center.iter().find(|(s, _)| *s == SOIL).unwrap().1, 255);
    assert!(
        a.vegetation
            .fields
            .iter()
            .all(|f| f.coverage[31 * usize::from(f.resolution) + 63] == 0)
    );
    // The painted meadow and straight road outside the junction keep their original behavior.
    assert!(
        a.vegetation
            .fields
            .iter()
            .any(|f| f.coverage[31 * usize::from(f.resolution) + 8] == 255)
    );
    let mut plain = r.clone();
    plain.junctions.clear();
    plain.spans.retain(|s| s.road == plain.roads[0].id);
    plain.roads.truncate(1);
    let before = compile(&plan, &c, &plain, CELLS[0]);
    for z in 0..33 {
        assert_eq!(
            h.height_at(4, z),
            before.terrain.as_ref().unwrap().height_at(4, z)
        );
    }
    let mut empty = c.clone();
    for cell in &mut empty.cells {
        for tile in &mut cell.tiles {
            tile.samples.fill(0);
        }
    }
    let empty = compile(&plan, &empty, &r, CELLS[0]);
    assert!(
        empty
            .vegetation
            .fields
            .iter()
            .all(|f| f.coverage.iter().all(|v| *v == 0))
    );
}
#[test]
fn independently_compiled_junction_borders_and_reordered_inputs_match() {
    let (plan, c, mut r) = setup();
    r.profiles[0].breakup = 0.6;
    r.profiles[0].relief.road_variation = 0.2;
    let a = compile(&plan, &c, &r, CELLS[0]);
    let b = compile(&plan, &c, &r, CELLS[1]);
    for z in 0..33 {
        assert_eq!(
            a.terrain.as_ref().unwrap().heights[z * 33 + 32],
            b.terrain.as_ref().unwrap().heights[z * 33]
        );
        assert_eq!(
            a.terrain.as_ref().unwrap().normals_oct[z * 33 + 32],
            b.terrain.as_ref().unwrap().normals_oct[z * 33]
        );
    }
    for z in 0..=64 {
        assert_eq!(
            ground_weights(&a.ground, 64, z),
            ground_weights(&b.ground, 0, z)
        );
    }
    r.spans.reverse();
    r.roads.reverse();
    r.profiles.reverse();
    assert_eq!(a, compile(&plan, &c, &r, CELLS[0]));
    let mut dry = r.clone();
    dry.profiles[0].breakup = 0.0;
    dry.profiles[0].track_retention = 1.0;
    assert_eq!(a.terrain, compile(&plan, &c, &dry, CELLS[0]).terrain);
}
#[test]
fn four_way_connection_is_explicit_and_preserves_its_center_when_an_arm_is_disabled() {
    let (plan, c, mut r) = setup();
    let mut start = r.spans[2].start.clone();
    start.id = RoadKnotId([6; 16]);
    start.position = RoadPoint::from_relative(CellCoord::ZERO, [8.0, -6.0], 8.0).unwrap();
    let mut end = r.spans[2].start.clone();
    end.incoming = [0.0, -10.0 / 3.0];
    r.spans[2].start = end.clone();
    r.spans.push(RoadSpan {
        id: RoadSpanId([4; 16]),
        revision: 1,
        road: r.roads[1].id,
        start,
        end,
    });
    compile(&plan, &c, &r, CELLS[0]);
    let mut disconnected = r.clone();
    disconnected.junctions.clear();
    assert!(
        plan.compile_cells_with_roads(&CELLS, &c, &disconnected, Default::default())
            .is_err()
    );
    r.roads[1].enabled = false;
    let disabled = compile(&plan, &c, &r, CELLS[0]);
    assert!((disabled.terrain.unwrap().height_at(32, 16) + 0.15).abs() < 0.001);
}
#[test]
fn malformed_connections_acute_forks_and_unrelated_crossings_are_rejected() {
    let (plan, c, r) = setup();
    let check = |r: &RoadSnapshot| plan.compile_cells_with_roads(&CELLS, &c, r, Default::default());
    let mut bad = r.clone();
    bad.junctions[0].knots.push(RoadKnotId([99; 16]));
    assert!(check(&bad).is_err());
    let mut bad = r.clone();
    bad.spans[2].start.position.local[0] += 0.5;
    assert!(check(&bad).is_err());
    let mut bad = r.clone();
    bad.junctions[0].radius = 1.0;
    assert!(check(&bad).is_err());
    let mut bad = r.clone();
    bad.spans[2].end.position =
        RoadPoint::from_relative(CellCoord::ZERO, [18.0, 5.0], 8.0).unwrap();
    bad.spans[2].start.outgoing = [10.0 / 3.0, 1.0 / 3.0];
    bad.spans[2].end.incoming = [-10.0 / 3.0, -1.0 / 3.0];
    assert!(check(&bad).unwrap_err().to_string().contains("30 degrees"));
    let mut bad = r.clone();
    bad.junctions.clear();
    assert!(check(&bad).is_err());
}

#[test]
fn overlapping_approach_materials_do_not_jump_at_the_junction_boundary() {
    let (_, c, mut r) = setup();
    let (d, mut library, plants, _, _) = cart_tracks();
    let environment::PresetKind::Ground(ground) = &mut library
        .presets
        .iter_mut()
        .find(|p| p.id == environment::fixtures::DRY_GROUND)
        .unwrap()
        .kind
    else {
        unreachable!()
    };
    ground.strength = 0.5;
    let plan = CompilePlan::new(&d, &plants, &library, Default::default()).unwrap();
    let direction = [
        32.0_f64.to_radians().cos() * 10.0,
        32.0_f64.to_radians().sin() * 10.0,
    ];
    r.spans[2].start.outgoing = direction.map(|v| v / 3.0);
    r.spans[2].end.incoming = direction.map(|v| -v / 3.0);
    r.spans[2].end.position = RoadPoint::from_relative(
        CellCoord::ZERO,
        [8.0 + direction[0], 4.0 + direction[1]],
        8.0,
    )
    .unwrap();
    let radius = 3.375_f32.hypot(1.375);
    let sample = |r: &RoadSnapshot| {
        let cell = plan
            .compile_cells_with_roads(&[CELLS[1]], &c, r, Default::default())
            .unwrap()
            .remove(0);
        ground_weights(&cell.ground, 27, 43)
            .into_iter()
            .find(|(s, _)| *s == SOIL)
            .unwrap()
            .1
    };
    r.junctions[0].radius = radius - 0.001;
    let outside = sample(&r);
    r.junctions[0].radius = radius + 0.001;
    let inside = sample(&r);
    assert!(outside > 0 && outside < 255);
    assert!(
        outside.abs_diff(inside) <= 1,
        "material jumps at junction radius: {outside} vs {inside}"
    );
}
