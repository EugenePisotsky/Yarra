mod support;

use std::collections::HashSet;

use environment::fixtures::{
    CLEAR_GRASS, CLEAR_USE, CLEARING, DRY_GROUND, GREEN_GROUND, GROUND_USE,
};

use environment::*;
use support::*;
use vegetation::{GrassTypeId, VegetationFieldPage, VegetationSurfaceField, fixtures};
use world::{CellCoord, TerrainSurfaceId, WorldSpaceId};
use yarra_environment_compile::{CompileError, CompilePlan, CompiledCell};

fn meadow() -> GrassTypeId {
    fixtures::meadow().id()
}

fn tufts() -> GrassTypeId {
    fixtures::tufts().id()
}

/// A grass type's coverage at vegetation sample (x, z), zero where it does not grow.
fn coverage(cell: &CompiledCell, grass: GrassTypeId, x: usize, z: usize) -> u8 {
    cell.vegetation
        .fields
        .iter()
        .find(|f| f.grass == grass)
        .map_or(0, |f| f.coverage[z * usize::from(f.resolution) + x])
}

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
    reordered_plants.surfaces.reverse();
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
fn grass_grows_on_its_surfaces_and_clearings_remove_it() {
    let (mut definition, library, plants, source) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let cells = plan.compile_cells(&CELLS, &source).unwrap();

    // The unpainted patch shows the dry base ground, and its tufts.
    assert_eq!(coverage(&cells[0], tufts(), 3, 3), 255);
    assert_eq!(coverage(&cells[0], meadow(), 3, 3), 0);

    // Green meadow ground at the far side of the gradient grows meadow grass only.
    assert_eq!(coverage(&cells[1], meadow(), 15, 15), 255);
    assert_eq!(coverage(&cells[1], tufts(), 15, 15), 0);

    // Inside the winding clearing at x=8.25, z=4.25 nothing grows.
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
fn grass_follows_the_share_of_its_surfaces() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers[0].enabled = false;
    definition.layers[2].enabled = false;
    soil(&mut library, GREEN_GROUND).strength = 0.5;
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let compile = |plants: &vegetation::VegetationCatalog| {
        CompilePlan::new(&definition, plants, &library, profile())
            .unwrap()
            .compile_cells(&CELLS, &source)
            .unwrap()
    };
    let cells = compile(&plants);
    assert_eq!(coverage(&cells[0], meadow(), 5, 5), 128);
    assert_eq!(coverage(&cells[0], tufts(), 5, 5), 128);
    let mut half = plants.clone();
    half.surfaces[1].grass[0].amount = 0.5;
    assert_eq!(coverage(&compile(&half)[0], meadow(), 5, 5), 64);
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
    assert_eq!(cells[0].ground.surfaces, vec![SOIL, GREEN_SOIL]);
    assert_eq!(cells[1].ground.surfaces, vec![GREEN_SOIL]);

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
fn only_grass_channel_exclusions_clear_grass_and_none_repaint_ground() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers[1].enabled = false;
    children(&mut library, CLEARING).retain(|c| c.id != GROUND_USE);
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let compile = |library: &PresetLibrary| {
        CompilePlan::new(&definition, &plants, library, profile())
            .unwrap()
            .compile_cells(&CELLS, &source)
            .unwrap()
    };

    for cell in compile(&library) {
        assert_eq!(cell.ground.surfaces, vec![SOIL]);
        assert!(cell.vegetation.fields.is_empty());
    }

    let PresetKind::Exclusion(exclusion) = &mut library
        .presets
        .iter_mut()
        .find(|p| p.id == CLEAR_GRASS)
        .unwrap()
        .kind
    else {
        panic!()
    };
    exclusion.channel = FLOWERS;

    for cell in compile(&library) {
        assert_eq!(cell.ground.surfaces, vec![SOIL]);
        assert_eq!(cell.vegetation.fields.len(), 1);
        assert!(cell.vegetation.fields[0].coverage.iter().all(|&v| v == 255));
    }
}

#[test]
fn partial_exclusions_thin_grass_without_moving_roots() {
    let (mut definition, library, plants, _) = fixture();
    definition.layers[1].enabled = false;
    definition.layers[2].opacity = 0.5;
    let source = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let mut full = definition.clone();
    full.layers[2].enabled = false;
    let full_plan = CompilePlan::new(&full, &plants, &library, profile()).unwrap();
    let thin_plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let thin = thin_plan.compile_cells(&CELLS, &source).unwrap();
    assert!(
        thin[0]
            .vegetation
            .fields
            .iter()
            .all(|f| f.coverage.iter().all(|&v| v == 128))
    );
    let roots = |plan: &CompilePlan| {
        plan.compile_cells(&CELLS, &source)
            .unwrap()
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
            .map(|p| (p.grass, p.seed, p.root.map(f32::to_bits)))
            .collect::<HashSet<_>>()
    };
    let full_roots = roots(&full_plan);
    let thin_roots = roots(&thin_plan);
    assert!(!thin_roots.is_empty());
    assert!(thin_roots.len() < full_roots.len());
    assert!(thin_roots.is_subset(&full_roots));
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
        Err(CompileError::Coverage(
            environment::CoverageError::CoordinateRange
        ))
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
        Err(CompileError::Coverage(
            environment::CoverageError::UnloadedCell(_)
        ))
    ));
    let mut missing_halo = source.clone();
    missing_halo
        .cells
        .retain(|c| c.cell != CellCoord { x: -1, z: -1 });
    assert!(matches!(
        plan.compile_cells(&CELLS, &missing_halo),
        Err(CompileError::Coverage(
            environment::CoverageError::UnloadedCell(_)
        ))
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
        Err(CompileError::Coverage(
            environment::CoverageError::BorderMismatch { .. }
        ))
    ));
    let mut malformed = source.clone();
    malformed.cells[0].tiles[0].samples.pop();
    assert!(matches!(
        plan.compile_cells(&CELLS, &malformed),
        Err(CompileError::Coverage(
            environment::CoverageError::InvalidTile(_)
        ))
    ));
    let mut duplicate = source.clone();
    let tile = duplicate.cells[0].tiles[0].clone();
    duplicate.cells[0].tiles.push(tile);
    assert!(matches!(
        plan.compile_cells(&CELLS, &duplicate),
        Err(CompileError::Coverage(
            environment::CoverageError::InvalidTile(_)
        ))
    ));
    assert!(matches!(
        plan.compile_cells(&[CELLS[0], CELLS[0]], &source),
        Err(CompileError::DuplicateCell(_))
    ));
    let mut wrong_world = source.clone();
    wrong_world.space = WorldSpaceId(2);
    assert!(matches!(
        plan.compile_cells(&CELLS, &wrong_world),
        Err(CompileError::Coverage(
            environment::CoverageError::WrongSpace
        ))
    ));
}

#[test]
fn absent_tiles_in_loaded_cells_mean_empty_but_unknown_cells_never_do() {
    let (definition, library, plants, mut source) = fixture();

    for cell in &mut source.cells {
        cell.tiles.clear();
    }

    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();

    for cell in plan.compile_cells(&CELLS, &source).unwrap() {
        // Unpainted ground is the base surface, with what grows on it.
        assert_eq!(cell.ground.surfaces, vec![SOIL]);
        assert_eq!(cell.vegetation.fields.len(), 1);
        assert_eq!(cell.vegetation.fields[0].grass, tufts());
    }
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

    // Eight surfaces fit one cell, four per RGBA weight page.
    let plan = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    let cells = plan.compile_cells(&CELLS, &source).unwrap();
    assert!(
        cells
            .iter()
            .any(|c| c.ground.surfaces.len() == 3 && c.ground.weight_pages.len() == 1)
    );

    // Past the budget a cell keeps its heaviest surfaces, with weights shared among them.
    let mut two = profile();
    two.max_surfaces_per_cell = 2;
    let plan = CompilePlan::new(&definition, &plants, &library, two).unwrap();

    for (narrow, full) in plan
        .compile_cells(&CELLS, &source)
        .unwrap()
        .iter()
        .zip(&cells)
    {
        assert!(narrow.ground.surfaces.len() <= 2);
        assert!(
            narrow
                .ground
                .surfaces
                .iter()
                .all(|s| full.ground.surfaces.contains(s))
        );

        if let [page] = &narrow.ground.weight_pages[..] {
            assert!(
                page.rgba
                    .chunks(4)
                    .all(|w| w[0] as u32 + w[1] as u32 == 255)
            );
        }
    }

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
            Err(CompileError::Budget(_)
                | CompileError::Coverage(environment::CoverageError::Budget(_)))
        ));
    }

    let mut limits = profile();
    limits.max_grass_types = 1;
    assert!(matches!(
        CompilePlan::new(&definition, &plants, &library, limits),
        Err(CompileError::Budget(_))
    ));
}

#[test]
fn malformed_definitions_libraries_and_catalogs_are_rejected() {
    let (definition, library, plants, _) = fixture();
    let mut invalid = definition.clone();
    invalid.layers[0].opacity = f32::NAN;
    assert!(invalid.validate(&library).is_err());
    let mut bad_library = library.clone();
    soil(&mut bad_library, DRY_GROUND).surfaces[0].weight = f32::INFINITY;
    assert!(definition.validate(&bad_library).is_err());
    invalid = definition.clone();
    invalid.layers[0].preset = PresetId([99; 16]);
    assert!(matches!(
        invalid.validate(&library),
        Err(ValidationError::Preset(_))
    ));
    let mut bad_plants = plants.clone();
    bad_plants.grass[0].height = [1.0, 0.5];
    assert!(matches!(
        CompilePlan::new(&definition, &bad_plants, &library, profile()),
        Err(CompileError::Catalog(_))
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
        Err(CompileError::Coverage(
            environment::CoverageError::UnloadedCell(_)
        ))
    ));
}

#[test]
fn repeated_preset_uses_keep_independent_overrides() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers.retain(|l| l.id == CLEAR);
    let second = PresetUseId([99; 16]);
    children(&mut library, CLEARING).push(PresetUse {
        id: second,
        name: "Second clearing".into(),
        preset: CLEAR_GRASS,
        overrides: vec![],
    });
    let baseline = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    definition.layers[0].overrides = vec![PresetOverride {
        path: vec![second],
        value: QuickValue::ExclusionInfluence(0.3),
    }];
    let second_only = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    definition.layers[0].overrides[0].path = vec![CLEAR_USE];
    let first_only = CompilePlan::new(&definition, &plants, &library, profile()).unwrap();
    assert_ne!(baseline.fingerprint(), second_only.fingerprint());
    assert_ne!(second_only.fingerprint(), first_only.fingerprint());
}

#[test]
fn plan_fingerprints_follow_reachable_presets_and_growth_but_not_looks() {
    let (mut definition, mut library, plants, _) = fixture();
    definition.layers.truncate(1);
    let fingerprint = |library: &PresetLibrary, plants: &vegetation::VegetationCatalog| {
        CompilePlan::new(&definition, plants, library, profile())
            .unwrap()
            .fingerprint()
    };
    let before = fingerprint(&library, &plants);
    library.revision += 1;
    library
        .presets
        .iter_mut()
        .find(|p| p.id == GREEN_GROUND)
        .unwrap()
        .revision += 1;
    assert_eq!(before, fingerprint(&library, &plants));
    library
        .presets
        .iter_mut()
        .find(|p| p.id == DRY_GROUND)
        .unwrap()
        .revision += 1;
    let after = fingerprint(&library, &plants);
    assert_ne!(before, after);

    // A look changes no coverage; what grows where does.
    let mut looks = plants.clone();
    looks.grass[0].colors.tip = [0.5, 0.4, 0.1];
    looks.grass[1].height = [0.2, 0.3];
    assert_eq!(after, fingerprint(&library, &looks));
    let mut growth = plants.clone();
    growth.surfaces[0].grass[0].amount = 0.4;
    assert_ne!(after, fingerprint(&library, &growth));
}
