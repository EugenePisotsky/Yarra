#[path = "support/roads.rs"]
mod road_support;

mod support;

use environment::roads::*;
use environment::{TerrainSource, TerrainSourceCell};
use road_support::*;
use support::*;
use world::CellCoord;
use yarra_environment_compile::CompilePlan;

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
    let center = worn_weights(&a, 64, 32);
    assert_eq!(center.iter().find(|(s, _)| *s == SOIL).unwrap().1, 255);

    // Worn bare there, the hub clears nearly all grass.
    for f in &a.vegetation.fields {
        let v = f.coverage[31 * usize::from(f.resolution) + 63];
        assert!(v < 26, "{v}");
    }

    // Outside the junction grass still grows down the road's middle.
    let middle = a
        .vegetation
        .fields
        .iter()
        .map(|f| f.coverage[31 * usize::from(f.resolution) + 8])
        .max()
        .unwrap();
    assert!(middle > 100, "{middle}");
    let mut plain = r.clone();
    plain.junctions.clear();
    plain.spans.retain(|s| s.road == plain.roads[0].id);
    plain.roads.truncate(1);
    let before = compile(&plan, &c, &plain, CELLS[0]);

    // Away from the corner legs (the page's roads after the two that meet).
    let mut compared = 0;

    for z in 0..33 {
        let local = [1.0, z as f64 * 0.25];
        let near_leg = (2..a.roads.roads.len()).any(|leg| {
            a.roads
                .nearest_on(leg, local)
                .is_some_and(|hit| hit.offset.distance < hit.offset.width * 0.5 + 2.0)
        });

        if near_leg {
            continue;
        }

        compared += 1;
        assert_eq!(
            h.height_at(4, z),
            before.terrain.as_ref().unwrap().height_at(4, z)
        );
    }

    assert!(compared >= 5, "{compared} samples away from the legs");

    // Roads and junctions only wear the meadow away. The worn ground they paint grows its own
    // grass, no more than the corridor retains.
    let mut none = r.clone();
    none.junctions.clear();
    none.spans.clear();
    none.roads.clear();
    let bare = compile(&plan, &c, &none, CELLS[0]);

    for field in &a.vegetation.fields {
        match bare
            .vegetation
            .fields
            .iter()
            .find(|f| f.grass == field.grass)
        {
            Some(without) => assert!(
                field
                    .coverage
                    .iter()
                    .zip(&without.coverage)
                    .all(|(with, without)| with <= without)
            ),
            None => assert!(field.coverage.iter().all(|&v| v <= 141)),
        }
    }
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
        worn_weights(&cell, 27, 43)
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

#[test]
fn page_roads_reproduce_a_junction_for_near_detail() {
    let (plan, c, r) = setup();
    let mut hub = 0;

    for cell in CELLS {
        let compiled = compile(&plan, &c, &r, cell);
        let roads = &compiled.roads;

        // Both roads, and the corner legs between them.
        assert!(roads.roads.len() >= 2, "both roads draw on {cell:?}");
        assert_eq!(roads.junctions.len(), 1);
        assert_eq!(roads.junctions[0].members, 0b11);
        let n = usize::from(compiled.ground.weight_pages[0].resolution);
        let origin = [f64::from(cell.x) * 8.0, f64::from(cell.z) * 8.0];

        for z in 0..n {
            for x in 0..n {
                let local = [x, z].map(|v| v as f64 * 8.0 / (n - 1) as f64);
                let laid = roads.ground(origin, local);
                let soil = worn_weights(&compiled, x, z)
                    .iter()
                    .find(|(s, _)| *s == SOIL)
                    .map_or(0, |w| w.1);
                assert!(
                    (laid * 255.0 - f64::from(soil)).abs() <= 1.0,
                    "{cell:?} at {local:?}: page roads lay {laid}, the compiler laid {soil}"
                );
                let layers = roads.layers(origin, local);
                hub += usize::from(layers[2] > 0.5);
            }
        }
    }

    assert!(hub > 20, "the junction wore {hub} samples");
}

#[test]
fn a_footpath_of_its_own_style_branches_from_a_cart_road() {
    let (plan, c, mut r) = setup();
    let mut path = r.profiles[0].clone();
    path.id = RoadProfileId([9; 16]);
    path.name = "Footpath".into();
    path.track_spacing = 0.0;
    path.track_width = 0.6;
    path.track_wander = 0.1;
    r.profiles.push(path.clone());
    r.roads[1].profile = path.id;

    for s in r.spans.iter_mut().filter(|s| s.road == r.roads[1].id) {
        s.start.width = 1.6;
        s.end.width = 1.6;
    }

    let cell = compile(&plan, &c, &r, CELLS[0]);
    let roads = &cell.roads;
    assert_eq!(roads.roads[1].shape.track_spacing, 0.0);

    // Its corner legs to the cart road take the quieter footpath's style.
    assert!(roads.roads.len() > 2);
    assert!(
        roads.roads[2..]
            .iter()
            .all(|r| r.shape.track_spacing == 0.0)
    );

    // One track: both wander as one.
    let tracks = roads.roads[1].shape.tracks([3.0, 5.0]);
    assert_eq!(tracks[0], tracks[1]);

    // The junction's patch takes the style of one of its roads.
    r.junctions[0].profile = RoadProfileId([8; 16]);
    r.profiles.push(environment::roads::CartTrackProfile {
        id: RoadProfileId([8; 16]),
        ..path
    });
    assert!(r.validate().is_err());
}

#[test]
fn corner_legs_cut_the_corners_traffic_turns_leaving_a_grass_island() {
    let (plan, c, r) = setup();

    // A T: the branch leaves the straight road at right angles, so both corners are cut.
    let cells: Vec<_> = CELLS
        .iter()
        .map(|&cell| compile(&plan, &c, &r, cell))
        .collect();
    let legs = cells[0].roads.roads.len() - 2;
    assert_eq!(legs, 2, "one leg per corner");

    // The legs are narrower and wear less than the roads they join.
    let main = &cells[0].roads.roads[0];
    assert!(
        cells[0].roads.roads[2..]
            .iter()
            .all(|l| l.strength < main.strength)
    );

    // Between a leg and the junction the meadow stays (an island); on the leg it is worn.
    let roads = &cells[0].roads;
    let origin = [0.0, 0.0];
    let island = roads.ground(origin, [6.0, 6.5]);
    let on_leg = (2..roads.roads.len())
        .filter_map(|leg| roads.nearest_on(leg, [6.0, 6.5]))
        .map(|hit| roads.ground(origin, hit.point))
        .fold(0.0, f64::max);
    assert!(on_leg > 0.4, "the leg wears {on_leg}");
    assert!(
        island < on_leg,
        "island {island} against the leg's {on_leg}"
    );

    // Corners too sharp or too straight are not cut: two roads meeting end to end keep none.
    let mut straight = r.clone();
    straight.spans.retain(|s| s.road == straight.roads[0].id);
    straight.roads.truncate(1);
    straight.junctions[0]
        .knots
        .retain(|k| *k == RoadKnotId([3; 16]));
    straight.junctions.clear();
    let plain = compile(&plan, &c, &straight, CELLS[0]);
    assert_eq!(plain.roads.roads.len(), 1);
}
