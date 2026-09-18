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
fn ground_at(cell: &CompiledCell, x: usize, z: usize) -> u8 {
    ground_weights(&cell.ground, x, z)
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
fn cart_tracks_preserve_existing_center_and_outside_grass_and_keep_population_identities() {
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
            .map(|f| f.population)
            .collect::<Vec<_>>(),
        base[0]
            .vegetation
            .fields
            .iter()
            .map(|f| f.population)
            .collect::<Vec<_>>()
    );
    // 8 m / 64 vegetation samples. Center at z=4; tracks at 4 ± .775.
    assert_eq!(grass(&cells[0], 32, 31), 255);
    assert_eq!(grass(&cells[0], 32, 0), 255);
    assert_eq!(grass(&cells[0], 32, 25), 0);
    assert_eq!(grass(&cells[0], 32, 38), 0);
    assert_eq!(ground_at(&cells[0], 32, 32), 0);
    assert!(ground_at(&cells[0], 32, 26) > 230);
    let shoulder = grass(&cells[0], 32, 20);
    assert!(shoulder > 0 && shoulder < 255);
    // A wider road changes only shoulders, not wheel spacing or the center.
    r.spans[0].start.width = 5.0;
    r.spans[0].end.width = 5.0;
    let wide = compile(&plan, &c, &r);
    assert_eq!(grass(&wide[0], 32, 25), 0);
    assert_eq!(grass(&wide[0], 32, 31), 255);
    assert!(grass(&wide[0], 32, 17) < grass(&cells[0], 32, 17));
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
fn disabled_and_absent_roads_leave_base_output_identical_and_do_not_fill_unpainted_holes() {
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
    assert_eq!(grass(&with_road[0], 24, 32), 0);
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
        assert_eq!(grass(&worn[0], x, 31), 255);
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
    r.profiles[0].vegetation_channel = FLOWERS;
    let roads = compile(&baseline_plan, &c, &r);
    assert_eq!(roads[0].vegetation, base[0].vegetation);
    assert_ne!(roads[0].ground, base[0].ground);
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
    assert!(matches!(
        narrow.compile_cells_with_roads(&CELLS, &c, &r, Default::default()),
        Err(CompileError::SurfaceLimit { .. })
    ));
}
#[test]
fn detail_below_output_resolution_is_rejected_instead_of_losing_wheel_tracks() {
    let (d, l, p, c, mut r) = cart_tracks();
    let coarse = CompilePlan::new(&d, &p, &l, profile()).unwrap();
    assert!(matches!(
        coarse.compile_cells_with_roads(&CELLS, &c, &r, Default::default()),
        Err(CompileError::Road(_))
    ));
    // A distant profile cannot impose its detail requirements on this output cell.
    r.spans[0].start.position.cell.z += 20;
    r.spans[0].end.position.cell.z += 20;
    assert_eq!(
        compile(&coarse, &c, &r),
        coarse.compile_cells(&CELLS, &c).unwrap()
    );
}
#[test]
fn advertised_detail_minima_satisfy_sampling_constraints_and_identify_invalid_fields() {
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
        let mut p = r.profiles[0].clone();
        p.edge_variation = 0.0;
        p.track_width = limits.track_core.max(limits.edge_softness);
        p.edge_softness = limits.edge_softness;
        p.edge_patch_size = limits.patch_size;
        p.breakup_patch_size = limits.patch_size;
        compiler
            .validate_detail(&p, size, terrain, vegetation)
            .unwrap();
        for field in [
            "edge softness",
            "edge patch size",
            "breakup patch size",
            "track width",
        ] {
            let mut invalid = p.clone();
            match field {
                "edge softness" => invalid.edge_softness = limits.edge_softness.next_down(),
                "edge patch size" => invalid.edge_patch_size = limits.patch_size.next_down(),
                "breakup patch size" => invalid.breakup_patch_size = limits.patch_size.next_down(),
                _ => invalid.track_width = limits.track_core.next_down(),
            }
            assert!(
                compiler
                    .validate_detail(&invalid, size, terrain, vegetation)
                    .unwrap_err()
                    .to_string()
                    .contains(field)
            );
        }
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
fn overlapping_shoulders_use_maximum_suppression_and_are_input_order_independent() {
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
            assert_eq!(
                grass(&both[0], x, z),
                grass(&a[0], x, z).min(grass(&b[0], x, z))
            );
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
            .map(|p| (p.population, p.seed, p.root.map(f32::to_bits)))
            .collect::<std::collections::BTreeSet<_>>()
    };
    let original = placements(&before);
    let retained = placements(&after);
    assert!(!retained.is_empty());
    assert!(retained.len() < original.len());
    assert!(retained.is_subset(&original));
}
