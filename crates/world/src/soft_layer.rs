//! The soft layer (snow) over the firm ground, as terrain v2 draws it. A leaf node's heights are
//! the layer's top and its normals the top's, so every level, distance and pass draws the snow
//! as the ground; the leaf keeps the layer's depth for what reaches through it: feet, which press
//! it down, and actors, who stand on the firm ground. The depth comes from the control samples
//! (`terrain_control`): a soft surface's own depth where soft ground covers most of the ground,
//! thinning to nothing where firm ground takes over, then smoothed over about a metre, so banks
//! are rounded rather than stepping with the half-metre samples. Smoothing reaches into the
//! neighbouring cells. Each world sample is read from one tile only and every sum is taken in one
//! order, so neighbours compute their shared border exactly alike.
use crate::{CONTROL_INTERVALS, CellCoord, TERRAIN_HEIGHT_STEP, unpack_control};

/// Where soft ground shares the ground with firm ground, its layer thins from the upper share to
/// nothing at the lower: banks as wide as the masks blend, before smoothing.
pub const SOFT_EDGE: [f32; 2] = [0.35, 0.65];

/// The smoothing: a Gaussian of this deviation in control samples (0.75 m with 32 m cells), cut
/// off where it has fallen to half a percent.
const SMOOTHING: f32 = 1.5;

const SMOOTHING_RADIUS: i32 = 5;

/// What the soft layer needs of a world: each texture layer's soft depth in metres
/// (`TerrainSurface::soft_depth`) and its cells' size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoftGround {
    pub layer_depths: Vec<f32>,
    pub cell_size: f32,
}

/// The soft layer over a leaf's samples, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct SoftLayer {
    /// How deep it lies over the firm ground, in terrain height steps ([`TERRAIN_HEIGHT_STEP`]),
    /// so its top stays on the heights' grid.
    pub depths: Vec<u16>,
    /// Its slope along x and z.
    pub slopes: Vec<[f32; 2]>,
}

/// A cell's control tile and its neighbours', rows by z then x, the cell's own in the middle;
/// `None` where no cell is.
pub type ControlNeighbourhood<'a> = [[Option<&'a [u32]>; 3]; 3];

/// The control tiles around `cell`, by cell.
pub fn control_neighbourhood<'a>(
    cell: CellCoord,
    tile: impl Fn(CellCoord) -> Option<&'a [u32]>,
) -> ControlNeighbourhood<'a> {
    std::array::from_fn(|z| {
        std::array::from_fn(|x| {
            tile(CellCoord {
                x: cell.x + x as i32 - 1,
                z: cell.z + z as i32 - 1,
            })
        })
    })
}

/// A cell and its neighbours: the cells whose control tiles its soft layer reads, and whose soft
/// layers read its tile.
pub fn soft_layer_neighbours(cell: CellCoord) -> impl Iterator<Item = CellCoord> {
    (-1..=1).flat_map(move |z| {
        (-1..=1).map(move |x| CellCoord {
            x: cell.x + x,
            z: cell.z + z,
        })
    })
}

impl SoftGround {
    pub fn has_soft_ground(&self) -> bool {
        self.layer_depths.iter().any(|&d| d > 0.0)
    }

    /// The soft layer over a leaf of `resolution`² samples from the control tiles around it;
    /// `None` where it lies nowhere over the leaf.
    pub fn layer(
        &self,
        tiles: &ControlNeighbourhood,
        resolution: u16,
    ) -> Result<Option<SoftLayer>, String> {
        if !self.has_soft_ground() || tiles[1][1].is_none() {
            return Ok(None);
        }

        let intervals = usize::from(resolution).saturating_sub(1);

        if intervals == 0 || !CONTROL_INTERVALS.is_multiple_of(intervals) {
            return Err(format!(
                "a soft layer needs leaves of at most {CONTROL_INTERVALS} intervals that divide \
                 the control tiles', not {intervals}"
            ));
        }

        let Some(smoothed) = self.smoothed(tiles) else {
            return Ok(None);
        };
        let step = CONTROL_INTERVALS / intervals;
        let spacing = self.cell_size / CONTROL_INTERVALS as f32;
        let mut depths = Vec::with_capacity((intervals + 1) * (intervals + 1));
        let mut slopes = Vec::with_capacity(depths.capacity());

        for j in 0..=intervals {
            for i in 0..=intervals {
                let (u, v) = ((i * step) as i32, (j * step) as i32);
                let steps = (smoothed.at(u, v) / TERRAIN_HEIGHT_STEP).round();
                let depth = steps.clamp(0.0, f32::from(u16::MAX)) as u16;
                depths.push(depth);

                // No layer, no tilt: a neighbour with no layer at all leaves its normals as they
                // are, and the slope of a layer too thin to keep would part their shared border.
                slopes.push(if depth == 0 {
                    [0.0; 2]
                } else {
                    [
                        (smoothed.at(u + 1, v) - smoothed.at(u - 1, v)) / (2.0 * spacing),
                        (smoothed.at(u, v + 1) - smoothed.at(u, v - 1)) / (2.0 * spacing),
                    ]
                });
            }
        }

        Ok(depths
            .iter()
            .any(|&d| d > 0)
            .then_some(SoftLayer { depths, slopes }))
    }

    /// The smoothed depth over the centre tile's samples and one beyond each edge (for the
    /// slope there), or `None` where no soft ground is within reach.
    fn smoothed(&self, tiles: &ControlNeighbourhood) -> Option<Smoothed> {
        let n = CONTROL_INTERVALS as i32;
        let reach = SMOOTHING_RADIUS + 1;
        let side = (n + 2 * reach + 1) as usize;
        let inner = (n + 3) as usize;
        let weights: Vec<f32> = (-SMOOTHING_RADIUS..=SMOOTHING_RADIUS)
            .map(|k| (-((k * k) as f32) / (2.0 * SMOOTHING * SMOOTHING)).exp())
            .collect();

        // Each sample's own depth over the smoothing's reach, and whether a cell holds it: the
        // smoothing weighs only samples that exist, so the world's edge does not thin the layer.
        let mut depth = vec![0.0_f32; side * side];
        let mut held = vec![0.0_f32; side * side];
        let mut any = false;

        for v in 0..side {
            for u in 0..side {
                let sample = control_sample(tiles, u as i32 - reach, v as i32 - reach);

                if let Some(packed) = sample {
                    let d = self.sample_depth(packed);
                    depth[v * side + u] = d;
                    held[v * side + u] = 1.0;
                    any |= d > 0.0;
                }
            }
        }

        if !any {
            return None;
        }

        // Along x for every row, then along z: a Gaussian is separable, and so are its sums of
        // depth and of weight.
        let mut rows = vec![[0.0_f32; 2]; side * inner];

        for v in 0..side {
            for i in 0..inner {
                let mut sum = [0.0_f32; 2];

                for (k, w) in weights.iter().enumerate() {
                    let at = v * side + i + k;
                    sum[0] += w * depth[at];
                    sum[1] += w * held[at];
                }

                rows[v * inner + i] = sum;
            }
        }

        let mut values = vec![0.0_f32; inner * inner];

        for j in 0..inner {
            for i in 0..inner {
                let mut sum = [0.0_f32; 2];

                for (k, w) in weights.iter().enumerate() {
                    let row = rows[(j + k) * inner + i];
                    sum[0] += w * row[0];
                    sum[1] += w * row[1];
                }

                values[j * inner + i] = if sum[1] > 0.0 { sum[0] / sum[1] } else { 0.0 };
            }
        }

        Some(Smoothed { values, inner })
    }

    /// A control sample's soft layer: soft ground's own depth by share where it covers most of
    /// the ground, thinning to its edge.
    fn sample_depth(&self, packed: u32) -> f32 {
        let (mut depth, mut soft) = (0.0, 0.0);

        for (layer, share) in unpack_control(packed) {
            let d = self
                .layer_depths
                .get(usize::from(layer))
                .copied()
                .unwrap_or(0.0);

            if d > 0.0 {
                depth += share * d;
                soft += share;
            }
        }

        if soft <= 0.0 {
            return 0.0;
        }

        let t = ((soft - SOFT_EDGE[0]) / (SOFT_EDGE[1] - SOFT_EDGE[0])).clamp(0.0, 1.0);
        depth / soft * t * t * (3.0 - 2.0 * t)
    }
}

/// Smoothed depths of the centre tile's samples `-1..=CONTROL_INTERVALS + 1` a side.
struct Smoothed {
    values: Vec<f32>,
    inner: usize,
}

impl Smoothed {
    fn at(&self, u: i32, v: i32) -> f32 {
        self.values[(v + 1) as usize * self.inner + (u + 1) as usize]
    }
}

/// The control sample `u`, `v` samples from the centre tile's first: from the tile holding it as
/// its first row or column, or where no cell follows, the one before as its last.
fn control_sample(tiles: &ControlNeighbourhood, u: i32, v: i32) -> Option<u32> {
    let n = CONTROL_INTERVALS as i32;
    let axis = |c: i32| {
        let (tile, local) = (c.div_euclid(n), c.rem_euclid(n));
        let last = (local == 0).then_some((tile - 1, n));
        [Some((tile, local)), last]
    };

    for (tz, lz) in axis(v).into_iter().flatten() {
        for (tx, lx) in axis(u).into_iter().flatten() {
            let tile = usize::try_from(tz + 1)
                .ok()
                .zip(usize::try_from(tx + 1).ok())
                .and_then(|(z, x)| *tiles.get(z)?.get(x)?);

            if let Some(tile) = tile {
                return Some(tile[(lz * (n + 1) + lx) as usize]);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CONTROL_SAMPLES, pack_control};

    const SNOW: u16 = 3;
    const GRASS: u16 = 1;

    fn ground() -> SoftGround {
        let mut layer_depths = vec![0.0; 8];
        layer_depths[usize::from(SNOW)] = 0.4;
        SoftGround {
            layer_depths,
            cell_size: 32.0,
        }
    }

    /// A tile of cell `cell` whose snow covers world x below `edge` metres.
    fn tile(cell: CellCoord, edge: f32) -> Vec<u32> {
        let spacing = 32.0 / CONTROL_INTERVALS as f32;
        (0..CONTROL_SAMPLES * CONTROL_SAMPLES)
            .map(|i| {
                let x = (cell.x * 32) as f32 + (i % CONTROL_SAMPLES) as f32 * spacing;
                let snow = (0.5 - (x - edge) / 2.0).clamp(0.0, 1.0);
                pack_control([(SNOW, snow), (GRASS, 1.0 - snow), (GRASS, 0.0)])
            })
            .collect()
    }

    fn world(cells: &[CellCoord], edge: f32) -> std::collections::HashMap<CellCoord, Vec<u32>> {
        cells.iter().map(|&c| (c, tile(c, edge))).collect()
    }

    fn layer_of(
        tiles: &std::collections::HashMap<CellCoord, Vec<u32>>,
        cell: CellCoord,
    ) -> Option<SoftLayer> {
        let around = control_neighbourhood(cell, |c| tiles.get(&c).map(Vec::as_slice));
        ground().layer(&around, 33).unwrap()
    }

    fn cells() -> Vec<CellCoord> {
        (-1..=1)
            .flat_map(|z| (-1..=2).map(move |x| CellCoord { x, z }))
            .collect()
    }

    #[test]
    fn neighbours_agree_on_their_shared_border() {
        // A bank running across the border between cells 0 and 1.
        let tiles = world(&cells(), 32.0);
        let a = layer_of(&tiles, CellCoord { x: 0, z: 0 }).unwrap();
        let b = layer_of(&tiles, CellCoord { x: 1, z: 0 }).unwrap();

        for j in 0..33 {
            assert_eq!(a.depths[j * 33 + 32], b.depths[j * 33]);
            assert_eq!(
                a.slopes[j * 33 + 32].map(f32::to_bits),
                b.slopes[j * 33].map(f32::to_bits)
            );
        }

        assert!(a.depths[16 * 33 + 32] > 0, "the bank reaches the border");
    }

    #[test]
    fn banks_are_smooth_and_full_depth_away_from_them() {
        let tiles = world(&cells(), 16.0);
        let layer = layer_of(&tiles, CellCoord { x: 0, z: 0 }).unwrap();
        let row: Vec<f32> = (0..33)
            .map(|i| f32::from(layer.depths[16 * 33 + i]) * TERRAIN_HEIGHT_STEP)
            .collect();
        assert!((row[0] - 0.4).abs() < 0.002, "deep snow far from the bank");
        assert_eq!(row[32], 0.0, "none far past it");
        assert!(row.windows(2).all(|w| w[1] <= w[0]), "falling across it");

        // The masks give way over half a metre of share; smoothed, the fall spreads over metres.
        let steepest = row.windows(2).map(|w| w[0] - w[1]).fold(0.0, f32::max);
        assert!(steepest < 0.25, "{steepest}");

        let slope = layer.slopes[16 * 33 + 16][0];
        assert!(slope < -0.1, "the slope falls with the bank: {slope}");
    }

    #[test]
    fn a_leaf_without_a_layer_meets_one_whose_fringe_reaches_its_border_untilted() {
        // Snow ends a few metres short of the border between cells 0 and 1: cell 0's smoothing
        // reaches the border with depths too thin to keep, cell 1 has none.
        let tiles = world(&cells(), 29.0);
        let a = layer_of(&tiles, CellCoord { x: 0, z: 0 }).unwrap();
        assert!(layer_of(&tiles, CellCoord { x: 1, z: 0 }).is_none());

        for j in 0..33 {
            assert_eq!(a.depths[j * 33 + 32], 0);
            assert_eq!(a.slopes[j * 33 + 32], [0.0; 2]);
        }
    }

    #[test]
    fn firm_ground_and_the_world_edge_take_no_layer() {
        let tiles = world(&cells(), -100.0);
        assert!(layer_of(&tiles, CellCoord { x: 0, z: 0 }).is_none());

        // Snow everywhere, at the world's edge too.
        let tiles = world(&[CellCoord { x: 0, z: 0 }], 1000.0);
        let layer = layer_of(&tiles, CellCoord { x: 0, z: 0 }).unwrap();
        let full = (0.4 / TERRAIN_HEIGHT_STEP).round() as u16;
        assert!(layer.depths.iter().all(|&d| d.abs_diff(full) <= 1));
    }
}
