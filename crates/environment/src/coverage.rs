//! Coverage seams: when a snapshot of painted masks can be compiled for some of its cells.
//! Neighbouring cells share their edge samples, so each cell's whole halo is loaded and every
//! shared edge and corner agrees.
use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;
use world::CellCoord;

use crate::{CoverageCell, CoverageSnapshot, EnvironmentDefinition, LayerId};

/// What one coverage check may load: cells a batch names, and mask samples in all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverageLimits {
    pub cells: usize,
    pub mask_samples: usize,
}

impl Default for CoverageLimits {
    fn default() -> Self {
        Self {
            cells: 64,
            mask_samples: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum CoverageError {
    #[error("coverage snapshot belongs to another world space")]
    WrongSpace,
    #[error("coverage budget exceeded: {0}")]
    Budget(&'static str),
    #[error("duplicate requested or loaded cell {0:?}")]
    DuplicateCell(CellCoord),
    #[error("coverage cell {0:?} is not loaded")]
    UnloadedCell(CellCoord),
    #[error("cell {0:?} has an invalid, duplicate or unknown coverage tile")]
    InvalidTile(CellCoord),
    #[error("coverage border mismatch between {cell:?} and {neighbour:?}, layer {layer:?}")]
    BorderMismatch {
        cell: CellCoord,
        neighbour: CellCoord,
        layer: LayerId,
    },
    #[error("cell coordinate has no representable dependency halo")]
    CoordinateRange,
}

/// A loaded cell's tiles by layer.
pub struct LoadedCoverageCell<'a> {
    pub source: &'a CoverageCell,
    pub tiles: BTreeMap<LayerId, &'a [u8]>,
}

/// A coverage snapshot's tiles by cell and layer, each checked to be one of the definition's
/// layers at its mask resolution.
pub struct CoverageIndex<'a> {
    cells: BTreeMap<CellCoord, LoadedCoverageCell<'a>>,
    layers: Vec<LayerId>,
    resolution: usize,
}

impl<'a> CoverageIndex<'a> {
    pub fn new(
        definition: &EnvironmentDefinition,
        source: &'a CoverageSnapshot,
        limits: CoverageLimits,
    ) -> Result<Self, CoverageError> {
        if source.space != definition.space {
            return Err(CoverageError::WrongSpace);
        }

        if source.cells.len() > limits.cells.saturating_mul(9) {
            return Err(CoverageError::Budget("loaded coverage cells"));
        }

        let layers: Vec<_> = definition.layers.iter().map(|l| l.id).collect();
        let known: BTreeSet<_> = layers.iter().copied().collect();
        let resolution = usize::from(definition.mask_resolution);
        let mut cells = BTreeMap::new();
        let mut samples = 0usize;

        for cell in &source.cells {
            let mut tiles = BTreeMap::new();

            for tile in &cell.tiles {
                if !known.contains(&tile.layer)
                    || tile.samples.len() != resolution * resolution
                    || tiles.insert(tile.layer, tile.samples.as_slice()).is_some()
                {
                    return Err(CoverageError::InvalidTile(cell.cell));
                }

                samples = samples
                    .checked_add(tile.samples.len())
                    .ok_or(CoverageError::Budget("mask samples"))?;

                if samples > limits.mask_samples {
                    return Err(CoverageError::Budget("mask samples"));
                }
            }

            let loaded = LoadedCoverageCell {
                source: cell,
                tiles,
            };

            if cells.insert(cell.cell, loaded).is_some() {
                return Err(CoverageError::DuplicateCell(cell.cell));
            }
        }

        Ok(Self {
            cells,
            layers,
            resolution,
        })
    }

    /// Checks `cells` can be compiled: each and its halo loaded and agreeing with its
    /// neighbours along shared edges and corners.
    pub fn check(&self, cells: &[CellCoord], limits: CoverageLimits) -> Result<(), CoverageError> {
        if cells.len() > limits.cells {
            return Err(CoverageError::Budget("requested cells"));
        }

        let mut unique = BTreeSet::new();

        for &cell in cells {
            if !unique.insert(cell) {
                return Err(CoverageError::DuplicateCell(cell));
            }

            self.check_halo(cell)?;
        }

        Ok(())
    }

    pub fn check_halo(&self, cell: CellCoord) -> Result<(), CoverageError> {
        if !self.cells.contains_key(&cell) {
            return Err(CoverageError::UnloadedCell(cell));
        }

        let last = self.resolution - 1;

        for (dx, dz, neighbour) in halo(cell)? {
            if !self.cells.contains_key(&neighbour) {
                return Err(CoverageError::UnloadedCell(neighbour));
            }

            if dx == 0 && dz == 0 {
                continue;
            }

            // Shared edges and diagonal corners must agree even when a neighbour has no tile.
            let edge = |d: i32, t: usize| match d {
                -1 => (0, last),
                1 => (last, 0),
                _ => (t, t),
            };

            for &layer in &self.layers {
                for t in 0..self.resolution {
                    let (ax, bx) = edge(dx, t);
                    let (az, bz) = edge(dz, t);

                    if self.value(cell, layer, ax, az) != self.value(neighbour, layer, bx, bz) {
                        return Err(CoverageError::BorderMismatch {
                            cell,
                            neighbour,
                            layer,
                        });
                    }
                }
            }
        }

        Ok(())
    }

    /// A loaded cell and its tiles.
    pub fn cell(&self, cell: CellCoord) -> Option<&LoadedCoverageCell<'a>> {
        self.cells.get(&cell)
    }

    fn value(&self, cell: CellCoord, layer: LayerId, x: usize, z: usize) -> u8 {
        self.cells[&cell]
            .tiles
            .get(&layer)
            .map_or(0, |tile| tile[z * self.resolution + x])
    }

    /// A layer's coverage in a loaded cell, interpolated bilinearly, from 0 to 1.
    pub fn sample(&self, cell: CellCoord, layer: LayerId, uv: [f64; 2]) -> f64 {
        let last = self.resolution - 1;
        let x = uv[0] * last as f64;
        let z = uv[1] * last as f64;
        let x0 = x.floor() as usize;
        let z0 = z.floor() as usize;
        let x1 = (x0 + 1).min(last);
        let z1 = (z0 + 1).min(last);
        let tx = x - x0 as f64;
        let tz = z - z0 as f64;
        let a = f64::from(self.value(cell, layer, x0, z0)) * (1.0 - tx)
            + f64::from(self.value(cell, layer, x1, z0)) * tx;
        let b = f64::from(self.value(cell, layer, x0, z1)) * (1.0 - tx)
            + f64::from(self.value(cell, layer, x1, z1)) * tx;
        (a * (1.0 - tz) + b * tz) / 255.0
    }
}

/// A cell and its eight neighbours, each with its offset.
pub fn halo(cell: CellCoord) -> Result<Vec<(i32, i32, CellCoord)>, CoverageError> {
    let mut cells = Vec::with_capacity(9);

    for dz in -1..=1 {
        for dx in -1..=1 {
            let x = cell.x.checked_add(dx);
            let z = cell.z.checked_add(dz);
            let (Some(x), Some(z)) = (x, z) else {
                return Err(CoverageError::CoordinateRange);
            };
            cells.push((dx, dz, CellCoord { x, z }));
        }
    }

    Ok(cells)
}
