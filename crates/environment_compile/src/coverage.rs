use std::collections::{BTreeMap, BTreeSet};

use environment::{CoverageCell, CoverageSnapshot, LayerId};
use world::CellCoord;

use crate::{CompileError, CompilePlan};

pub(crate) struct LoadedCell<'a> {
    pub source: &'a CoverageCell,
    pub tiles: BTreeMap<LayerId, &'a [u8]>,
}

pub(crate) struct Coverage<'a> {
    pub cells: BTreeMap<CellCoord, LoadedCell<'a>>,
    resolution: usize,
}

impl<'a> Coverage<'a> {
    pub fn new(plan: &CompilePlan, source: &'a CoverageSnapshot) -> Result<Self, CompileError> {
        if source.space != plan.definition.space {
            return Err(CompileError::WrongSpace);
        }
        if source.cells.len() > plan.profile.max_cells_per_batch.saturating_mul(9) {
            return Err(CompileError::Budget("loaded coverage cells"));
        }
        let layers: BTreeSet<_> = plan.definition.layers.iter().map(|l| l.id).collect();
        let resolution = usize::from(plan.definition.mask_resolution);
        let mut cells = BTreeMap::new();
        let mut samples = 0usize;
        for cell in &source.cells {
            let mut tiles = BTreeMap::new();
            for tile in &cell.tiles {
                if !layers.contains(&tile.layer)
                    || tile.samples.len() != resolution * resolution
                    || tiles.insert(tile.layer, tile.samples.as_slice()).is_some()
                {
                    return Err(CompileError::InvalidTile(cell.cell));
                }
                samples = samples
                    .checked_add(tile.samples.len())
                    .ok_or(CompileError::Budget("mask samples"))?;
                if samples > plan.profile.max_mask_samples {
                    return Err(CompileError::Budget("mask samples"));
                }
            }
            if cells
                .insert(
                    cell.cell,
                    LoadedCell {
                        source: cell,
                        tiles,
                    },
                )
                .is_some()
            {
                return Err(CompileError::DuplicateCell(cell.cell));
            }
        }
        Ok(Self { cells, resolution })
    }

    pub fn check_halo(&self, plan: &CompilePlan, cell: CellCoord) -> Result<(), CompileError> {
        if !self.cells.contains_key(&cell) {
            return Err(CompileError::UnloadedCell(cell));
        }
        let last = self.resolution - 1;
        for (dx, dz, neighbour) in halo(cell)? {
            if !self.cells.contains_key(&neighbour) {
                return Err(CompileError::UnloadedCell(neighbour));
            }
            if dx == 0 && dz == 0 {
                continue;
            }
            // Shared edges and diagonal corners must agree even when a neighbour has no tile.
            for layer in &plan.definition.layers {
                for t in 0..self.resolution {
                    let ax = match dx {
                        -1 => 0,
                        1 => last,
                        _ => t,
                    };
                    let bx = match dx {
                        -1 => last,
                        1 => 0,
                        _ => t,
                    };
                    let az = match dz {
                        -1 => 0,
                        1 => last,
                        _ => t,
                    };
                    let bz = match dz {
                        -1 => last,
                        1 => 0,
                        _ => t,
                    };
                    if self.value(cell, layer.id, ax, az) != self.value(neighbour, layer.id, bx, bz)
                    {
                        return Err(CompileError::BorderMismatch {
                            cell,
                            neighbour,
                            layer: layer.id,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn value(&self, cell: CellCoord, layer: LayerId, x: usize, z: usize) -> u8 {
        self.cells[&cell]
            .tiles
            .get(&layer)
            .map_or(0, |tile| tile[z * self.resolution + x])
    }

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

    pub fn fingerprint(
        &self,
        plan: &CompilePlan,
        cell: CellCoord,
    ) -> Result<[u8; 32], CompileError> {
        let mut hash = blake3::Hasher::new();
        hash.update(&plan.fingerprint);
        hash.update(&cell.x.to_le_bytes());
        hash.update(&cell.z.to_le_bytes());
        for (_, _, neighbour) in halo(cell)? {
            let source = &self.cells[&neighbour];
            // BTreeMap gives a canonical tile order, including explicit absent-tile information.
            let bytes = bincode::serde::encode_to_vec(
                (neighbour, source.source.revision, &source.tiles),
                bincode::config::standard(),
            )?;
            hash.update(&bytes);
        }
        Ok(*hash.finalize().as_bytes())
    }
}

pub(crate) fn halo(cell: CellCoord) -> Result<Vec<(i32, i32, CellCoord)>, CompileError> {
    let mut cells = Vec::with_capacity(9);
    for dz in -1..=1 {
        for dx in -1..=1 {
            cells.push((
                dx,
                dz,
                CellCoord {
                    x: cell
                        .x
                        .checked_add(dx)
                        .ok_or(CompileError::CoordinateRange)?,
                    z: cell
                        .z
                        .checked_add(dz)
                        .ok_or(CompileError::CoordinateRange)?,
                },
            ));
        }
    }
    Ok(cells)
}
