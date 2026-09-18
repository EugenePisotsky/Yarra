//! Frame-rate-independent, world-space coverage strokes. A segment paints a capsule; samples
//! shared by neighboring cells are evaluated at exactly the same logical coordinate.
use crate::{CoverageCell, CoverageTile, EnvironmentDefinition, LayerId};
use world::CellCoord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushOperation {
    Paint,
    Erase,
}

#[derive(Debug, Clone, Copy)]
pub struct CoverageBrush {
    pub radius: f64,
    /// Fraction of the radius occupied by the soft edge (0 = hard, 1 = fully soft).
    pub falloff: f64,
    /// Maximum change per stroke, independent of pointer-event frequency or dwell time.
    pub strength: f64,
    pub operation: BrushOperation,
}

impl CoverageBrush {
    pub fn valid(self) -> bool {
        self.radius.is_finite()
            && self.radius > 0.0
            && self.radius <= 64.0
            && self.falloff.is_finite()
            && (0.0..=1.0).contains(&self.falloff)
            && self.strength.is_finite()
            && (0.0..=1.0).contains(&self.strength)
    }

    /// Conservatively includes both owners of shared endpoints. Never returns a partial region.
    pub fn cells(
        self,
        cell_size: f32,
        from: [f64; 2],
        to: [f64; 2],
    ) -> Result<Vec<CellCoord>, &'static str> {
        if !self.valid()
            || !cell_size.is_finite()
            || cell_size <= 0.0
            || !from.into_iter().chain(to).all(f64::is_finite)
        {
            return Err("Invalid brush or world position");
        }
        let size = f64::from(cell_size);
        let minimum = [
            (from[0].min(to[0]) - self.radius) / size,
            (from[1].min(to[1]) - self.radius) / size,
        ]
        .map(f64::floor);
        let maximum = [
            (from[0].max(to[0]) + self.radius) / size,
            (from[1].max(to[1]) + self.radius) / size,
        ]
        .map(f64::floor);
        if minimum
            .into_iter()
            .chain(maximum)
            .any(|v| v <= i32::MIN as f64 || v >= i32::MAX as f64)
        {
            return Err("Brush is outside the supported world coordinates");
        }
        if (maximum[0] - minimum[0] + 1.0) * (maximum[1] - minimum[1] + 1.0) > 64.0 {
            return Err("Move the brush a shorter distance");
        }
        let mut cells = Vec::new();
        for x in minimum[0] as i32..=maximum[0] as i32 {
            for z in minimum[1] as i32..=maximum[1] as i32 {
                cells.push(CellCoord { x, z });
            }
        }
        Ok(cells)
    }

    /// `before` is the original cell at mouse-down (or on first entering this cell). The current
    /// cell may contain earlier segments of this same stroke. Their influence combines by max,
    /// so subdividing a segment or receiving duplicate input cannot strengthen a stroke.
    pub fn apply_segment(
        self,
        definition: &EnvironmentDefinition,
        layer: LayerId,
        before: &CoverageCell,
        current: &CoverageCell,
        from: [f64; 2],
        to: [f64; 2],
    ) -> Result<CoverageCell, &'static str> {
        if !self.valid()
            || !definition.cell_size.is_finite()
            || definition.cell_size <= 0.0
            || before.cell != current.cell
            || !definition.layers.iter().any(|l| l.id == layer)
            || !(2..=257).contains(&definition.mask_resolution)
            || !from.into_iter().chain(to).all(f64::is_finite)
        {
            return Err("Invalid coverage stroke");
        }
        let n = usize::from(definition.mask_resolution);
        let original = before
            .tiles
            .iter()
            .find(|t| t.layer == layer)
            .map(|t| t.samples.as_slice());
        let mut result = current.clone();
        if !result.tiles.iter().any(|t| t.layer == layer) {
            result.tiles.push(CoverageTile {
                layer,
                samples: vec![0; n * n],
            });
        }
        let tile = result.tiles.iter_mut().find(|t| t.layer == layer).unwrap();
        if tile.samples.len() != n * n || original.is_some_and(|s| s.len() != n * n) {
            return Err("Coverage tile dimensions differ from the layer grid");
        }
        let size = f64::from(definition.cell_size);
        let delta = [to[0] - from[0], to[1] - from[1]];
        let length_squared = delta[0] * delta[0] + delta[1] * delta[1];
        for (i, value) in tile.samples.iter_mut().enumerate() {
            // Form a global integer grid index first: both owners of a shared sample use
            // identical arithmetic, including at distant cells and non-integer cell sizes.
            let point = [
                (i64::from(current.cell.x) * (n - 1) as i64 + (i % n) as i64) as f64 * size
                    / (n - 1) as f64,
                (i64::from(current.cell.z) * (n - 1) as i64 + (i / n) as i64) as f64 * size
                    / (n - 1) as f64,
            ];
            let t = if length_squared > 0.0 {
                (((point[0] - from[0]) * delta[0] + (point[1] - from[1]) * delta[1])
                    / length_squared)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let distance =
                (point[0] - from[0] - t * delta[0]).hypot(point[1] - from[1] - t * delta[1]);
            if distance >= self.radius {
                continue;
            }
            let edge = if self.falloff == 0.0 {
                1.0
            } else {
                ((self.radius - distance) / (self.radius * self.falloff)).clamp(0.0, 1.0)
            };
            let amount = edge * edge * (3.0 - 2.0 * edge) * self.strength;
            let base = f64::from(original.map_or(0, |s| s[i]));
            *value = match self.operation {
                BrushOperation::Paint => {
                    (*value).max((base + (255.0 - base) * amount).round() as u8)
                }
                BrushOperation::Erase => (*value).min((base * (1.0 - amount)).round() as u8),
            };
        }
        result.tiles.retain(|t| t.samples.iter().any(|&v| v != 0));
        result.tiles.sort_by_key(|t| t.layer);
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
