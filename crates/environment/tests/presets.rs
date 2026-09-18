use world::TerrainSurfaceId;
use yarra_environment::fixtures::*;
use yarra_environment::*;
fn fixture() -> (PresetLibrary, vegetation::VegetationCatalog) {
    (
        meadow_library(
            TerrainSurfaceId([1; 16]),
            TerrainSurfaceId([2; 16]),
            ChannelId([1; 16]),
        ),
        vegetation::fixtures::reference_catalog(),
    )
}
fn children(lib: &mut PresetLibrary, id: PresetId) -> &mut Vec<PresetUse> {
    let PresetKind::Composition(v) = &mut lib.presets.iter_mut().find(|p| p.id == id).unwrap().kind
    else {
        panic!()
    };
    v
}
fn foliage(lib: &mut PresetLibrary, id: PresetId) -> &mut VegetationTreatment {
    let PresetKind::Foliage(v) = &mut lib.presets.iter_mut().find(|p| p.id == id).unwrap().kind
    else {
        panic!()
    };
    v
}
fn density(lib: &PresetLibrary, root: PresetId, overrides: &[PresetOverride]) -> Vec<f32> {
    lib.uses(root, overrides)
        .unwrap()
        .into_iter()
        .filter_map(|u| {
            if let PresetKind::Foliage(v) = u.kind {
                Some(v.density)
            } else {
                None
            }
        })
        .collect()
}
#[test]
fn repeated_nested_uses_have_independent_overrides_and_stable_output_identities() {
    let (mut lib, plants) = fixture();
    foliage(&mut lib, DRY_FOLIAGE).blend = VegetationBlend::Add;
    children(&mut lib, DRY_MEADOW).retain(|c| c.id == FOLIAGE_USE);
    let root = PresetId([90; 16]);
    let a = PresetUseId([10; 16]);
    let b = PresetUseId([11; 16]);
    lib.presets.push(Preset {
        id: root,
        revision: 1,
        name: "Forest".into(),
        kind: PresetKind::Composition(vec![
            PresetUse {
                id: a,
                name: "Understory".into(),
                preset: DRY_MEADOW,
                overrides: vec![PresetOverride {
                    path: vec![FOLIAGE_USE],
                    value: QuickValue::FoliageDensity(0.2),
                }],
            },
            PresetUse {
                id: b,
                name: "Edge".into(),
                preset: DRY_MEADOW,
                overrides: vec![PresetOverride {
                    path: vec![FOLIAGE_USE],
                    value: QuickValue::FoliageDensity(0.7),
                }],
            },
        ]),
    });
    lib.validate(&plants).unwrap();
    assert_eq!(density(&lib, root, &[]), vec![0.2, 0.7]);
    let edits = vec![PresetOverride {
        path: vec![a, FOLIAGE_USE],
        value: QuickValue::FoliageDensity(0.4),
    }];
    assert_eq!(density(&lib, root, &edits), vec![0.4, 0.7]);
    let before = lib.resolve(root, &edits).unwrap();
    assert_ne!(before.vegetation[0].id, before.vegetation[1].id);
    children(&mut lib, root).reverse();
    children(&mut lib, root)[0].name = "Renamed".into();
    lib.presets.reverse();
    assert_eq!(before, lib.resolve(root, &edits).unwrap());
}
#[test]
fn explicit_overrides_survive_default_changes_and_reset_resumes_inheritance() {
    let (mut lib, _) = fixture();
    let edits = vec![PresetOverride {
        path: vec![FOLIAGE_USE],
        value: QuickValue::FoliageDensity(1.0),
    }];
    foliage(&mut lib, DRY_FOLIAGE).density = 0.3;
    assert_eq!(density(&lib, DRY_MEADOW, &edits), vec![1.0]);
    assert_eq!(density(&lib, DRY_MEADOW, &[]), vec![0.3]);
    children(&mut lib, DRY_MEADOW)[1]
        .overrides
        .push(PresetOverride {
            path: vec![],
            value: QuickValue::FoliageDensity(0.6),
        });
    assert_eq!(density(&lib, DRY_MEADOW, &[]), vec![0.6]);
    assert_eq!(density(&lib, DRY_MEADOW, &edits), vec![1.0]);
}
#[test]
fn invalid_override_targets_types_duplicates_and_values_are_rejected() {
    let (lib, _) = fixture();
    for edit in [
        PresetOverride {
            path: vec![PresetUseId([99; 16])],
            value: QuickValue::FoliageDensity(0.5),
        },
        PresetOverride {
            path: vec![GROUND_USE],
            value: QuickValue::FoliageDensity(0.5),
        },
        PresetOverride {
            path: vec![FOLIAGE_USE],
            value: QuickValue::FoliageDensity(f32::NAN),
        },
        PresetOverride {
            path: vec![FOLIAGE_USE],
            value: QuickValue::FoliageDensity(1.1),
        },
    ] {
        assert!(lib.resolve(DRY_MEADOW, &[edit]).is_err());
    }
    let edit = PresetOverride {
        path: vec![FOLIAGE_USE],
        value: QuickValue::FoliageSeed(42),
    };
    assert!(lib.resolve(DRY_MEADOW, &[edit.clone(), edit]).is_err());
    assert!(
        lib.resolve(
            DRY_MEADOW,
            &[PresetOverride {
                path: vec![],
                value: QuickValue::FoliageDensity(0.5)
            }]
        )
        .is_err()
    );
}
#[test]
fn missing_references_cycles_and_depth_are_rejected_even_if_unused() {
    let (lib, plants) = fixture();
    let mut bad = lib.clone();
    children(&mut bad, DRY_MEADOW)[1].preset = PresetId([99; 16]);
    assert!(bad.validate(&plants).is_err());
    let mut bad = lib.clone();
    children(&mut bad, DRY_MEADOW)[1].preset = DRY_MEADOW;
    assert!(bad.validate(&plants).is_err());
    let mut bad = lib.clone();
    children(&mut bad, DRY_MEADOW)[1].preset = GREEN_MEADOW;
    children(&mut bad, GREEN_MEADOW)[1].preset = DRY_MEADOW;
    assert!(bad.validate(&plants).is_err());
    let mut deep = lib;
    let mut last = DRY_FOLIAGE;
    for n in 0..=MAX_PRESET_DEPTH {
        let id = PresetId([100 + n as u8; 16]);
        deep.presets.push(Preset {
            id,
            revision: 1,
            name: "Nested".into(),
            kind: PresetKind::Composition(vec![PresetUse {
                id: FOLIAGE_USE,
                name: "Child".into(),
                preset: last,
                overrides: vec![],
            }]),
        });
        last = id;
    }
    assert!(deep.validate(&plants).is_err());
}
#[test]
fn expansion_and_duplicate_use_budgets_are_enforced() {
    let (mut lib, plants) = fixture();
    foliage(&mut lib, DRY_FOLIAGE).blend = VegetationBlend::Add;
    let child = children(&mut lib, DRY_MEADOW)[1].clone();
    *children(&mut lib, DRY_MEADOW) = vec![child.clone(), child.clone()];
    assert!(lib.validate(&plants).is_err());
    *children(&mut lib, DRY_MEADOW) = (0..MAX_PRESET_CHILDREN)
        .map(|n| PresetUse {
            id: PresetUseId([n as u8; 16]),
            ..child.clone()
        })
        .collect();
    lib.validate(&plants).unwrap();
    let parent = Preset {
        id: PresetId([99; 16]),
        revision: 1,
        name: "Too many leaves".into(),
        kind: PresetKind::Composition(vec![
            PresetUse {
                preset: DRY_MEADOW,
                ..child.clone()
            },
            PresetUse {
                id: PresetUseId([99; 16]),
                ..child
            },
        ]),
    };
    lib.presets.push(parent);
    assert!(lib.validate(&plants).is_err());
}
#[test]
fn resolved_dependencies_only_include_reachable_presets() {
    let (mut lib, _) = fixture();
    let before = lib.resolve(DRY_MEADOW, &[]).unwrap();
    lib.revision += 1;
    lib.presets
        .iter_mut()
        .find(|p| p.id == GREEN_FOLIAGE)
        .unwrap()
        .revision += 1;
    assert_eq!(before, lib.resolve(DRY_MEADOW, &[]).unwrap());
    lib.presets
        .iter_mut()
        .find(|p| p.id == DRY_FOLIAGE)
        .unwrap()
        .revision += 1;
    assert_ne!(before, lib.resolve(DRY_MEADOW, &[]).unwrap());
}
