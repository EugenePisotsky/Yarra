use super::*;
use crate::{Layer, PresetId};
fn definition() -> EnvironmentDefinition {
    EnvironmentDefinition {
        space: world::WorldSpaceId(1),
        revision: 1,
        cell_size: 8.0,
        mask_resolution: 17,
        surfaces: vec![],
        base_surface: world::TerrainSurfaceId([1; 16]),
        layers: vec![Layer {
            id: LayerId([1; 16]),
            revision: 1,
            name: "Test".into(),
            preset: PresetId([1; 16]),
            order: 0,
            seed: 1,
            enabled: true,
            opacity: 1.0,
            overrides: vec![],
        }],
    }
}
fn brush() -> CoverageBrush {
    CoverageBrush {
        radius: 3.0,
        falloff: 0.65,
        strength: 0.5,
        operation: BrushOperation::Paint,
    }
}
fn empty(cell: CellCoord) -> CoverageCell {
    CoverageCell {
        cell,
        revision: 1,
        tiles: vec![],
    }
}
#[test]
fn continuous_stroke_is_independent_of_event_frequency_and_dwell() {
    let def = definition();
    let layer = def.layers[0].id;
    let before = empty(CellCoord { x: 0, z: 0 });
    let from = [1.0, 2.0];
    let to = [7.0, 6.0];
    let midpoint = [4.0, 4.0];
    let entire = brush()
        .apply_segment(&def, layer, &before, &before, from, to)
        .unwrap();
    let first = brush()
        .apply_segment(&def, layer, &before, &before, from, midpoint)
        .unwrap();
    let split = brush()
        .apply_segment(&def, layer, &before, &first, midpoint, to)
        .unwrap();
    assert_eq!(entire, split);
    assert_eq!(
        entire,
        brush()
            .apply_segment(&def, layer, &before, &entire, to, to)
            .unwrap()
    );
    let second_stroke = brush()
        .apply_segment(&def, layer, &entire, &entire, from, to)
        .unwrap();
    assert!(
        second_stroke.tiles[0]
            .samples
            .iter()
            .zip(&entire.tiles[0].samples)
            .any(|(a, b)| a > b)
    );
}
#[test]
fn neighboring_positive_and_negative_cells_share_identical_endpoints() {
    let def = definition();
    let layer = def.layers[0].id;
    let cells = brush()
        .cells(def.cell_size, [-2.0, -1.0], [2.0, 1.0])
        .unwrap();
    assert_eq!(cells.len(), 4);
    let results = cells
        .into_iter()
        .map(|coord| {
            let cell = empty(coord);
            (
                coord,
                brush()
                    .apply_segment(&def, layer, &cell, &cell, [-2.0, -1.0], [2.0, 1.0])
                    .unwrap(),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let n = usize::from(def.mask_resolution);
    for z in [-1, 0] {
        for row in 0..n {
            assert_eq!(
                results[&CellCoord { x: -1, z }].tiles[0].samples[row * n + n - 1],
                results[&CellCoord { x: 0, z }].tiles[0].samples[row * n]
            );
        }
    }
    for x in [-1, 0] {
        for col in 0..n {
            assert_eq!(
                results[&CellCoord { x, z: -1 }].tiles[0].samples[(n - 1) * n + col],
                results[&CellCoord { x, z: 0 }].tiles[0].samples[col]
            );
        }
    }
}
#[test]
fn erase_preserves_other_layers_and_prunes_zero_tiles() {
    let def = definition();
    let layer = def.layers[0].id;
    let before = CoverageCell {
        cell: CellCoord { x: 0, z: 0 },
        revision: 1,
        tiles: vec![
            CoverageTile {
                layer,
                samples: vec![200; 289],
            },
            CoverageTile {
                layer: LayerId([2; 16]),
                samples: vec![123; 289],
            },
        ],
    };
    let erase = CoverageBrush {
        radius: 12.0,
        strength: 1.0,
        falloff: 0.0,
        operation: BrushOperation::Erase,
    };
    let result = erase
        .apply_segment(&def, layer, &before, &before, [4.0, 4.0], [4.0, 4.0])
        .unwrap();
    assert_eq!(result.tiles, vec![before.tiles[1].clone()]);
}
#[test]
fn invalid_and_unbounded_strokes_are_rejected() {
    assert!(brush().cells(0.0, [0.0, 0.0], [1.0, 1.0]).is_err());
    assert!(brush().cells(8.0, [0.0, 0.0], [10000.0, 10000.0]).is_err());
    assert!(brush().cells(8.0, [f64::NAN, 0.0], [1.0, 1.0]).is_err());
    let def = definition();
    let before = CoverageCell {
        cell: CellCoord { x: 0, z: 0 },
        revision: 1,
        tiles: vec![CoverageTile {
            layer: def.layers[0].id,
            samples: vec![255; 2],
        }],
    };
    assert!(
        brush()
            .apply_segment(
                &def,
                def.layers[0].id,
                &before,
                &before,
                [0.0, 0.0],
                [1.0, 1.0]
            )
            .is_err()
    );
}

#[test]
fn distant_non_integer_grids_still_share_exact_border_samples() {
    let mut def = definition();
    def.cell_size = 33.3;
    let x = 1_000_000;
    let z = -1_000_000;
    let left = empty(CellCoord { x: x - 1, z });
    let right = empty(CellCoord { x, z });
    let point = [
        f64::from(x) * f64::from(def.cell_size),
        f64::from(z) * f64::from(def.cell_size) + 10.0,
    ];
    let left = brush()
        .apply_segment(&def, def.layers[0].id, &left, &left, point, point)
        .unwrap();
    let right = brush()
        .apply_segment(&def, def.layers[0].id, &right, &right, point, point)
        .unwrap();
    let n = usize::from(def.mask_resolution);
    for row in 0..n {
        assert_eq!(
            left.tiles[0].samples[row * n + n - 1],
            right.tiles[0].samples[row * n]
        );
    }
}
