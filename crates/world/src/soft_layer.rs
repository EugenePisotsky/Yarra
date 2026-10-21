//! The soft layer (snow) over the firm ground, as terrain v2 draws it. A leaf node's heights are
//! the layer's top and its normals the top's, so every level, distance and pass draws the snow
//! as the ground; the leaf keeps the layer's depth for what reaches through it: feet, which press
//! it down, and actors, who stand on the firm ground. The depth comes from the control samples
//! (`terrain_control`), smoothed so banks are rounded rather than stepping with the half-metre
//! samples, once the soft ground is drawn back from its edge and the deeper from the shallower:
//! how much of the ground around is soft, and how deep (snow's depth, or packed snow's on a road
//! through it). The layer so starts where the ground's surfaces show its edge and thickens into
//! the soft ground over a few metres; firm ground among snow, a patch of grass, holds none, and a
//! path of packed snow keeps its own depth with the snow's banks rising from its sides. Smoothing
//! reaches into the neighbouring cells. Each world sample is read from one tile only and every
//! sum is taken in one order, so neighbours compute their shared border exactly alike.
use crate::{CONTROL_INTERVALS, CellCoord, TERRAIN_HEIGHT_STEP, unpack_control};

/// How far, in control samples, deep soft ground is drawn back from shallower (1 m), and the
/// smoothing of its depth after: a Gaussian of this deviation in samples (0.75 m with 32 m cells),
/// cut off where it has fallen to half a percent. A path of packed snow so keeps its depth to its
/// sides, where the snow's banks rise over a couple of metres.
const DEPTH_RETREAT: i32 = 2;

const DEPTH_SMOOTHING: (f32, i32) = (1.5, 5);

/// How far soft ground is drawn back from its edge (1.5 m), the smoothing of how much ground
/// around is soft after (1 m), and the shares of it over which the layer thickens from nothing to
/// its full depth.
const EDGE_RETREAT: i32 = 3;

const EDGE_SMOOTHING: (f32, i32) = (2.0, 7);

const EDGE: [f32; 2] = [0.05, 0.95];

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
        let reach = (DEPTH_RETREAT + DEPTH_SMOOTHING.1).max(EDGE_RETREAT + EDGE_SMOOTHING.1) + 1;
        let side = (n + 2 * reach + 1) as usize;

        // Each sample's soft share, the soft ground's depth by share, and whether a cell holds
        // it: smoothing weighs only samples that exist, so the world's edge does not thin it.
        let mut samples = vec![[0.0_f32; 3]; side * side];
        let mut any = false;

        for v in 0..side {
            for u in 0..side {
                let sample = control_sample(tiles, u as i32 - reach, v as i32 - reach);

                if let Some(packed) = sample {
                    let [soft, depth] = self.softness(packed);
                    samples[v * side + u] = [soft, depth, 1.0];
                    any |= depth > 0.0;
                }
            }
        }

        if !any {
            return None;
        }

        let drawn_back = |by: i32| retreat(&samples, side, by as usize);
        let depth = smooth(
            &drawn_back(DEPTH_RETREAT),
            side,
            reach,
            DEPTH_SMOOTHING,
            [1, 0],
        );
        let edge = smooth(
            &drawn_back(EDGE_RETREAT),
            side,
            reach,
            EDGE_SMOOTHING,
            [0, 2],
        );
        let values = depth
            .iter()
            .zip(&edge)
            .map(|(d, e)| {
                let depth = if d[1] > 0.0 { d[0] / d[1] } else { 0.0 };
                let share = if e[1] > 0.0 { e[0] / e[1] } else { 0.0 };
                let t = ((share - EDGE[0]) / (EDGE[1] - EDGE[0])).clamp(0.0, 1.0);
                depth * t * t * (3.0 - 2.0 * t)
            })
            .collect();
        Some(Smoothed {
            values,
            inner: (n + 3) as usize,
        })
    }

    /// A control sample's share of soft ground and its depth by share.
    fn softness(&self, packed: u32) -> [f32; 2] {
        let (mut soft, mut depth) = (0.0, 0.0);

        for (layer, share) in unpack_control(packed) {
            let d = self
                .layer_depths
                .get(usize::from(layer))
                .copied()
                .unwrap_or(0.0);

            if d > 0.0 {
                soft += share;
                depth += share * d;
            }
        }

        [soft, depth]
    }
}

/// `samples` with each soft share and depth by share the least within `retreat` samples along x
/// and z among those a cell holds: the soft ground drawn back from its edge, and deep soft ground
/// from shallower. Samples within `retreat` of the grid's edge keep theirs; smoothing never reads
/// them.
fn retreat(samples: &[[f32; 3]], side: usize, retreat: usize) -> Vec<[f32; 3]> {
    let least = |values: &mut dyn Iterator<Item = [f32; 3]>| {
        values
            .filter(|s| s[2] > 0.0)
            .fold([f32::INFINITY; 2], |l, s| [l[0].min(s[0]), l[1].min(s[1])])
    };
    let mut rows = samples.to_vec();

    for v in 0..side {
        for u in retreat..side - retreat {
            if samples[v * side + u][2] > 0.0 {
                let [soft, depth] =
                    least(&mut (u - retreat..=u + retreat).map(|i| samples[v * side + i]));
                rows[v * side + u][..2].copy_from_slice(&[soft, depth]);
            }
        }
    }

    let mut out = rows.clone();

    for v in retreat..side - retreat {
        for u in 0..side {
            if rows[v * side + u][2] > 0.0 {
                let [soft, depth] =
                    least(&mut (v - retreat..=v + retreat).map(|j| rows[j * side + u]));
                out[v * side + u][..2].copy_from_slice(&[soft, depth]);
            }
        }
    }

    out
}

/// Two channels of `samples` (`side`² around the centre tile, `reach` beyond it), each smoothed
/// by a Gaussian of `(deviation, radius)` in samples over the centre tile's samples and one
/// beyond each edge. A Gaussian is separable: along x for every row, then along z.
fn smooth(
    samples: &[[f32; 3]],
    side: usize,
    reach: i32,
    (deviation, radius): (f32, i32),
    [a, b]: [usize; 2],
) -> Vec<[f32; 2]> {
    let inner = CONTROL_INTERVALS + 3;
    let weights: Vec<f32> = (-radius..=radius)
        .map(|k| (-((k * k) as f32) / (2.0 * deviation * deviation)).exp())
        .collect();

    // The first sample each output's taps read: one before the centre tile, less the radius.
    let first = (reach - 1 - radius) as usize;
    let mut rows = vec![[0.0_f32; 2]; side * inner];

    for v in 0..side {
        for i in 0..inner {
            let mut sum = [0.0_f32; 2];

            for (k, w) in weights.iter().enumerate() {
                let sample = samples[v * side + first + i + k];
                sum[0] += w * sample[a];
                sum[1] += w * sample[b];
            }

            rows[v * inner + i] = sum;
        }
    }

    let mut out = vec![[0.0_f32; 2]; inner * inner];

    for j in 0..inner {
        for i in 0..inner {
            let mut sum = [0.0_f32; 2];

            for (k, w) in weights.iter().enumerate() {
                let row = rows[(first + j + k) * inner + i];
                sum[0] += w * row[0];
                sum[1] += w * row[1];
            }

            out[j * inner + i] = sum;
        }
    }

    out
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
    const PACKED: u16 = 4;
    const GRASS: u16 = 1;

    fn ground() -> SoftGround {
        let mut layer_depths = vec![0.0; 8];
        layer_depths[usize::from(SNOW)] = 0.4;
        layer_depths[usize::from(PACKED)] = 0.06;
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
        let tiles = world(&cells(), 33.0);
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

        // The masks give way over a couple of metres; the layer thickens over as many.
        let steepest = row.windows(2).map(|w| w[0] - w[1]).fold(0.0, f32::max);
        assert!(steepest < 0.3, "{steepest}");
        assert!(
            row[17] < 0.002,
            "none where the mask is more firm than soft"
        );

        let slope = layer.slopes[16 * 33 + 15][0];
        assert!(slope < -0.1, "the slope falls with the bank: {slope}");
    }

    #[test]
    fn a_patch_of_firm_ground_among_snow_holds_none() {
        // Grass a metre and a half across around x 15.75, its masks giving way over a metre.
        let tile = |cell: CellCoord| -> Vec<u32> {
            let spacing = 32.0 / CONTROL_INTERVALS as f32;
            (0..CONTROL_SAMPLES * CONTROL_SAMPLES)
                .map(|i| {
                    let x = (cell.x * 32) as f32 + (i % CONTROL_SAMPLES) as f32 * spacing;
                    let grass = (1.25 - (x - 15.75).abs()).clamp(0.0, 1.0);
                    pack_control([(SNOW, 1.0 - grass), (GRASS, grass), (GRASS, 0.0)])
                })
                .collect()
        };
        let tiles = cells().into_iter().map(|c| (c, tile(c))).collect();
        let layer = layer_of(&tiles, CellCoord { x: 0, z: 0 }).unwrap();
        let depth = |x: usize| f32::from(layer.depths[16 * 33 + x]) * TERRAIN_HEIGHT_STEP;
        assert!(
            depth(16) < 0.03,
            "the patch holds next to none: {}",
            depth(16)
        );
        assert!(depth(12) > 0.3 && depth(20) > 0.3, "deep snow around it");
    }

    #[test]
    fn a_path_of_packed_snow_keeps_its_depth_with_banks_rising_from_its_sides() {
        // Packed snow two metres across around x 16, its masks giving way over a metre.
        let tile = |cell: CellCoord| -> Vec<u32> {
            let spacing = 32.0 / CONTROL_INTERVALS as f32;
            (0..CONTROL_SAMPLES * CONTROL_SAMPLES)
                .map(|i| {
                    let x = (cell.x * 32) as f32 + (i % CONTROL_SAMPLES) as f32 * spacing;
                    let path = (1.5 - (x - 16.0).abs()).clamp(0.0, 1.0);
                    pack_control([(SNOW, 1.0 - path), (PACKED, path), (GRASS, 0.0)])
                })
                .collect()
        };
        let tiles = cells().into_iter().map(|c| (c, tile(c))).collect();
        let layer = layer_of(&tiles, CellCoord { x: 0, z: 0 }).unwrap();
        let row: Vec<f32> = (0..33)
            .map(|i| f32::from(layer.depths[16 * 33 + i]) * TERRAIN_HEIGHT_STEP)
            .collect();
        assert!(
            (row[16] - 0.06).abs() < 0.01,
            "the path's own depth: {}",
            row[16]
        );
        assert!(
            row[15] < 0.1 && row[17] < 0.1,
            "its edges: {} {}",
            row[15],
            row[17]
        );
        assert!(row[11] > 0.35 && row[21] > 0.35, "deep snow beside it");
        let steepest = row
            .windows(2)
            .map(|w| (w[0] - w[1]).abs())
            .fold(0.0, f32::max);
        assert!(steepest < 0.3, "{steepest}");
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
