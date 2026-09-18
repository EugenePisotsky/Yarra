mod support;

use std::collections::HashSet;

use environment::fixtures::{
    CLEARING, DRY_FOLIAGE, DRY_GROUND, DRY_MEADOW, FOLIAGE_USE, GREEN_FOLIAGE, GREEN_GROUND,
    GREEN_MEADOW, GROUND_USE,
};
use environment::*;
use support::*;
use vegetation::{VegetationFieldPage, VegetationSurfaceField, fixtures};
use world::{CellCoord, TerrainSurfaceId, WorldSpaceId};
use yarra_environment_compile::{CompileError, CompilePlan};

#[test]
fn local_and_batch_compilation_match_with_canonical_order_and_seamless_ground() {
    let (definition, library, plants, source) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let batch = plan.compile_cells(&CELLS, &source).unwrap();
    for (cell, expected) in CELLS.iter().zip(&batch) {
        let local = plan.compile_cells(&[*cell], &source).unwrap();
        assert_eq!(&local[0], expected);
        for z in 0..17 {
            for x in 0..17 {
                assert_eq!(
                    ground_weights(&expected.ground, x, z)
                        .iter()
                        .map(|(_, v)| u16::from(*v))
                        .sum::<u16>(),
                    255
                );
            }
        }
    }
    for z in 0..17 {
        assert_eq!(
            ground_weights(&batch[0].ground, 16, z),
            ground_weights(&batch[1].ground, 0, z)
        );
    }
    let mut reordered_definition = definition.clone();
    reordered_definition.layers.reverse();
    let mut reordered_library = library.clone();
    reordered_library.presets.reverse();
    for p in &mut reordered_library.presets {
        if let PresetKind::Composition(c) = &mut p.kind {
            c.reverse();
        }
    }
    reordered_definition.surfaces.reverse();
    let mut reordered_source = source.clone();
    reordered_source.cells.reverse();
    for cell in &mut reordered_source.cells {
        cell.tiles.reverse();
    }
    let mut reordered_plants = plants.clone();
    reordered_plants.populations.reverse();
    reordered_plants.species.reverse();
    reordered_plants.assemblages.reverse();
    for assemblage in &mut reordered_plants.assemblages {
        assemblage.populations.reverse();
    }
    let other = CompilePlan::new(
        &reordered_definition,
        &reordered_plants,
        &reordered_library,
        profile(),
    )
    .unwrap();
    assert_eq!(plan.fingerprint(), other.fingerprint());
    assert_eq!(
        batch,
        other
            .compile_cells(&[CELLS[1], CELLS[0]], &reordered_source)
            .unwrap()
    );
}

#[test]
fn holes_and_clearing_are_preserved_and_erasing_reveals_lower_vegetation() {
    let (mut definition, library, plants, source) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let cells = plan.compile_cells(&CELLS, &source).unwrap();
    // Deep inside the deliberately unpainted 2x2 m patch.
    assert!(
        cells[0]
            .vegetation
            .fields
            .iter()
            .all(|f| f.coverage[3 * 16 + 3] == 0)
    );
    // Inside the winding exclusion at x=8.25, z=4.25.
    assert!(
        cells[1]
            .vegetation
            .fields
            .iter()
            .all(|f| f.coverage[8 * 16] == 0)
    );
    definition
        .layers
        .iter_mut()
        .find(|l| l.id == CLEAR)
        .unwrap()
        .enabled = false;
    let restored = CompilePlan::new(&definition, &plants, &library, profile())
        .unwrap()
        .compile_cells(&CELLS, &source)
        .unwrap();
    assert!(
        restored[1]
            .vegetation
            .fields
            .iter()
            .any(|f| f.coverage[8 * 16] > 0)
    );
}

#[test]
fn ground_borders_match_even_when_cells_have_different_palettes() {
    let (mut definition, library, plants, _) = fixture();
    definition.layers[2].enabled = false;
    let source = snapshot(&definition, &CELLS, |l, x, _| {
        if l == GREEN {
            ((x - 6.0) / 2.0).clamp(0.0, 1.0)
        } else {
            1.0
        }
    });
    let cells = CompilePlan::new(&definition, &plants, &library, profile())
        .unwrap()
        .compile_cells(&CELLS, &source)
        .unwrap();
    assert_eq!(cells[0].ground.surfaces.len(), 2);
    assert_eq!(cells[1].ground.surfaces, vec![GREEN_SOIL]);
    assert!(cells[1].ground.weight_pages.is_empty());
    for z in 0..17 {
        assert_eq!(
            ground_weights(&cells[0].ground, 16, z),
            ground_weights(&cells[1].ground, 0, z)
        );
    }
}

#[test]
fn quantization_preserves_total_and_breaks_ties_by_surface_id() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers[0].enabled = false;
    definition.layers[2].enabled = false;
    soil(&mut library, GREEN_GROUND).strength = 0.5;
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let cells = CompilePlan::new(&definition, &plants, &library, profile())
        .unwrap()
        .compile_cells(&CELLS, &source)
        .unwrap();
    assert_eq!(
        ground_weights(&cells[0].ground, 2, 3),
        vec![(SOIL, 128), (GREEN_SOIL, 127)]
    );
}

#[test]
fn exclusions_are_channel_specific_and_do_not_repaint_ground() {
    let (mut definition, mut library, plants, _) = fixture();
    append(
        &mut library,
        DRY_MEADOW,
        PresetId([90; 16]),
        PresetKind::Foliage(VegetationTreatment {
            id: OutputId([4; 16]),
            channel: FLOWERS,
            assemblage: fixtures::MIXED_GREEN_ASSEMBLAGE_ID,
            blend: VegetationBlend::Add,
            strength: 1.0,
            density: 1.0,
            seed: 0,
        }),
    );
    definition.layers[1].enabled = false;
    children(&mut library, CLEARING).retain(|c| c.id != GROUND_USE);
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let cells = plan.compile_cells(&CELLS, &source).unwrap();
    for cell in cells {
        assert_eq!(cell.ground.surfaces, vec![SOIL]);
        assert!(!cell.vegetation.fields.is_empty());
        for field in &cell.vegetation.fields {
            let binding = plan
                .bindings()
                .iter()
                .find(|b| b.runtime_population == field.population)
                .unwrap();
            assert_eq!(binding.channel, FLOWERS);
            assert!(field.coverage.iter().all(|&v| v == 255));
        }
    }
}

#[test]
fn a_layers_exclusion_does_not_remove_its_own_vegetation() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers[2].enabled = false;
    foliage(&mut library, GREEN_FOLIAGE).blend = VegetationBlend::Add;
    append(
        &mut library,
        GREEN_MEADOW,
        PresetId([90; 16]),
        PresetKind::Exclusion(Exclusion {
            id: OutputId([5; 16]),
            channel: GRASS,
            strength: 1.0,
        }),
    );
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let cells = plan.compile_cells(&CELLS, &source).unwrap();
    assert_eq!(cells[0].vegetation.fields.len(), 3);
    for field in &cells[0].vegetation.fields {
        let binding = plan
            .bindings()
            .iter()
            .find(|b| b.runtime_population == field.population)
            .unwrap();
        assert_eq!(binding.key.layer, GREEN);
        assert!(field.coverage.iter().all(|&v| v == 255));
    }
}

#[test]
fn layer_seeds_and_competition_are_isolated_but_internal_competition_survives() {
    let (mut definition, mut library, plants, _) = fixture();
    foliage(&mut library, DRY_FOLIAGE).assemblage = fixtures::MIXED_GREEN_ASSEMBLAGE_ID;
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let groups = |layer| {
        plan.bindings()
            .iter()
            .filter(|b| b.key.layer == layer)
            .filter_map(|b| {
                plan.catalog()
                    .population(b.runtime_population)
                    .unwrap()
                    .competition_group
            })
            .collect::<HashSet<_>>()
    };
    assert_eq!(groups(DRY).len(), 1);
    assert_eq!(groups(GREEN).len(), 1);
    assert_ne!(groups(DRY), groups(GREEN));
    let seeds: HashSet<_> = plan.catalog().populations.iter().map(|p| p.seed).collect();
    assert_eq!(seeds.len(), 6);
    let catalog = plan.catalog().clone();
    definition.layers[1].enabled = false;
    definition.layers[0].name = "Renamed".into();
    definition.layers[0].order = 100;
    let changed = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    assert_eq!(catalog, *changed.catalog());
    assert_eq!(plan.bindings(), changed.bindings());
}

#[test]
fn local_density_retains_actual_roots_without_reseeding_the_lattice() {
    let (mut definition, library, plants, _) = fixture();
    definition.layers[1].enabled = false;
    definition.layers[2].enabled = false;
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let full = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    definition.layers[0].overrides.push(PresetOverride {
        path: vec![FOLIAGE_USE],
        value: QuickValue::FoliageDensity(0.35),
    });
    let thin = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    assert_eq!(full.catalog(), thin.catalog());
    let roots = |plan: &CompilePlan| {
        let cells = plan.compile_cells(&CELLS, &source).unwrap();
        cells
            .into_iter()
            .flat_map(|cell| {
                let page = VegetationFieldPage {
                    origin_xz: [cell.cell.x as f32 * 8.0, 0.0],
                    size: 8.0,
                    surface: VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
                    fields: cell.vegetation.fields,
                };
                vegetation_compile::generate_page_debug_placements(plan.catalog(), &page).unwrap()
            })
            .map(|p| (p.population, p.seed, p.root.map(f32::to_bits)))
            .collect::<HashSet<_>>()
    };
    let full_roots = roots(&full);
    let thin_roots = roots(&thin);
    assert!(!thin_roots.is_empty());
    assert!(thin_roots.len() < full_roots.len());
    assert!(thin_roots.is_subset(&full_roots));
}

#[test]
fn replace_and_exclude_attenuate_lower_content_once_while_add_retains_it() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers[2].enabled = false;
    definition.layers[1].opacity = 0.5;
    append(
        &mut library,
        GREEN_MEADOW,
        PresetId([90; 16]),
        PresetKind::Exclusion(Exclusion {
            id: OutputId([8; 16]),
            channel: GRASS,
            strength: 1.0,
        }),
    );
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let replace = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let replaced = replace.compile_cells(&CELLS, &source).unwrap();
    assert!(
        replaced[0]
            .vegetation
            .fields
            .iter()
            .all(|f| f.coverage.iter().all(|&v| v == 128))
    );
    foliage(&mut library, GREEN_FOLIAGE).blend = VegetationBlend::Add;
    children(&mut library, GREEN_MEADOW).retain(|c| c.id == FOLIAGE_USE || c.id == GROUND_USE);
    let add = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let added = add.compile_cells(&CELLS, &source).unwrap();
    for field in &added[0].vegetation.fields {
        let layer = add
            .bindings()
            .iter()
            .find(|b| b.runtime_population == field.population)
            .unwrap()
            .key
            .layer;
        let expected = if layer == DRY { 255 } else { 128 };
        assert!(field.coverage.iter().all(|&v| v == expected));
    }
}

#[test]
fn ground_only_and_decoration_only_outputs_do_not_modify_other_channels() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers[2].enabled = false;
    children(&mut library, GREEN_MEADOW).retain(|c| c.id != FOLIAGE_USE);
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let ground_only = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let compiled = ground_only.compile_cells(&CELLS, &source).unwrap();
    assert_eq!(compiled[0].ground.surfaces, vec![GREEN_SOIL]);
    assert_eq!(compiled[0].vegetation.fields.len(), 2);
    assert!(
        compiled[0]
            .vegetation
            .fields
            .iter()
            .all(|f| f.coverage.iter().all(|&v| v == 255))
    );

    children(&mut library, GREEN_MEADOW).retain(|c| c.id != GROUND_USE);
    append(
        &mut library,
        GREEN_MEADOW,
        PresetId([90; 16]),
        PresetKind::Foliage(VegetationTreatment {
            id: OutputId([2; 16]),
            channel: FLOWERS,
            assemblage: fixtures::MIXED_GREEN_ASSEMBLAGE_ID,
            blend: VegetationBlend::Replace,
            strength: 1.0,
            density: 1.0,
            seed: 0,
        }),
    );
    let flowers_only = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let compiled = flowers_only.compile_cells(&CELLS, &source).unwrap();
    assert_eq!(compiled[0].ground.surfaces, vec![SOIL]);
    assert_eq!(compiled[0].vegetation.fields.len(), 5);
    assert!(
        compiled[0]
            .vegetation
            .fields
            .iter()
            .all(|f| f.coverage.iter().all(|&v| v == 255))
    );
}

#[test]
fn known_cell_at_coordinate_limit_reports_unrepresentable_halo() {
    let (definition, library, plants, _) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let cell = CellCoord { x: i32::MAX, z: 0 };
    let source = CoverageSnapshot {
        space: definition.space,
        cells: vec![CoverageCell {
            cell,
            revision: 1,
            tiles: vec![],
        }],
    };
    assert!(matches!(
        plan.compile_cells(&[cell], &source),
        Err(CompileError::CoordinateRange)
    ));
}

#[test]
fn unloaded_cells_corrupt_borders_and_invalid_tiles_fail_without_partial_results() {
    let (definition, library, plants, source) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let mut missing_target = source.clone();
    missing_target.cells.retain(|c| c.cell != CELLS[0]);
    assert!(matches!(
        plan.compile_cells(&CELLS, &missing_target),
        Err(CompileError::UnloadedCell(_))
    ));
    let mut missing_halo = source.clone();
    missing_halo
        .cells
        .retain(|c| c.cell != CellCoord { x: -1, z: -1 });
    assert!(matches!(
        plan.compile_cells(&CELLS, &missing_halo),
        Err(CompileError::UnloadedCell(_))
    ));
    let mut corrupt = source.clone();
    corrupt
        .cells
        .iter_mut()
        .find(|c| c.cell == CELLS[0])
        .unwrap()
        .tiles[0]
        .samples[16] = 0;
    assert!(matches!(
        plan.compile_cells(&CELLS, &corrupt),
        Err(CompileError::BorderMismatch { .. })
    ));
    let mut malformed = source.clone();
    malformed.cells[0].tiles[0].samples.pop();
    assert!(matches!(
        plan.compile_cells(&CELLS, &malformed),
        Err(CompileError::InvalidTile(_))
    ));
    let mut duplicate = source.clone();
    let tile = duplicate.cells[0].tiles[0].clone();
    duplicate.cells[0].tiles.push(tile);
    assert!(matches!(
        plan.compile_cells(&CELLS, &duplicate),
        Err(CompileError::InvalidTile(_))
    ));
    assert!(matches!(
        plan.compile_cells(&[CELLS[0], CELLS[0]], &source),
        Err(CompileError::DuplicateCell(_))
    ));
    let mut wrong_world = source.clone();
    wrong_world.space = WorldSpaceId(2);
    assert!(matches!(
        plan.compile_cells(&CELLS, &wrong_world),
        Err(CompileError::WrongSpace)
    ));
}

#[test]
fn absent_tiles_in_loaded_cells_mean_empty_but_unknown_cells_never_do() {
    let (definition, library, plants, mut source) = fixture();
    for cell in &mut source.cells {
        cell.tiles.clear();
    }
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let cells = plan.compile_cells(&CELLS, &source).unwrap();
    assert!(
        cells
            .iter()
            .all(|c| c.vegetation.fields.is_empty() && c.ground.surfaces == vec![SOIL])
    );
}

#[test]
fn dependency_fingerprints_include_revisions_but_ignore_unrelated_loaded_cells() {
    let (definition, library, plants, mut source) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let original = plan.compile_cells(&CELLS, &source).unwrap();
    source.cells.push(CoverageCell {
        cell: CellCoord { x: 50, z: 50 },
        revision: 1,
        tiles: vec![],
    });
    assert_eq!(original, plan.compile_cells(&CELLS, &source).unwrap());
    source
        .cells
        .iter_mut()
        .find(|c| c.cell == CELLS[0])
        .unwrap()
        .revision += 1;
    let changed = plan.compile_cells(&CELLS, &source).unwrap();
    assert_ne!(original[0].input_fingerprint, changed[0].input_fingerprint);
    assert_eq!(original[0].ground, changed[0].ground);
    assert_eq!(original[0].vegetation, changed[0].vegetation);
}

#[test]
fn unsupported_material_counts_and_work_budgets_are_errors() {
    let (mut definition, mut library, plants, source) = fixture();
    let third = TerrainSurfaceId([3; 16]);
    definition.surfaces.push(third);
    children(&mut library, CLEARING).retain(|c| c.id != GROUND_USE);
    append(
        &mut library,
        CLEARING,
        PresetId([99; 16]),
        PresetKind::Ground(ground(third)),
    );
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    assert!(matches!(
        plan.compile_cells(&CELLS, &source),
        Err(CompileError::SurfaceLimit {
            required: 3,
            maximum: 2,
            ..
        })
    ));
    for restriction in 0..4 {
        let mut limits = profile();
        match restriction {
            0 => limits.max_output_bytes = 1,
            1 => limits.max_sample_work = 1,
            2 => limits.max_mask_samples = 1,
            _ => limits.max_cells_per_batch = 1,
        }
        let plan = CompilePlan::new(&definition, &plants, &library, limits).unwrap();
        assert!(matches!(
            plan.compile_cells(&CELLS, &source),
            Err(CompileError::Budget(_))
        ));
    }
    let mut limits = profile();
    limits.max_population_bindings = 1;
    assert!(matches!(
        CompilePlan::new(&definition, &plants, &library, limits),
        Err(CompileError::Budget(_))
    ));
}

#[test]
fn malformed_definition_references_and_nonfinite_controls_are_rejected() {
    let (definition, library, plants, _) = fixture();
    let mut invalid = definition.clone();
    invalid.layers[0].opacity = f32::NAN;
    assert!(invalid.validate(&plants, &library).is_err());
    let mut bad_library = library.clone();
    soil(&mut bad_library, DRY_GROUND).surfaces[0].weight = f32::INFINITY;
    assert!(definition.validate(&plants, &bad_library).is_err());
    bad_library = library.clone();
    foliage(&mut bad_library, DRY_FOLIAGE).assemblage =
        vegetation::VegetationAssemblageId([99; 16]);
    assert!(matches!(
        definition.validate(&plants, &bad_library),
        Err(ValidationError::MissingAssemblage(_))
    ));
    bad_library = library.clone();
    let extra = foliage(&mut bad_library, DRY_FOLIAGE).clone();
    append(
        &mut bad_library,
        DRY_MEADOW,
        PresetId([99; 16]),
        PresetKind::Foliage(extra),
    );
    assert!(definition.validate(&plants, &bad_library).is_err());
    invalid = definition;
    invalid.layers[0].preset = PresetId([99; 16]);
    assert!(matches!(
        invalid.validate(&plants, &library),
        Err(ValidationError::Preset(_))
    ));
}

#[test]
fn logical_negative_and_large_cell_coordinates_do_not_affect_sampling() {
    let (definition, library, plants, _) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let mut reference = None;
    for cell in [
        CellCoord { x: -20, z: -3 },
        CellCoord {
            x: 1_000_000_000,
            z: 999_999_999,
        },
    ] {
        let source = snapshot(
            &definition,
            &[cell],
            |l, _, _| if l == DRY { 1.0 } else { 0.0 },
        );
        let compiled = plan.compile_cells(&[cell], &source).unwrap().remove(0);
        let result = (compiled.ground, compiled.vegetation);
        if let Some(reference) = &reference {
            assert_eq!(reference, &result);
        } else {
            reference = Some(result);
        }
    }
    let empty = CoverageSnapshot {
        space: definition.space,
        cells: vec![],
    };
    assert!(matches!(
        plan.compile_cells(&[CellCoord { x: i32::MAX, z: 0 }], &empty),
        Err(CompileError::UnloadedCell(_))
    ));
}

#[test]
fn world_plans_namespace_populations_and_merge_catalogs_in_canonical_order() {
    let (a, library, plants, _) = fixture();
    let mut b = a.clone();
    b.space = world::WorldSpaceId(2);
    let pa = CompilePlan::new(&a, &plants, &library, profile()).unwrap();
    let pb = CompilePlan::new(&b, &plants, &library, profile()).unwrap();
    let merged = yarra_environment_compile::merge_runtime_catalogs(&[&pa, &pb]).unwrap();
    assert_eq!(
        merged,
        yarra_environment_compile::merge_runtime_catalogs(&[&pb, &pa]).unwrap()
    );
    assert_eq!(
        merged.populations.len(),
        pa.catalog().populations.len() + pb.catalog().populations.len()
    );
    for first in &pa.catalog().populations {
        for second in &pb.catalog().populations {
            assert_ne!(first.id, second.id);
        }
    }
    let groups = |plan: &CompilePlan| {
        plan.catalog()
            .populations
            .iter()
            .filter_map(|p| {
                merged
                    .populations
                    .iter()
                    .find(|m| m.id == p.id)
                    .unwrap()
                    .competition_group
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert!(groups(&pa).is_disjoint(&groups(&pb)));
}

#[test]
fn repeated_preset_uses_keep_independent_runtime_seeds_and_density_overrides() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers.truncate(1);
    foliage(&mut library, DRY_FOLIAGE).blend = VegetationBlend::Add;
    let second = PresetUseId([99; 16]);
    children(&mut library, DRY_MEADOW).push(PresetUse {
        id: second,
        name: "Second grass use".into(),
        preset: DRY_FOLIAGE,
        overrides: vec![],
    });
    let baseline = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    assert_eq!(baseline.bindings().len(), 4);
    definition.layers[0].overrides = vec![
        PresetOverride {
            path: vec![second],
            value: QuickValue::FoliageDensity(0.3),
        },
        PresetOverride {
            path: vec![second],
            value: QuickValue::FoliageSeed(123),
        },
    ];
    let changed = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    assert_eq!(baseline.bindings(), changed.bindings());
    let changed_seeds = baseline
        .catalog()
        .populations
        .iter()
        .filter(|p| p.seed != changed.catalog().population(p.id).unwrap().seed)
        .count();
    assert_eq!(changed_seeds, 2);
    definition.layers[0].overrides.reverse();
    assert_eq!(
        changed.fingerprint(),
        CompilePlan::new(&definition, &plants, &library, profile())
            .unwrap()
            .fingerprint()
    );
}

#[test]
fn plan_fingerprints_follow_only_reachable_preset_revisions() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers.truncate(1);
    let before = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    library.revision += 1;
    library
        .presets
        .iter_mut()
        .find(|p| p.id == GREEN_FOLIAGE)
        .unwrap()
        .revision += 1;
    assert_eq!(
        before.fingerprint(),
        CompilePlan::new(&definition, &plants, &library, profile())
            .unwrap()
            .fingerprint()
    );
    library
        .presets
        .iter_mut()
        .find(|p| p.id == DRY_FOLIAGE)
        .unwrap()
        .revision += 1;
    assert_ne!(
        before.fingerprint(),
        CompilePlan::new(&definition, &plants, &library, profile())
            .unwrap()
            .fingerprint()
    );
}
