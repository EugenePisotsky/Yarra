#[path = "support/roads.rs"]
mod road_support;
mod support;
use environment::roads::*;
use road_support::*;
use support::*;
use world::CellCoord;
use yarra_environment_compile::{
    CompileError, CompilePlan, CompileProfile, CompiledCell, RoadCompileProfile,
};

fn grass(cell: &CompiledCell, x: usize, z: usize) -> u8 {
    cell.vegetation
        .fields
        .iter()
        .map(|f| f.coverage[z * usize::from(f.resolution) + x])
        .max()
        .unwrap_or(0)
}
/// The road's soil at a weight sample, as the page draws it (natural ground worn by its roads).
fn ground_at(cell: &CompiledCell, x: usize, z: usize) -> u8 {
    worn_weights(cell, x, z)
        .iter()
        .find(|(s, _)| *s == SOIL)
        .map_or(0, |(_, v)| *v)
}
fn compile(
    plan: &CompilePlan,
    coverage: &environment::CoverageSnapshot,
    roads: &RoadSnapshot,
) -> Vec<CompiledCell> {
    plan.compile_cells_with_roads(&CELLS, coverage, roads, RoadCompileProfile::default())
        .unwrap()
}
#[test]
fn cart_tracks_keep_grass_down_the_middle_and_outside_and_keep_population_identities() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    let plan = CompilePlan::new(&d, &p, &l, CompileProfile::default()).unwrap();
    let base = plan.compile_cells(&CELLS, &c).unwrap();
    let cells = compile(&plan, &c, &r);
    assert_eq!(
        cells[0]
            .vegetation
            .fields
            .iter()
            .map(|f| f.grass)
            .collect::<Vec<_>>(),
        base[0]
            .vegetation
            .fields
            .iter()
            .map(|f| f.grass)
            .collect::<Vec<_>>()
    );
    // 8 m / 64 vegetation samples. Center at z=4; tracks at 4 ± .775. At medium traffic grass
    // grows down the middle, thinned where it is trampled, and in full outside.
    let middle = grass(&cells[0], 32, 31);
    assert!(middle > 100 && middle < 255, "{middle}");
    assert_eq!(grass(&cells[0], 32, 0), 255);
    assert_eq!(grass(&cells[0], 32, 25), 0);
    assert_eq!(grass(&cells[0], 32, 38), 0);
    assert!(ground_at(&cells[0], 32, 32) < 128);
    assert!(ground_at(&cells[0], 32, 26) > 230);
    let shoulder = grass(&cells[0], 32, 20);
    assert!(shoulder > 0 && shoulder < 255);
    // A wider road changes only shoulders, not wheel spacing or the center.
    r.spans[0].start.width = 5.0;
    r.spans[0].end.width = 5.0;
    let wide = compile(&plan, &c, &r);
    assert_eq!(grass(&wide[0], 32, 25), 0);
    assert_eq!(grass(&wide[0], 32, 31), middle);
    assert!(grass(&wide[0], 32, 17) < grass(&cells[0], 32, 17));
}
#[test]
fn traffic_wears_a_quiet_track_to_trampled_lines_and_a_busy_one_to_a_bare_band() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    let plan = CompilePlan::new(&d, &p, &l, CompileProfile::default()).unwrap();
    let at = |r: &RoadSnapshot, traffic: f32| {
        let mut r = r.clone();
        r.roads[0].traffic = traffic;
        compile(&plan, &c, &r).remove(0)
    };
    let (quiet, medium, busy) = (at(&r, 0.1), at(&r, 0.5), at(&r, 1.0));
    // Tracks: worn partly when quiet, bare from medium traffic on.
    assert!(ground_at(&quiet, 32, 26) < 200);
    assert!(ground_at(&medium, 32, 26) > 230 && ground_at(&busy, 32, 26) > 230);
    // The middle: untouched when quiet, worn bare when busy, grass gone with it.
    assert_eq!(ground_at(&quiet, 32, 32), 0);
    assert_eq!(grass(&quiet, 32, 31), 255);
    assert!(ground_at(&busy, 32, 32) > 230);
    assert!(grass(&busy, 32, 31) < 20, "{}", grass(&busy, 32, 31));
    // Shoulders wear more the busier the road.
    let shoulder = |cell: &CompiledCell| grass(cell, 32, 20);
    assert!(shoulder(&quiet) > shoulder(&medium) && shoulder(&medium) > shoulder(&busy));
}
#[test]
fn curved_road_matches_local_batch_order_and_cell_border_compilation() {
    let (d, l, p, c, r) = cart_tracks();
    let plan = CompilePlan::new(&d, &p, &l, CompileProfile::default()).unwrap();
    let batch = compile(&plan, &c, &r);
    for (i, cell) in CELLS.iter().enumerate() {
        assert_eq!(
            plan.compile_cells_with_roads(&[*cell], &c, &r, Default::default())
                .unwrap()[0],
            batch[i]
        );
    }
    let mut reverse = r.clone();
    reverse.spans.reverse();
    reverse.roads.reverse();
    reverse.profiles.reverse();
    assert_eq!(
        plan.compile_cells_with_roads(&[CELLS[1], CELLS[0]], &c, &reverse, Default::default())
            .unwrap(),
        batch
    );
    for z in 0..=64 {
        assert_eq!(
            ground_weights(&batch[0].ground, 64, z),
            ground_weights(&batch[1].ground, 0, z)
        );
    }
}
#[test]
fn splitting_a_curve_preserves_coverage_within_raster_tolerance_and_never_reseeds_noise() {
    let (d, l, p, c, mut r) = cart_tracks();
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let original = compile(&plan, &c, &r);
    let (left, right) = split_span(
        &r.spans[0],
        d.cell_size,
        0.37,
        RoadKnotId([3; 16]),
        RoadSpanId([2; 16]),
    )
    .unwrap();
    r.spans = vec![right, left];
    let split = compile(&plan, &c, &r);
    for (a, b) in original.iter().zip(&split) {
        assert_eq!(a.ground.surfaces, b.ground.surfaces);
        let ground_delta = a.ground.weight_pages[0]
            .rgba
            .iter()
            .zip(&b.ground.weight_pages[0].rgba)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap();
        let grass_delta = a
            .vegetation
            .fields
            .iter()
            .zip(&b.vegetation.fields)
            .flat_map(|(x, y)| x.coverage.iter().zip(&y.coverage))
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap();
        assert!(
            ground_delta <= 6 && grass_delta <= 6,
            "ground {ground_delta}, grass {grass_delta}"
        );
    }
}
#[test]
fn disabled_and_absent_roads_leave_base_output_identical_and_add_no_grass() {
    let (d, l, p, _, mut r) = cart_tracks();
    let c = snapshot(&d, &CELLS, |_, x, z| {
        if x > 2.0 && x < 5.0 && z > 2.0 && z < 6.0 {
            0.0
        } else {
            1.0
        }
    });
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let base = plan.compile_cells(&CELLS, &c).unwrap();
    let with_road = compile(&plan, &c, &r);
    assert!((0..64).all(|z| (0..64).all(|x| grass(&with_road[0], x, z) <= grass(&base[0], x, z))));
    r.roads[0].enabled = false;
    assert_eq!(compile(&plan, &c, &r), base);
    r.roads.clear();
    r.spans.clear();
    r.profiles.clear();
    assert_eq!(compile(&plan, &c, &r), base);
}
#[test]
fn breakup_retains_grass_in_worn_areas_without_changing_outside_or_base_catalog() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let solid = compile(&plan, &c, &r);
    r.profiles[0].breakup = 1.0;
    let worn = compile(&plan, &c, &r);
    assert!((0..64).any(|x| grass(&worn[0], x, 25) > grass(&solid[0], x, 25) + 20));
    for x in 0..64 {
        assert_eq!(grass(&worn[0], x, 0), grass(&solid[0], x, 0));
        assert!(grass(&worn[0], x, 31) >= grass(&solid[0], x, 31));
    }
}
#[test]
fn incomplete_queries_and_expensive_or_invalid_geometry_fail_explicitly() {
    let (d, l, p, c, r) = cart_tracks();
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let mut bad = r.clone();
    bad.truncated = true;
    assert!(
        plan.compile_cells_with_roads(&CELLS, &c, &bad, Default::default())
            .is_err()
    );
    bad = r.clone();
    bad.loaded_bounds.maximum = CellCoord::ZERO;
    assert!(matches!(
        plan.compile_cells_with_roads(&CELLS, &c, &bad, Default::default()),
        Err(CompileError::RoadWindow(_))
    ));
    assert!(matches!(
        plan.compile_cells_with_roads(
            &CELLS,
            &c,
            &r,
            RoadCompileProfile {
                max_segments: 1,
                ..Default::default()
            }
        ),
        Err(CompileError::Budget(_))
    ));
    assert!(matches!(
        plan.compile_cells_with_roads(
            &CELLS,
            &c,
            &r,
            RoadCompileProfile {
                max_sample_work: 1,
                ..Default::default()
            }
        ),
        Err(CompileError::Budget(_))
    ));
    bad = r.clone();
    bad.spans[0].start.outgoing = [0.0, 1.0];
    bad.spans[0].end.incoming = [0.0, 1.0];
    assert!(
        plan.compile_cells_with_roads(&CELLS, &c, &bad, Default::default())
            .is_err()
    );
    bad = r.clone();
    bad.spans[0].end.position = bad.spans[0].start.position;
    bad.spans[0].start.outgoing = [0.0; 2];
    bad.spans[0].end.incoming = [0.0; 2];
    assert!(
        plan.compile_cells_with_roads(&CELLS, &c, &bad, Default::default())
            .is_err()
    );
}
#[test]
fn intersecting_routes_require_explicit_junctions_instead_of_implied_connectivity() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let mut other = r.roads[0].clone();
    other.id = RoadId([2; 16]);
    r.roads.push(other);
    let mut span = r.spans[0].clone();
    span.id = RoadSpanId([2; 16]);
    span.road = RoadId([2; 16]);
    span.start.id = RoadKnotId([3; 16]);
    span.end.id = RoadKnotId([4; 16]);
    span.start.position = RoadPoint::from_relative(CellCoord::ZERO, [4.0, -2.0], 8.0).unwrap();
    span.end.position = RoadPoint::from_relative(CellCoord::ZERO, [4.0, 10.0], 8.0).unwrap();
    span.start.outgoing = [0.0, 4.0];
    span.end.incoming = [0.0, -4.0];
    r.spans.push(span);
    assert!(matches!(
        plan.compile_cells_with_roads(&CELLS, &c, &r, Default::default()),
        Err(CompileError::RoadJunctionRequired { .. })
    ));
}
#[test]
fn geometry_is_independent_of_render_origin_at_large_logical_coordinates() {
    let (d, l, p, _, mut r) = cart_tracks();
    straight(&mut r);
    r.profiles[0].breakup = 0.0;
    r.profiles[0].edge_variation = 0.0;
    let base = snapshot(&d, &CELLS, |_, _, _| 1.0);
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let original = compile(&plan, &base, &r);
    let shift = 1_000_000_000;
    for span in &mut r.spans {
        span.start.position.cell.x += shift;
        span.end.position.cell.x += shift;
    }
    r.loaded_bounds.minimum.x += shift;
    r.loaded_bounds.maximum.x += shift;
    let cells = CELLS.map(|c| CellCoord {
        x: c.x + shift,
        z: c.z,
    });
    let source = snapshot(&d, &cells, |_, _, _| 1.0);
    let moved = plan
        .compile_cells_with_roads(&cells, &source, &r, Default::default())
        .unwrap();
    for (a, b) in original.iter().zip(&moved) {
        assert_eq!(a.ground, b.ground);
        assert_eq!(a.vegetation, b.vegetation);
    }
}

#[test]
fn routes_suppress_only_the_named_channel_and_material_budget_still_applies() {
    let (d, mut l, p, c, mut r) = cart_tracks();
    let baseline_plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let base = baseline_plan.compile_cells(&CELLS, &c).unwrap();
    let worn = compile(&baseline_plan, &c, &r);
    r.profiles[0].vegetation_channel = FLOWERS;
    let roads = compile(&baseline_plan, &c, &r);
    // Grass still follows the road's ground, but no wear clears it beyond that.
    assert_ne!(roads[0].ground, base[0].ground);
    assert!((0..64).all(|z| (0..64).all(|x| grass(&roads[0], x, z) >= grass(&worn[0], x, z))));
    assert!((0..64).any(|z| grass(&roads[0], 32, z) > grass(&worn[0], 32, z)));
    let road_ground = l
        .presets
        .iter_mut()
        .find(|p| p.id == environment::fixtures::DRY_GROUND)
        .unwrap();
    let environment::PresetKind::Ground(g) = &mut road_ground.kind else {
        unreachable!()
    };
    g.strength = 0.0;
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    assert_eq!(compile(&plan, &c, &r)[0].ground, base[0].ground);
    let narrow = CompilePlan::new(
        &d,
        &p,
        &l,
        CompileProfile {
            max_surfaces_per_cell: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        narrow
            .compile_cells_with_roads(&CELLS, &c, &r, Default::default())
            .is_ok()
    );
    let road_ground = l
        .presets
        .iter_mut()
        .find(|p| p.id == environment::fixtures::DRY_GROUND)
        .unwrap();
    let environment::PresetKind::Ground(g) = &mut road_ground.kind else {
        unreachable!()
    };
    g.strength = 1.0;
    let narrow = CompilePlan::new(
        &d,
        &p,
        &l,
        CompileProfile {
            max_surfaces_per_cell: 1,
            ..Default::default()
        },
    )
    .unwrap();
    // Past the budget a cell keeps the surfaces that show most, worn ground included and its
    // edge counting most, so the road still compiles and neighbours agree along their edge.
    let cells = narrow
        .compile_cells_with_roads(&CELLS, &c, &r, Default::default())
        .unwrap();
    assert!(cells.iter().all(|c| c.ground.surfaces.len() == 1));
    assert!(cells.iter().any(|c| !c.roads.roads.is_empty()));
    assert_eq!(cells[0].ground.surfaces, cells[1].ground.surfaces);
}
#[test]
fn detail_below_output_resolution_is_broadened_in_the_bake_and_drawn_as_authored_up_close() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    // Half-metre grids: 0.65 m tracks with 0.15 m edges are finer than they resolve.
    let coarse = CompilePlan::new(&d, &p, &l, profile()).unwrap();
    let cells = compile(&coarse, &c, &r);
    // The bake broadens the tracks to a metre, so both still clear their grass (centres at
    // z = 4 -+ 0.775; texel centres every 0.5 m from 0.25).
    for z in [6, 9] {
        assert_eq!(grass(&cells[0], 8, z), 0, "track at row {z}");
    }
    // Near detail gets the style as authored.
    let shape = cells[0].roads.roads[0].shape;
    assert_eq!(
        (shape.track_width, shape.edge_softness),
        (0.65, 0.15),
        "page roads carry the authored style"
    );
    // A distant profile cannot impose anything on this output cell.
    r.spans[0].start.position.cell.z += 20;
    r.spans[0].end.position.cell.z += 20;
    assert_eq!(
        compile(&coarse, &c, &r),
        coarse.compile_cells(&CELLS, &c).unwrap()
    );
}
#[test]
fn detail_minima_satisfy_sampling_constraints_and_broaden_only_what_falls_below() {
    let (_, _, _, _, r) = cart_tracks();
    for (size, terrain, vegetation, tolerance) in [
        (8.0, 65, 64, 0.005),
        (16.0, 65, 64, 0.005),
        (7.3, 60, 64, 0.04),
    ] {
        let compiler = RoadCompileProfile {
            curve_tolerance: tolerance,
            ..Default::default()
        };
        let limits = compiler.detail_limits(size, terrain, vegetation).unwrap();
        let step = f64::from(size) / (f64::from(terrain) - 1.0).min(f64::from(vegetation));
        assert!(f64::from(limits.track_core) >= 2.0 * step);
        assert!(f64::from(limits.edge_softness) >= 0.5 * step);
        assert!(f64::from(limits.edge_softness) * 0.1 >= tolerance);
        assert!(f64::from(limits.patch_size) >= 4.0 * step);
        let p = &r.profiles[0];
        let fine = world::RoadShape {
            seed: 1,
            track_spacing: p.track_spacing,
            track_width: limits.track_core + 0.5,
            edge_softness: limits.edge_softness,
            edge_variation: 0.1,
            edge_patch_size: limits.patch_size,
            track_wander: 0.2,
            track_wander_length: limits.patch_size,
            breakup: 0.5,
            breakup_patch_size: limits.patch_size,
        };
        assert_eq!(limits.broaden(fine), fine, "resolvable detail is kept");
        let coarse = limits.broaden(world::RoadShape {
            track_width: 0.1,
            edge_softness: 0.01,
            edge_patch_size: 0.25,
            track_wander_length: 0.25,
            breakup_patch_size: 0.25,
            ..fine
        });
        assert_eq!(coarse.track_width, limits.track_core + 0.2 + 0.2);
        assert_eq!(coarse.edge_softness, limits.edge_softness);
        assert_eq!(coarse.edge_patch_size, limits.patch_size);
        assert_eq!(coarse.track_wander_length, limits.patch_size);
        assert_eq!(coarse.breakup_patch_size, limits.patch_size);
    }
}
#[test]
fn source_records_reject_nan_duplicates_invalid_references_and_overflowing_coordinates() {
    let (_, _, _, _, r) = cart_tracks();
    assert!(r.validate().is_ok());
    let mut bad = r.clone();
    bad.spans.push(bad.spans[0].clone());
    assert!(bad.validate().is_err());
    bad = r.clone();
    bad.profiles[0].track_width = f32::NAN;
    assert!(bad.validate().is_err());
    bad = r.clone();
    bad.spans[0].start.position.local[0] = 8.0;
    assert!(bad.validate().is_err());
    bad = r.clone();
    bad.roads[0].profile = RoadProfileId([9; 16]);
    assert!(bad.validate().is_err());
    bad = r.clone();
    bad.spans[0].start.width = 0.1;
    assert!(bad.validate().is_err());
    assert!(RoadPoint::from_relative(CellCoord { x: i32::MAX, z: 0 }, [9.0, 0.0], 8.0).is_err());
    assert!(
        split_span(
            &r.spans[0],
            8.0,
            0.0,
            RoadKnotId([3; 16]),
            RoadSpanId([2; 16])
        )
        .is_err()
    );
    bad = r.clone();
    bad.spans[0].revision = u64::MAX;
    assert!(
        split_span(
            &bad.spans[0],
            8.0,
            0.5,
            RoadKnotId([3; 16]),
            RoadSpanId([2; 16])
        )
        .is_err()
    );
}
#[test]
fn offscreen_controls_are_loaded_but_unrelated_far_spans_do_not_invalidate_products() {
    let (d, l, p, c, mut r) = cart_tracks();
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let original = compile(&plan, &c, &r);
    assert!(!CELLS.contains(&r.spans[0].start.position.cell));
    assert!(!CELLS.contains(&r.spans[0].end.position.cell));
    let mut far = r.spans[0].clone();
    far.id = RoadSpanId([22; 16]);
    far.start.id = RoadKnotId([23; 16]);
    far.end.id = RoadKnotId([24; 16]);
    far.start.position.cell.z += 20;
    far.end.position.cell.z += 20;
    r.spans.push(far);
    assert_eq!(compile(&plan, &c, &r), original);
    r.spans[1].start.outgoing[0] += 0.25;
    assert_eq!(compile(&plan, &c, &r), original);
}
#[test]
fn overlapping_shoulders_wear_at_least_as_much_and_are_input_order_independent() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let a = compile(&plan, &c, &r);
    let mut other = r.clone();
    other.roads[0].id = RoadId([2; 16]);
    other.spans[0].road = RoadId([2; 16]);
    other.spans[0].id = RoadSpanId([2; 16]);
    other.spans[0].start.id = RoadKnotId([3; 16]);
    other.spans[0].end.id = RoadKnotId([4; 16]);
    other.spans[0].start.position.local[1] += 2.0;
    other.spans[0].end.position.local[1] += 2.0;
    let b = compile(&plan, &c, &other);
    r.roads.extend(other.roads);
    r.spans.extend(other.spans);
    let both = compile(&plan, &c, &r);
    for z in 0..64 {
        for x in 0..64 {
            assert!(grass(&both[0], x, z) <= grass(&a[0], x, z).min(grass(&b[0], x, z)));
        }
    }
    r.roads.reverse();
    r.spans.reverse();
    assert_eq!(compile(&plan, &c, &r), both);
}
#[test]
fn same_road_continuation_does_not_leave_an_internal_cap_or_reroll_noise() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    r.profiles[0].edge_variation = 0.08;
    r.profiles[0].breakup = 0.9;
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let before = compile(&plan, &c, &r);
    let (a, b) = split_span(
        &r.spans[0],
        8.0,
        0.5,
        RoadKnotId([3; 16]),
        RoadSpanId([2; 16]),
    )
    .unwrap();
    r.spans = vec![a, b];
    let after = compile(&plan, &c, &r);
    for (a, b) in before.iter().zip(after) {
        assert_eq!(a.ground, b.ground);
        assert_eq!(a.vegetation, b.vegetation);
    }
}

#[test]
fn width_taper_survives_knot_insertion_on_a_straight_curve_with_nonuniform_speed() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    r.spans[0].start.outgoing = [1.0, 0.0];
    r.spans[0].end.incoming = [-1.0, 0.0];
    r.spans[0].start.width = 3.0;
    r.spans[0].end.width = 6.0;
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let before = compile(&plan, &c, &r);
    let (a, b) = split_span(
        &r.spans[0],
        8.0,
        0.27,
        RoadKnotId([3; 16]),
        RoadSpanId([2; 16]),
    )
    .unwrap();
    r.spans = vec![a, b];
    let after = compile(&plan, &c, &r);
    for (a, b) in before.iter().zip(&after) {
        for z in 0..64 {
            for x in 0..64 {
                assert!(grass(a, x, z).abs_diff(grass(b, x, z)) <= 4);
            }
        }
    }
}
#[test]
fn finite_end_caps_do_not_turn_into_infinite_wheel_strips() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    r.spans[0].start.position = RoadPoint::from_relative(CellCoord::ZERO, [3.0, 4.0], 8.0).unwrap();
    r.spans[0].end.position = RoadPoint::from_relative(CellCoord::ZERO, [10.0, 4.0], 8.0).unwrap();
    r.spans[0].start.outgoing = [7.0 / 3.0, 0.0];
    r.spans[0].end.incoming = [-7.0 / 3.0, 0.0];
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let output = compile(&plan, &c, &r);
    assert_eq!(grass(&output[0], 0, 25), 255);
    assert_eq!(ground_at(&output[0], 0, 26), 0);
    assert_eq!(grass(&output[0], 32, 25), 0);
    assert_eq!(grass(&output[1], 63, 25), 255);
}
#[test]
fn road_thinning_keeps_cpu_reference_roots_at_their_original_positions() {
    let (d, l, p, c, r) = cart_tracks();
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    let before = plan.compile_cells(&CELLS, &c).unwrap();
    let after = compile(&plan, &c, &r);
    let placements = |cells: &[CompiledCell]| {
        let scene = vegetation::VegetationScene {
            catalog: plan.catalog().clone(),
            pages: cells
                .iter()
                .map(|c| vegetation::VegetationFieldPage {
                    origin_xz: [c.cell.x as f32 * 8.0, c.cell.z as f32 * 8.0],
                    size: 8.0,
                    surface: vegetation::VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
                    fields: c.vegetation.fields.clone(),
                })
                .collect(),
        };
        vegetation_compile::generate_scene_debug_placements(&scene)
            .unwrap()
            .into_iter()
            .map(|p| (p.grass, p.seed, p.root.map(f32::to_bits)))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let original = placements(&before);
    let retained = placements(&after);
    assert!(!retained.is_empty());
    assert!(retained.len() < original.len());
    assert!(retained.is_subset(&original));
}
#[test]
fn page_roads_reproduce_the_compiled_road_ground_for_near_detail() {
    let (d, l, p, c, r) = cart_tracks();
    let plan = CompilePlan::new(&d, &p, &l, CompileProfile::default()).unwrap();
    let cells = compile(&plan, &c, &r);
    let mut worn = 0;
    for (cell, compiled) in CELLS.iter().zip(&cells) {
        let roads = &compiled.roads;
        assert_eq!(roads.roads.len(), 1);
        assert!(!roads.segments.is_empty());
        let n = usize::from(compiled.ground.weight_pages[0].resolution);
        let size = f64::from(d.cell_size);
        let origin = [f64::from(cell.x) * size, f64::from(cell.z) * size];
        for z in 0..n {
            for x in 0..n {
                let local = [x, z].map(|v| v as f64 * size / (n - 1) as f64);
                let ground = roads.ground(origin, local);
                let compiled = f64::from(ground_at(compiled, x, z));
                assert!(
                    (ground * 255.0 - compiled).abs() <= 1.0,
                    "{cell:?} at {local:?}: page roads draw {ground}, the compiler laid {compiled}"
                );
                worn += usize::from(ground > 0.5);
            }
        }
    }
    assert!(worn > 20, "the road wore {worn} samples");
}
#[test]
fn road_wear_clears_plants_but_leaves_stones_and_twigs() {
    let (d, l, mut p, c, mut r) = cart_tracks();
    straight(&mut r);
    let detail = |key: &str, on_paths| vegetation::DetailType {
        key: key.into(),
        assets: vec![format!("{key}.glb")],
        density: 1.0,
        scale: [1.0, 1.0],
        clumps: vegetation::DetailClumps {
            size: 1.0,
            pull: 0.0,
            patches: 0.0,
        },
        sink: 0.0,
        align: 0.0,
        tilt: 0.0,
        distance: 20.0,
        shadows: false,
        on_paths,
        wear: None,
    };
    let (pebbles, nettles) = (detail("pebbles", true), detail("nettles", false));
    // Both grow on the meadow and on the road's bare soil alike.
    p.surfaces.push(vegetation::SurfaceGrass {
        surface: SOIL.0,
        grass: vec![],
    });
    for s in &mut p.surfaces {
        for t in [&pebbles, &nettles] {
            s.grass.push(vegetation::GrassAmount {
                grass: t.id(),
                amount: 1.0,
            });
        }
    }
    p.details = vec![pebbles.clone(), nettles.clone()];
    let plan = CompilePlan::new(&d, &p, &l, CompileProfile::default()).unwrap();
    let cells = compile(&plan, &c, &r);
    let at = |id, z: usize| {
        let field = cells[0]
            .vegetation
            .fields
            .iter()
            .find(|f| f.grass == id)
            .unwrap();
        field.coverage[z * usize::from(field.resolution) + 32]
    };
    // A wheel track (row 25) is worn bare of plants; pebbles still lie there.
    assert_eq!(at(nettles.id(), 25), 0);
    assert_eq!(at(pebbles.id(), 25), 255);
    // Off the road both grow.
    assert_eq!(at(nettles.id(), 0), 255);
    assert_eq!(at(pebbles.id(), 0), 255);
}
#[test]
fn details_with_a_wear_band_lie_where_wear_falls_off() {
    let (d, l, mut p, c, mut r) = cart_tracks();
    straight(&mut r);
    let verge = vegetation::DetailType {
        key: "verge".into(),
        assets: vec!["verge.glb".into()],
        density: 1.0,
        scale: [1.0, 1.0],
        clumps: vegetation::DetailClumps {
            size: 1.0,
            pull: 0.0,
            patches: 0.0,
        },
        sink: 0.0,
        align: 0.0,
        tilt: 0.0,
        distance: 20.0,
        shadows: false,
        on_paths: false,
        wear: Some([0.02, 0.15, 0.35]),
    };
    // On the meadow the road wears; its worn soil carries none.
    for s in &mut p.surfaces {
        if s.surface == GREEN_SOIL.0 {
            s.grass.push(vegetation::GrassAmount {
                grass: verge.id(),
                amount: 1.0,
            });
        }
    }
    p.details = vec![verge.clone()];
    let plan = CompilePlan::new(&d, &p, &l, CompileProfile::default()).unwrap();
    let cells = compile(&plan, &c, &r);
    let field = cells[0]
        .vegetation
        .fields
        .iter()
        .find(|f| f.grass == verge.id())
        .unwrap();
    let column: Vec<u8> = (0..64).map(|z| field.coverage[z * 64 + 32]).collect();
    // None away from the road or in its wheel tracks; some along each edge, outside the tracks.
    assert_eq!(column[0], 0);
    assert_eq!(column[63], 0);
    assert_eq!(column[25], 0);
    assert_eq!(column[38], 0);
    assert!(column[..25].iter().any(|&v| v > 100), "{column:?}");
    assert!(column[39..].iter().any(|&v| v > 100), "{column:?}");
    // The plain compile has no wear, so nothing grows there.
    let base = plan.compile_cells(&CELLS, &c).unwrap();
    assert!(
        base[0]
            .vegetation
            .fields
            .iter()
            .all(|f| f.grass != verge.id())
    );
}
#[test]
fn the_wear_field_draws_what_roads_wear_and_names_the_worn_ground() {
    let (d, l, p, c, mut r) = cart_tracks();
    straight(&mut r);
    let plan = CompilePlan::new(&d, &p, &l, CompileProfile::default()).unwrap();
    let cells = compile(&plan, &c, &r);
    let cell = &cells[0];
    let wear = cell.wear.as_ref().unwrap();
    let surfaces = &cell.ground.surfaces;
    assert!(wear.is_valid(surfaces.len()));
    // Green meadow wears down to soil; soil to itself.
    let slot = |s| surfaces.iter().position(|x| *x == s).unwrap();
    let none = (world::NO_WORN_SLOT, 0);
    assert_eq!(wear.worn[slot(GREEN_SOIL)], [(slot(SOIL) as u8, 255), none]);
    assert_eq!(wear.worn[slot(SOIL)], [none, none]);
    // Texels hold the roads' wear at their centres.
    let n = usize::from(wear.resolution);
    let origin = [f64::from(cell.cell.x) * 8.0, f64::from(cell.cell.z) * 8.0];
    for i in (0..n * n).step_by(37) {
        let local = [(i % n) as f64 + 0.5, (i / n) as f64 + 0.5].map(|v| v / n as f64 * 8.0);
        let s = cell.roads.wear(origin, local);
        assert_eq!(
            wear.texels[i],
            world::TerrainWear::encode(s.wear, s.rut, s.direction)
        );
    }
    let worn = wear.texels.iter().filter(|t| t[0] > 200).count();
    assert!(worn > 1000, "{worn} texels worn");
    // A cell no road reaches has no field.
    let far = plan.compile_cells(&CELLS, &c).unwrap();
    assert!(far.iter().all(|c| c.wear.is_none()));
}
