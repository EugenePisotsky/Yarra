//! Clipmap layout and CDLOD patch selection, free of GPU state.
//!
//! Every level of the clipmap is a `side`² texture window of one node level's samples (level L:
//! 2^L × the leaf spacing), addressed toroidally: world sample `s` lives at texel `s mod side`, so
//! moving the window rewrites only the node blocks that enter it. Patches are terrain nodes drawn
//! as one fixed grid of their own samples (CDLOD, Strugar 2009): a node is split while the camera
//! is within its children's range and all four children are resident, and each vertex morphs
//! towards the next coarser level over the outer part of its range, so neighbouring levels meet
//! without cracks.
use bevy::math::DVec3;
use std::collections::HashMap;
use world::TerrainNodeKey;

/// Shape of the clipmap: identical for height and normal levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipmapLayout {
    /// Texels per side of every level; a power of two and a multiple of `intervals`.
    pub side: u32,
    /// Levels from the leaves (0) up.
    pub levels: u8,
    /// Metres of a leaf node.
    pub cell_size: f64,
    /// Sample intervals per node side (node samples − 1).
    pub intervals: u32,
}

impl ClipmapLayout {
    pub fn new(side: u32, levels: u8, cell_size: f64, intervals: u32) -> Self {
        assert!(side.is_power_of_two() && intervals > 0 && side.is_multiple_of(intervals));
        assert!((1..=16).contains(&levels));
        Self {
            side,
            levels,
            cell_size,
            intervals,
        }
    }

    /// Metres between samples of `level`.
    pub fn spacing(&self, level: u8) -> f64 {
        self.cell_size * f64::from(1_u32 << level) / f64::from(self.intervals)
    }

    /// Metres of a node of `level`.
    pub fn node_metres(&self, level: u8) -> f64 {
        self.cell_size * f64::from(1_u32 << level)
    }

    /// Nodes per side of a level's window.
    pub fn nodes_per_side(&self) -> i32 {
        (self.side / self.intervals) as i32
    }

    /// Texel of world sample index `sample` along one axis.
    pub fn texel(&self, sample: i64) -> u32 {
        sample.rem_euclid(i64::from(self.side)) as u32
    }

    /// First texel (x, z) of a node's block in its level.
    pub fn block_origin(&self, key: TerrainNodeKey) -> [u32; 2] {
        let n = i64::from(self.intervals);
        [
            self.texel(i64::from(key.x) * n),
            self.texel(i64::from(key.z) * n),
        ]
    }
}

/// A level's window: `size` nodes per side from `min` (inclusive), in node coordinates. It
/// leaves one guard column and row of blocks free: the samples a window's last nodes share with
/// the nodes beyond them land there instead of on the window's first nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeWindow {
    pub level: u8,
    pub min: [i32; 2],
    pub size: i32,
}

impl NodeWindow {
    /// The node of `level` holding world `xz`.
    pub fn node_at(layout: &ClipmapLayout, level: u8, xz: [f64; 2]) -> [i32; 2] {
        let node = layout.node_metres(level);
        xz.map(|v| (v / node).floor() as i32)
    }

    /// The window of `level` around node `centre`.
    pub fn around(layout: &ClipmapLayout, level: u8, centre: [i32; 2]) -> Self {
        let half = (layout.nodes_per_side() - 1) / 2;
        Self {
            level,
            min: centre.map(|c| c - half),
            size: 2 * half + 1,
        }
    }

    /// The window of `level` centred on world `xz`.
    pub fn centred(layout: &ClipmapLayout, level: u8, xz: [f64; 2]) -> Self {
        Self::around(layout, level, Self::node_at(layout, level, xz))
    }

    pub fn centre(&self) -> [i32; 2] {
        self.min.map(|m| m + self.size / 2)
    }

    pub fn contains(&self, x: i32, z: i32) -> bool {
        x >= self.min[0]
            && z >= self.min[1]
            && x < self.min[0] + self.size
            && z < self.min[1] + self.size
    }

    /// Whether world `xz` lies at least `margin` nodes inside the window.
    pub fn holds(&self, layout: &ClipmapLayout, xz: [f64; 2], margin: i32) -> bool {
        let at = Self::node_at(layout, self.level, xz);
        (0..2).all(|i| at[i] >= self.min[i] + margin && at[i] < self.min[i] + self.size - margin)
    }

    pub fn nodes(&self) -> impl Iterator<Item = [i32; 2]> + '_ {
        (0..self.size * self.size)
            .map(move |i| [self.min[0] + i % self.size, self.min[1] + i / self.size])
    }
}

/// Distance ranges of the levels: level L draws out to `range(L)` from the camera, morphing
/// towards L + 1 over its last `MORPH_SHARE`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodRanges {
    /// Range of level 0, metres; every coarser level doubles it.
    pub leaf: f64,
}

pub const MORPH_SHARE: f64 = 0.3;

impl LodRanges {
    pub fn range(&self, level: u8) -> f64 {
        self.leaf * f64::from(1_u32 << level)
    }

    /// Where level `level` starts and ends morphing towards the next coarser level.
    pub fn morph(&self, level: u8) -> [f64; 2] {
        let end = self.range(level);
        [end * (1.0 - MORPH_SHARE), end]
    }
}

/// A node drawn as a patch, with its height bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    pub key: TerrainNodeKey,
    pub bounds: [f32; 2],
}

/// What selection needs of a node: its height bounds once it is resident in the clipmap.
pub trait NodeSource {
    fn resident(&self, key: TerrainNodeKey) -> Option<[f32; 2]>;
}

impl NodeSource for HashMap<TerrainNodeKey, [f32; 2]> {
    fn resident(&self, key: TerrainNodeKey) -> Option<[f32; 2]> {
        self.get(&key).copied()
    }
}

/// Distance from `eye` to a node's box.
pub fn distance(layout: &ClipmapLayout, key: TerrainNodeKey, bounds: [f32; 2], eye: DVec3) -> f64 {
    let size = layout.node_metres(key.level);
    let lo = DVec3::new(
        f64::from(key.x) * size,
        f64::from(bounds[0]),
        f64::from(key.z) * size,
    );
    let hi = DVec3::new(lo.x + size, f64::from(bounds[1]), lo.z + size);
    eye.distance(eye.clamp(lo, hi))
}

/// Patches covering `starts` (resident nodes whose union is the world): split a node while the
/// eye is within its children's range and every child is resident and inside its level's
/// clipmap window.
pub fn select(
    layout: &ClipmapLayout,
    ranges: &LodRanges,
    windows: &[NodeWindow],
    starts: &[TerrainNodeKey],
    nodes: &impl NodeSource,
    eye: DVec3,
) -> Vec<Patch> {
    let mut out = Vec::new();
    let mut stack: Vec<Patch> = starts
        .iter()
        .filter_map(|&key| {
            Some(Patch {
                key,
                bounds: nodes.resident(key)?,
            })
        })
        .collect();

    while let Some(patch) = stack.pop() {
        let key = patch.key;
        let split =
            key.level > 0 && distance(layout, key, patch.bounds, eye) < ranges.range(key.level - 1);
        let children = split
            .then(|| key.children().ok().flatten())
            .flatten()
            .and_then(|children| {
                let window = windows.iter().find(|w| w.level == key.level - 1)?;
                children
                    .iter()
                    .map(|&child| {
                        window
                            .contains(child.x, child.z)
                            .then(|| nodes.resident(child))
                            .flatten()
                            .map(|bounds| Patch { key: child, bounds })
                    })
                    .collect::<Option<Vec<_>>>()
            });

        match children {
            Some(children) => stack.extend(children),
            None => out.push(patch),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::WorldSpaceId;

    fn key(level: u8, x: i32, z: i32) -> TerrainNodeKey {
        TerrainNodeKey {
            space: WorldSpaceId(1),
            level,
            x,
            z,
        }
    }

    /// Every node of levels 0..=top over `cells`² leaves, flat at 0..10 m.
    fn world(top: u8, cells: i32) -> HashMap<TerrainNodeKey, [f32; 2]> {
        let mut nodes = HashMap::new();

        for level in 0..=top {
            let n = cells >> level;
            for z in 0..n {
                for x in 0..n {
                    nodes.insert(key(level, x, z), [0.0, 10.0]);
                }
            }
        }

        nodes
    }

    #[test]
    fn toroidal_texels_wrap_and_blocks_tile_the_window() {
        let layout = ClipmapLayout::new(1024, 6, 32.0, 32);
        assert_eq!(layout.texel(-1), 1023);
        assert_eq!(layout.texel(1024 + 5), 5);
        assert_eq!(layout.spacing(0), 1.0);
        assert_eq!(layout.spacing(3), 8.0);
        let window = NodeWindow::centred(&layout, 2, [1000.0, -1000.0]);
        let mut seen = std::collections::HashSet::new();

        for [x, z] in window.nodes() {
            assert!(
                seen.insert(layout.block_origin(key(2, x, z))),
                "two nodes share a block"
            );
        }

        assert_eq!(seen.len(), 31 * 31);

        // The shared last column and row land in the guard, not on the first nodes.
        let [x, z] = window.min.map(|m| m + window.size);

        for guard in [
            key(2, x, window.min[1]),
            key(2, window.min[0], z),
            key(2, x, z),
        ] {
            assert!(!seen.contains(&layout.block_origin(guard)));
        }

        assert_eq!(
            window.centre(),
            NodeWindow::node_at(&layout, 2, [1000.0, -1000.0])
        );
    }

    #[test]
    fn patches_cover_the_world_once_and_refine_towards_the_eye() {
        let layout = ClipmapLayout::new(1024, 6, 32.0, 32);
        let ranges = LodRanges { leaf: 192.0 };
        let nodes = world(5, 320);
        let eye = DVec3::new(5000.0, 50.0, 5000.0);
        let windows: Vec<_> = (0..6)
            .map(|l| NodeWindow::centred(&layout, l, [eye.x, eye.z]))
            .collect();
        let starts: Vec<_> = (0..10)
            .flat_map(|z| (0..10).map(move |x| key(5, x, z)))
            .collect();
        let patches = select(&layout, &ranges, &windows, &starts, &nodes, eye);

        // Leaf area adds up to the world's.
        let area: i64 = patches.iter().map(|p| 1_i64 << (2 * p.key.level)).sum();
        assert_eq!(area, 320 * 320);

        // Leaves near the eye, coarse far away, and a patch's level never jumps more than one
        // between a node and the ground the eye stands on.
        let under = patches
            .iter()
            .find(|p| {
                let s = layout.node_metres(p.key.level);
                let (x, z) = (f64::from(p.key.x) * s, f64::from(p.key.z) * s);
                (x..x + s).contains(&eye.x) && (z..z + s).contains(&eye.z)
            })
            .unwrap();
        assert_eq!(under.key.level, 0);
        assert!(patches.iter().any(|p| p.key.level == 5));

        for p in &patches {
            let d = distance(&layout, p.key, p.bounds, eye);
            // A patch is never drawn nearer than its parent's split range would allow.
            if p.key.level > 0 {
                assert!(d >= ranges.range(p.key.level - 1) - 1e-6, "{p:?} at {d}");
            }
        }

        assert!(patches.len() < 1500, "{}", patches.len());
    }

    #[test]
    fn missing_children_keep_the_parent_drawn() {
        let layout = ClipmapLayout::new(1024, 2, 32.0, 32);
        let ranges = LodRanges { leaf: 192.0 };
        let mut nodes = world(1, 4);
        nodes.remove(&key(0, 0, 0));
        let eye = DVec3::new(10.0, 5.0, 10.0);
        let windows: Vec<_> = (0..2)
            .map(|l| NodeWindow::centred(&layout, l, [eye.x, eye.z]))
            .collect();
        let starts: Vec<_> = (0..2)
            .flat_map(|z| (0..2).map(move |x| key(1, x, z)))
            .collect();
        let patches = select(&layout, &ranges, &windows, &starts, &nodes, eye);
        assert!(patches.iter().any(|p| p.key == key(1, 0, 0)));
        assert_eq!(patches.iter().filter(|p| p.key.level == 0).count(), 12);
    }
}
