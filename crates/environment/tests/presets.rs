use world::TerrainSurfaceId;
use yarra_environment::fixtures::*;
use yarra_environment::*;
fn fixture() -> PresetLibrary {
    meadow_library(TerrainSurfaceId([1; 16]), TerrainSurfaceId([2; 16]))
}
fn children(lib: &mut PresetLibrary, id: PresetId) -> &mut Vec<PresetUse> {
    let PresetKind::Composition(v) = &mut lib.presets.iter_mut().find(|p| p.id == id).unwrap().kind
    else {
        panic!()
    };
    v
}
fn exclusion(lib: &mut PresetLibrary, id: PresetId) -> &mut Exclusion {
    let PresetKind::Exclusion(v) = &mut lib.presets.iter_mut().find(|p| p.id == id).unwrap().kind
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
            if let PresetKind::Exclusion(v) = u.kind {
                Some(v.strength)
            } else {
                None
            }
        })
        .collect()
}
#[test]
fn repeated_nested_uses_have_independent_overrides_and_stable_output_identities() {
    let mut lib = fixture();
    children(&mut lib, CLEARING).retain(|c| c.id == CLEAR_USE);
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
                preset: CLEARING,
                overrides: vec![PresetOverride {
                    path: vec![CLEAR_USE],
                    value: QuickValue::ExclusionInfluence(0.2),
                }],
            },
            PresetUse {
                id: b,
                name: "Edge".into(),
                preset: CLEARING,
                overrides: vec![PresetOverride {
                    path: vec![CLEAR_USE],
                    value: QuickValue::ExclusionInfluence(0.7),
                }],
            },
        ]),
    });
    lib.validate().unwrap();
    assert_eq!(density(&lib, root, &[]), vec![0.2, 0.7]);
    let edits = vec![PresetOverride {
        path: vec![a, CLEAR_USE],
        value: QuickValue::ExclusionInfluence(0.4),
    }];
    assert_eq!(density(&lib, root, &edits), vec![0.4, 0.7]);
    let before = lib.resolve(root, &edits).unwrap();
    assert_ne!(before.exclusions[0].id, before.exclusions[1].id);
    children(&mut lib, root).reverse();
    children(&mut lib, root)[0].name = "Renamed".into();
    lib.presets.reverse();
    assert_eq!(before, lib.resolve(root, &edits).unwrap());
}
#[test]
fn explicit_overrides_survive_default_changes_and_reset_resumes_inheritance() {
    let mut lib = fixture();
    let edits = vec![PresetOverride {
        path: vec![CLEAR_USE],
        value: QuickValue::ExclusionInfluence(1.0),
    }];
    exclusion(&mut lib, CLEAR_GRASS).strength = 0.3;
    assert_eq!(density(&lib, CLEARING, &edits), vec![1.0]);
    assert_eq!(density(&lib, CLEARING, &[]), vec![0.3]);
    children(&mut lib, CLEARING)[1]
        .overrides
        .push(PresetOverride {
            path: vec![],
            value: QuickValue::ExclusionInfluence(0.6),
        });
    assert_eq!(density(&lib, CLEARING, &[]), vec![0.6]);
    assert_eq!(density(&lib, CLEARING, &edits), vec![1.0]);
}
#[test]
fn invalid_override_targets_types_duplicates_and_values_are_rejected() {
    let lib = fixture();
    for edit in [
        PresetOverride {
            path: vec![PresetUseId([99; 16])],
            value: QuickValue::ExclusionInfluence(0.5),
        },
        PresetOverride {
            path: vec![GROUND_USE],
            value: QuickValue::ExclusionInfluence(0.5),
        },
        PresetOverride {
            path: vec![CLEAR_USE],
            value: QuickValue::ExclusionInfluence(f32::NAN),
        },
        PresetOverride {
            path: vec![CLEAR_USE],
            value: QuickValue::ExclusionInfluence(1.1),
        },
    ] {
        assert!(lib.resolve(CLEARING, &[edit]).is_err());
    }
    let edit = PresetOverride {
        path: vec![CLEAR_USE],
        value: QuickValue::ExclusionInfluence(0.5),
    };
    assert!(lib.resolve(CLEARING, &[edit.clone(), edit]).is_err());
    assert!(
        lib.resolve(
            CLEARING,
            &[PresetOverride {
                path: vec![],
                value: QuickValue::ExclusionInfluence(0.5)
            }]
        )
        .is_err()
    );
}
#[test]
fn missing_references_cycles_and_depth_are_rejected_even_if_unused() {
    let lib = fixture();
    let mut bad = lib.clone();
    children(&mut bad, CLEARING)[1].preset = PresetId([99; 16]);
    assert!(bad.validate().is_err());
    let mut bad = lib.clone();
    children(&mut bad, CLEARING)[1].preset = CLEARING;
    assert!(bad.validate().is_err());
    let mut bad = lib.clone();
    children(&mut bad, CLEARING)[1].preset = GREEN_MEADOW;
    children(&mut bad, GREEN_MEADOW)[0].preset = CLEARING;
    assert!(bad.validate().is_err());
    let mut deep = lib;
    let mut last = CLEAR_GRASS;
    for n in 0..=MAX_PRESET_DEPTH {
        let id = PresetId([100 + n as u8; 16]);
        deep.presets.push(Preset {
            id,
            revision: 1,
            name: "Nested".into(),
            kind: PresetKind::Composition(vec![PresetUse {
                id: CLEAR_USE,
                name: "Child".into(),
                preset: last,
                overrides: vec![],
            }]),
        });
        last = id;
    }
    assert!(deep.validate().is_err());
}
#[test]
fn expansion_and_duplicate_use_budgets_are_enforced() {
    let mut lib = fixture();
    let child = children(&mut lib, CLEARING)[1].clone();
    *children(&mut lib, CLEARING) = vec![child.clone(), child.clone()];
    assert!(lib.validate().is_err());
    *children(&mut lib, CLEARING) = (0..MAX_PRESET_CHILDREN)
        .map(|n| PresetUse {
            id: PresetUseId([n as u8; 16]),
            ..child.clone()
        })
        .collect();
    lib.validate().unwrap();
    let parent = Preset {
        id: PresetId([99; 16]),
        revision: 1,
        name: "Too many leaves".into(),
        kind: PresetKind::Composition(vec![
            PresetUse {
                preset: CLEARING,
                ..child.clone()
            },
            PresetUse {
                id: PresetUseId([99; 16]),
                ..child
            },
        ]),
    };
    lib.presets.push(parent);
    assert!(lib.validate().is_err());
}
#[test]
fn resolved_dependencies_only_include_reachable_presets() {
    let mut lib = fixture();
    let before = lib.resolve(CLEARING, &[]).unwrap();
    lib.revision += 1;
    lib.presets
        .iter_mut()
        .find(|p| p.id == GREEN_GROUND)
        .unwrap()
        .revision += 1;
    assert_eq!(before, lib.resolve(CLEARING, &[]).unwrap());
    lib.presets
        .iter_mut()
        .find(|p| p.id == CLEAR_GRASS)
        .unwrap()
        .revision += 1;
    assert_ne!(before, lib.resolve(CLEARING, &[]).unwrap());
}
#[test]
fn imported_mask_names_are_checked_with_their_layer() {
    let lib = fixture();
    let layer = |mask: Option<&str>| EnvironmentDefinition {
        space: world::WorldSpaceId(1),
        revision: 1,
        cell_size: 32.0,
        mask_resolution: 33,
        surfaces: vec![TerrainSurfaceId([1; 16]), TerrainSurfaceId([2; 16])],
        base_surface: TerrainSurfaceId([1; 16]),
        layers: vec![Layer {
            id: LayerId([1; 16]),
            revision: 1,
            name: "Sand".into(),
            preset: DRY_MEADOW,
            overrides: vec![],
            order: 0,
            seed: 1,
            enabled: true,
            opacity: 1.0,
            imported_mask: mask.map(Into::into),
        }],
    };
    for mask in [None, Some("sand"), Some("rock_2"), Some(&*"a".repeat(32))] {
        assert!(layer(mask).validate_layers(&lib).is_ok(), "{mask:?}");
    }
    for mask in ["", "Sand", "2rock", "wet ness", "_sand", &"a".repeat(33)] {
        assert!(layer(Some(mask)).validate_layers(&lib).is_err(), "{mask:?}");
    }
}
