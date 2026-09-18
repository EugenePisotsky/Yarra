#[path = "support/roads.rs"]
mod road_support;
mod support;
use environment::*;
use support::*;
use world::{AssetId, CellCoord, StaticObjectInstance};
use yarra_environment_compile::{CompilePlan, TerrainSource, TerrainSourceCell};
const COLLECTION: PresetId = PresetId([71; 16]);
const TREES: LayerId = LayerId([72; 16]);
fn add_collection(d: &mut EnvironmentDefinition, l: &mut PresetLibrary) {
    l.presets.push(Preset {
        id: COLLECTION,
        revision: 1,
        name: "Trees".into(),
        kind: PresetKind::AssetCollection(AssetCollection {
            id: OutputId([73; 16]),
            channel: ASSET_CHANNEL,
            assets: vec![
                CollectionAsset {
                    asset: AssetId([1; 32]),
                    weight: 1.0,
                    scale_min: 0.8,
                    scale_max: 1.2,
                },
                CollectionAsset {
                    asset: AssetId([2; 32]),
                    weight: 3.0,
                    scale_min: 1.1,
                    scale_max: 1.4,
                },
            ],
            spacing: 1.0,
            density: 1.0,
            seed: 124,
            max_slope_degrees: 45.0,
            road_clearance: 1.0,
        }),
    });
    d.layers.push(Layer {
        id: TREES,
        revision: 1,
        name: "Forest".into(),
        preset: COLLECTION,
        overrides: vec![],
        order: 10,
        seed: 78,
        enabled: true,
        opacity: 1.0,
    });
}
fn collection(l: &mut PresetLibrary) -> &mut AssetCollection {
    match &mut l
        .presets
        .iter_mut()
        .find(|p| p.id == COLLECTION)
        .unwrap()
        .kind
    {
        PresetKind::AssetCollection(c) => c,
        _ => unreachable!(),
    }
}
#[test]
fn neighbours_negative_coordinates_batch_order_and_density_are_stable() {
    let (mut d, mut l, p, _) = fixture();
    d.layers.clear();
    add_collection(&mut d, &mut l);
    let cells = [
        CellCoord { x: -1, z: 0 },
        CellCoord::ZERO,
        CellCoord { x: 1, z: 0 },
    ];
    let c = snapshot(&d, &cells, |_, _, _| 1.0);
    let plan = CompilePlan::new(&d, &p, &l, profile()).unwrap();
    let batch = plan.compile_cells(&cells, &c).unwrap();
    assert_eq!(
        batch,
        plan.compile_cells(&[cells[2], cells[0], cells[1]], &c)
            .unwrap()
    );
    let mut world = vec![];
    for cell in &batch {
        assert_eq!(*cell, plan.compile_cells(&[cell.cell], &c).unwrap()[0]);
        for o in &cell.objects {
            assert!(o.generated);
            assert!(
                (0.0..8.0).contains(&o.translation[0]) && (0.0..8.0).contains(&o.translation[2])
            );
            world.push((
                o.id,
                [
                    cell.cell.x as f32 * 8.0 + o.translation[0],
                    o.translation[2],
                ],
            ));
        }
    }
    assert!(world.len() > 30);
    for (i, (id, a)) in world.iter().enumerate() {
        for (other, b) in &world[i + 1..] {
            assert_ne!(id, other);
            assert!((a[0] - b[0]).hypot(a[1] - b[1]) >= 1.0 - 1e-5);
        }
    }
    collection(&mut l).density = 0.35;
    let thin = CompilePlan::new(&d, &p, &l, profile())
        .unwrap()
        .compile_cells(&cells, &c)
        .unwrap();
    let all: Vec<_> = batch.iter().flat_map(|c| &c.objects).collect();
    let remaining: Vec<_> = thin.iter().flat_map(|c| &c.objects).collect();
    assert!(!remaining.is_empty() && remaining.len() < all.len());
    for object in remaining {
        assert!(all.contains(&object));
    }
    collection(&mut l).density = 1.0;
    collection(&mut l).assets.reverse();
    assert_eq!(
        batch,
        CompilePlan::new(&d, &p, &l, profile())
            .unwrap()
            .compile_cells(&cells, &c)
            .unwrap()
    );
}
#[test]
fn paint_holes_exclusions_disabling_and_composition_overrides() {
    let (mut d, mut l, p, _) = fixture();
    d.layers.clear();
    add_collection(&mut d, &mut l);
    let group = PresetId([79; 16]);
    let use_id = PresetUseId([80; 16]);
    l.presets.push(Preset {
        id: group,
        revision: 1,
        name: "Woodland".into(),
        kind: PresetKind::Composition(vec![PresetUse {
            id: use_id,
            name: "Trees".into(),
            preset: COLLECTION,
            overrides: vec![],
        }]),
    });
    d.layers[0].preset = group;
    let c = snapshot(&d, &CELLS, |_, x, _| if x < 4.0 { 0.0 } else { 1.0 });
    let objects = CompilePlan::new(&d, &p, &l, profile())
        .unwrap()
        .compile_cells(&CELLS, &c)
        .unwrap();
    assert!(!objects[1].objects.is_empty());
    assert!(objects[0].objects.iter().all(|o| o.translation[0] >= 3.5)); // interpolated mask edge
    d.layers[0].overrides = vec![PresetOverride {
        path: vec![use_id],
        value: QuickValue::CollectionDensity(0.0),
    }];
    assert!(
        CompilePlan::new(&d, &p, &l, profile())
            .unwrap()
            .compile_cells(&CELLS, &c)
            .unwrap()
            .iter()
            .all(|c| c.objects.is_empty())
    );
    d.layers[0].overrides.clear();
    let exclusion = PresetId([81; 16]);
    l.presets.push(Preset {
        id: exclusion,
        revision: 1,
        name: "Clear trees".into(),
        kind: PresetKind::Exclusion(Exclusion {
            id: OutputId([82; 16]),
            channel: ASSET_CHANNEL,
            strength: 1.0,
        }),
    });
    let mut clear = d.layers[0].clone();
    clear.id = LayerId([83; 16]);
    clear.preset = exclusion;
    clear.order = 11;
    d.layers.push(clear);
    let c = snapshot(&d, &CELLS, |_, _, _| 1.0);
    assert!(
        CompilePlan::new(&d, &p, &l, profile())
            .unwrap()
            .compile_cells(&CELLS, &c)
            .unwrap()
            .iter()
            .all(|c| c.objects.is_empty())
    );
    d.layers[1].enabled = false;
    assert!(
        !CompilePlan::new(&d, &p, &l, profile())
            .unwrap()
            .compile_cells(&CELLS, &c)
            .unwrap()[0]
            .objects
            .is_empty()
    );
    d.layers[0].enabled = false;
    assert!(
        CompilePlan::new(&d, &p, &l, profile())
            .unwrap()
            .compile_cells(&CELLS, &c)
            .unwrap()
            .iter()
            .all(|c| c.objects.is_empty())
    );
}
fn terrain(cell: CellCoord, slope: f32) -> TerrainSource {
    let loaded_cells: Vec<_> = (-1..=1)
        .flat_map(|x| {
            (-1..=1).map(move |z| CellCoord {
                x: cell.x + x,
                z: cell.z + z,
            })
        })
        .collect();
    TerrainSource {
        space: world::WorldSpaceId(1),
        cell_size: 8.0,
        minimum_height: -32.0,
        maximum_height: 32.0,
        cells: loaded_cells
            .iter()
            .map(|&cell| TerrainSourceCell {
                flat: false,
                cell,
                revision: 1,
                resolution: 33,
                heights: (0..33 * 33)
                    .map(|i| slope * (cell.x as f32 * 8.0 + (i % 33) as f32 * 0.25))
                    .collect(),
            })
            .collect(),
        loaded_cells,
    }
}
#[test]
fn entire_road_corridor_and_clearance_are_empty_and_roots_follow_final_terrain() {
    let (mut d, mut l, p, _, mut roads) = road_support::cart_tracks();
    add_collection(&mut d, &mut l);
    road_support::straight(&mut roads);
    roads.loaded_bounds = roads::RoadCellBounds {
        minimum: CellCoord { x: -1, z: -1 },
        maximum: CellCoord { x: 2, z: 1 },
    };
    roads.profiles[0].relief.road_depth = 0.15;
    let c = snapshot(&d, &CELLS, |_, _, _| 1.0);
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    for cell in CELLS {
        let compiled = plan
            .compile_cell_with_terrain(cell, &c, &roads, &terrain(cell, 0.2), Default::default())
            .unwrap();
        assert!(!compiled.objects.is_empty());
        for o in &compiled.objects {
            assert!((o.translation[2] - 4.0).abs() >= 3.5 / 2.0 + 1.0);
            let sampled = compiled
                .terrain
                .as_ref()
                .unwrap()
                .sample([o.translation[0], o.translation[2]], 8.0);
            assert_eq!(o.translation[1], sampled.height);
            assert!(
                (o.translation[1] - 0.2 * (cell.x as f32 * 8.0 + o.translation[0])).abs() < 0.003
            );
        }
    }
    collection(&mut l).max_slope_degrees = 5.0;
    let plan = CompilePlan::new(&d, &p, &l, Default::default()).unwrap();
    assert!(
        plan.compile_cell_with_terrain(
            CellCoord::ZERO,
            &c,
            &roads,
            &terrain(CellCoord::ZERO, 0.2),
            Default::default()
        )
        .unwrap()
        .objects
        .is_empty()
    );
}
#[test]
fn invalid_collection_and_excessive_candidate_work_fail_explicitly() {
    let (mut d, mut l, p, _) = fixture();
    d.layers.clear();
    add_collection(&mut d, &mut l);
    collection(&mut l).assets[0].weight = f32::NAN;
    assert!(CompilePlan::new(&d, &p, &l, profile()).is_err());
    collection(&mut l).assets[0].weight = 1.0;
    collection(&mut l).spacing = 0.5;
    d.cell_size = 128.0;
    let c = snapshot(&d, &[CellCoord::ZERO], |_, _, _| 1.0);
    let err = CompilePlan::new(&d, &p, &l, profile())
        .unwrap()
        .compile_cells(&[CellCoord::ZERO], &c)
        .unwrap_err();
    assert!(err.to_string().contains("scatter candidates"));
}
#[test]
fn weighted_mix_and_variation_are_deterministic() {
    let (mut d, mut l, p, _) = fixture();
    d.layers.clear();
    add_collection(&mut d, &mut l);
    d.cell_size = 32.0;
    let c = snapshot(&d, &CELLS, |_, _, _| 1.0);
    let cells = CompilePlan::new(&d, &p, &l, profile())
        .unwrap()
        .compile_cells(&CELLS, &c)
        .unwrap();
    let objects: Vec<&StaticObjectInstance> = cells.iter().flat_map(|c| &c.objects).collect();
    assert!(objects.len() > 500);
    let second = objects
        .iter()
        .filter(|o| o.asset == world::AssetId([2; 32]))
        .count() as f32
        / objects.len() as f32;
    assert!((0.68..0.82).contains(&second), "mix {second}");
    for o in objects {
        let a = collection(&mut l)
            .assets
            .iter()
            .find(|a| a.asset == o.asset)
            .unwrap()
            .clone();
        assert!((a.scale_min..=a.scale_max).contains(&o.scale));
        assert!((0.0..std::f32::consts::TAU).contains(&o.yaw));
    }
}
