#![allow(dead_code)]

use std::collections::BTreeSet;

use environment::*;
use vegetation::{VegetationCatalog, fixtures};
use world::{CellCoord, TerrainSurfaceId, WorldSpaceId};

pub const GRASS: ChannelId = ChannelId([1; 16]);
pub const FLOWERS: ChannelId = ChannelId([2; 16]);
pub const DRY: LayerId = LayerId([11; 16]);
pub const GREEN: LayerId = LayerId([12; 16]);
pub const CLEAR: LayerId = LayerId([13; 16]);
pub const SOIL: TerrainSurfaceId = TerrainSurfaceId([1; 16]);
pub const GREEN_SOIL: TerrainSurfaceId = TerrainSurfaceId([2; 16]);
pub const CELLS: [CellCoord; 2] = [CellCoord { x: 0, z: 0 }, CellCoord { x: 1, z: 0 }];

pub fn fixture() -> (
    EnvironmentDefinition,
    PresetLibrary,
    VegetationCatalog,
    CoverageSnapshot,
) {
    let library = environment::fixtures::meadow_library(SOIL, GREEN_SOIL, GRASS);
    let definition = EnvironmentDefinition {
        space: WorldSpaceId(1),
        revision: 1,
        cell_size: 8.0,
        mask_resolution: 17,
        surfaces: vec![SOIL, GREEN_SOIL],
        base_surface: SOIL,
        layers: library.presets[..3]
            .iter()
            .enumerate()
            .map(|(i, c)| Layer {
                id: [DRY, GREEN, CLEAR][i],
                revision: 1,
                name: c.name.clone(),
                preset: c.id,
                order: i as i32,
                seed: 42,
                enabled: true,
                opacity: 1.0,
                overrides: vec![],
            })
            .collect(),
    };
    let coverage = snapshot(&definition, &CELLS, |layer, x, z| {
        let hole = (1.0..=3.0).contains(&x) && (1.0..=3.0).contains(&z);
        if layer == DRY {
            return if hole { 0.0 } else { 1.0 };
        }
        if layer == GREEN {
            return ((x - 6.0) / 4.0).clamp(0.0, 1.0);
        }
        // A sampled painted mask, not the future editable road-curve implementation.
        let distance = (z - (4.0 + (x * 0.3).sin() * 0.7)).abs();
        ((1.25 - distance) / 0.5).clamp(0.0, 1.0)
    });
    (definition, library, fixtures::reference_catalog(), coverage)
}

pub fn ground(surface: TerrainSurfaceId) -> GroundTreatment {
    GroundTreatment {
        id: OutputId([1; 16]),
        strength: 1.0,
        surfaces: vec![SurfaceWeight {
            surface,
            weight: 1.0,
        }],
    }
}

pub fn profile() -> yarra_environment_compile::CompileProfile {
    yarra_environment_compile::CompileProfile {
        terrain_resolution: 17,
        vegetation_resolution: 16,
        ..Default::default()
    }
}

pub fn snapshot(
    definition: &EnvironmentDefinition,
    requested: &[CellCoord],
    value: impl Fn(LayerId, f64, f64) -> f64,
) -> CoverageSnapshot {
    let mut cells = BTreeSet::new();
    for cell in requested {
        for z in -1..=1 {
            for x in -1..=1 {
                cells.insert(CellCoord {
                    x: cell.x + x,
                    z: cell.z + z,
                });
            }
        }
    }
    let n = usize::from(definition.mask_resolution);
    let size = f64::from(definition.cell_size);
    CoverageSnapshot {
        space: definition.space,
        cells: cells
            .into_iter()
            .map(|cell| CoverageCell {
                cell,
                revision: 1,
                tiles: definition
                    .layers
                    .iter()
                    .map(|layer| CoverageTile {
                        layer: layer.id,
                        samples: (0..n * n)
                            .map(|i| {
                                let x = f64::from(cell.x) * size
                                    + (i % n) as f64 * size / (n - 1) as f64;
                                let z = f64::from(cell.z) * size
                                    + (i / n) as f64 * size / (n - 1) as f64;
                                (value(layer.id, x, z).clamp(0.0, 1.0) * 255.0).round() as u8
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub fn ground_weights(
    ground: &yarra_environment_compile::CompiledGround,
    x: usize,
    z: usize,
) -> Vec<(TerrainSurfaceId, u8)> {
    ground
        .surfaces
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let value = if ground.surfaces.len() == 1 {
                255
            } else {
                let page = &ground.weight_pages[i / 4];
                page.rgba[(z * usize::from(page.resolution) + x) * 4 + i % 4]
            };
            (*id, value)
        })
        .filter(|(_, value)| *value != 0)
        .collect()
}

pub fn foliage(library: &mut PresetLibrary, id: PresetId) -> &mut VegetationTreatment {
    let PresetKind::Foliage(v) = &mut library
        .presets
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .kind
    else {
        panic!()
    };
    v
}
pub fn soil(library: &mut PresetLibrary, id: PresetId) -> &mut GroundTreatment {
    let PresetKind::Ground(v) = &mut library
        .presets
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .kind
    else {
        panic!()
    };
    v
}
pub fn children(library: &mut PresetLibrary, id: PresetId) -> &mut Vec<PresetUse> {
    let PresetKind::Composition(v) = &mut library
        .presets
        .iter_mut()
        .find(|p| p.id == id)
        .unwrap()
        .kind
    else {
        panic!()
    };
    v
}
pub fn append(library: &mut PresetLibrary, parent: PresetId, id: PresetId, kind: PresetKind) {
    library.presets.push(Preset {
        id,
        revision: 1,
        name: "Extra".into(),
        kind,
    });
    children(library, parent).push(PresetUse {
        id: PresetUseId(id.0),
        name: "Extra use".into(),
        preset: id,
        overrides: vec![],
    });
}
