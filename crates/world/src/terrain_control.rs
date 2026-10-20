//! Terrain control tiles (terrain v2): which of the world's ground surfaces cover each sample of a
//! hierarchy node, and how much. A tile holds [`CONTROL_SAMPLES`]² samples, 1/64 of the node
//! apart (the weight pages' 0.5 m at level 0 with 32 m cells), endpoint-inclusive like
//! heightfields: its last row and column are the next nodes' first. Each sample keeps the three
//! heaviest surfaces, by texture array layer, and their shares in 32 bits, so the GPU blends any
//! of the world's surfaces anywhere. Parents filter their children's surface shares, never their
//! layer indices.
use crate::{
    CellCoord, MAX_TERRAIN_SURFACES_PER_CELL, TerrainHeightfieldPage, TerrainNodeKey,
    TerrainWeightPage, bare_share,
};

/// Surfaces a sample keeps and the GPU blends at one point: the heaviest three.
pub const TERRAIN_BLEND_SURFACES: usize = 3;

/// Samples per tile side.
pub const CONTROL_SAMPLES: usize = 65;

/// Intervals per tile side.
pub const CONTROL_INTERVALS: usize = CONTROL_SAMPLES - 1;

/// Texture array layers a control sample can name.
pub const MAX_CONTROL_LAYERS: u16 = 32;

/// The coarsest level with control tiles: terrain v2's top clipmap level.
pub const CONTROL_TOP_LEVEL: u8 = 5;

/// A node's control samples, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainControlTile {
    pub key: TerrainNodeKey,
    pub samples: Vec<u32>,
}

/// Bits 0–4, 5–9 and 10–14 hold the layers, heaviest first; bits 16–23 and 24–31 the first two
/// shares of 255; the third takes the rest. Shares are normalised; a sample with fewer surfaces
/// repeats its first layer with no share.
pub fn pack_control(top: [(u16, f32); TERRAIN_BLEND_SURFACES]) -> u32 {
    let sum: f32 = top.iter().map(|t| t.1.max(0.0)).sum();
    let first = top[0].0.min(MAX_CONTROL_LAYERS - 1);

    if sum <= 0.0 {
        return u32::from(first) | u32::from(first) << 5 | u32::from(first) << 10 | 255 << 16;
    }

    let share = |i: usize| top[i].1.max(0.0) / sum;
    let a = (share(0) * 255.0).round().clamp(0.0, 255.0) as u32;
    let b = ((share(1) * 255.0).round() as u32).min(255 - a);
    let layer = |i: usize| {
        if top[i].1 > 0.0 {
            u32::from(top[i].0.min(MAX_CONTROL_LAYERS - 1))
        } else {
            u32::from(first)
        }
    };
    layer(0) | layer(1) << 5 | layer(2) << 10 | a << 16 | b << 24
}

/// The three (layer, share) pairs of a packed sample; shares sum to one.
pub fn unpack_control(sample: u32) -> [(u16, f32); TERRAIN_BLEND_SURFACES] {
    let layer = |i: u32| ((sample >> (5 * i)) & 31) as u16;
    let a = (sample >> 16) & 255;
    let b = (sample >> 24) & 255;
    let c = 255 - a - b.min(255 - a);
    [
        (layer(0), a as f32 / 255.0),
        (layer(1), b as f32 / 255.0),
        (layer(2), c as f32 / 255.0),
    ]
}

/// Surface shares gathered from several sources, by layer.
#[derive(Clone, Copy, Debug, Default)]
pub struct ControlMix {
    entries: [(u16, f32); 16],
    len: usize,
}

impl ControlMix {
    pub fn add(&mut self, layer: u16, weight: f32) {
        if weight <= 0.0 {
            return;
        }
        if let Some(entry) = self.entries[..self.len].iter_mut().find(|e| e.0 == layer) {
            entry.1 += weight;
        } else if self.len < self.entries.len() {
            self.entries[self.len] = (layer, weight);
            self.len += 1;
        } else if let Some(lightest) = self.entries.iter_mut().min_by(|a, b| a.1.total_cmp(&b.1))
            && lightest.1 < weight
        {
            *lightest = (layer, weight);
        }
    }

    /// Adds a packed sample's shares, scaled by `weight`.
    pub fn add_packed(&mut self, sample: u32, weight: f32) {
        for (layer, share) in unpack_control(sample) {
            self.add(layer, share * weight);
        }
    }

    /// The three heaviest; ties keep the lower layer, so the result does not depend on order.
    pub fn top(&self) -> [(u16, f32); TERRAIN_BLEND_SURFACES] {
        let mut best = [(0_u16, 0.0_f32); TERRAIN_BLEND_SURFACES];

        for &(layer, weight) in &self.entries[..self.len] {
            let heavier = |b: &(u16, f32)| weight > b.1 || (weight == b.1 && layer < b.0);
            if let Some(rank) = best.iter().position(heavier) {
                best.copy_within(rank..TERRAIN_BLEND_SURFACES - 1, rank + 1);
                best[rank] = (layer, weight);
            }
        }

        best
    }

    pub fn pack(&self) -> u32 {
        pack_control(self.top())
    }
}

/// A cell's samples from its page's ground weights, worn where its roads wear it as near
/// detail wears it; `slot_layers` gives each page surface's texture layer.
pub fn leaf_control(
    page: &TerrainHeightfieldPage,
    slot_layers: &[u16],
    cell: CellCoord,
    cell_size: f32,
) -> Result<Vec<u32>, String> {
    let count = page.surfaces.len();

    if count == 0 || count > MAX_TERRAIN_SURFACES_PER_CELL || slot_layers.len() != count {
        return Err(format!(
            "terrain pages have one to {MAX_TERRAIN_SURFACES_PER_CELL} surfaces with layers"
        ));
    }

    if count > 1 && page.weight_pages.len() != count.div_ceil(4) {
        return Err("terrain page weights do not match its surfaces".into());
    }

    let mut samples = Vec::with_capacity(CONTROL_SAMPLES * CONTROL_SAMPLES);
    let mut ground = vec![0.0_f64; count];

    for j in 0..CONTROL_SAMPLES {
        for i in 0..CONTROL_SAMPLES {
            let uv = [
                i as f64 / CONTROL_INTERVALS as f64,
                j as f64 / CONTROL_INTERVALS as f64,
            ];

            if count == 1 {
                ground[0] = 1.0;
            } else {
                page_weights(&page.weight_pages, uv, &mut ground);
                if let Some(wear) = &page.wear {
                    let world = [
                        (f64::from(cell.x) + uv[0]) * f64::from(cell_size),
                        (f64::from(cell.z) + uv[1]) * f64::from(cell_size),
                    ];
                    let w = wear.sample(uv).wear;
                    wear.lay(&mut ground, w, bare_share(w, world));
                }
            }

            let mut mix = ControlMix::default();

            for (&layer, &weight) in slot_layers.iter().zip(&ground) {
                mix.add(layer, weight as f32);
            }

            samples.push(mix.pack());
        }
    }

    Ok(samples)
}

/// Bilinear ground weights of the page's surfaces at `uv`, four per RGBA page, normalised.
fn page_weights(pages: &[TerrainWeightPage], uv: [f64; 2], result: &mut [f64]) {
    result.fill(0.0);
    let n = pages[0].resolution as usize;
    let p = uv.map(|x| x * (n - 1) as f64);
    let a = p.map(|x| (x.floor() as usize).min(n - 1));
    let f = [p[0] - a[0] as f64, p[1] - a[1] as f64];

    for y in 0..2 {
        for x in 0..2 {
            let w =
                (if x == 0 { 1. - f[0] } else { f[0] }) * (if y == 0 { 1. - f[1] } else { f[1] });

            if w <= 0.0 {
                continue;
            }

            let i = ((a[1] + y).min(n - 1) * n + (a[0] + x).min(n - 1)) * 4;

            for (surface, value) in result.iter_mut().enumerate() {
                *value += f64::from(pages[surface / 4].rgba[i + surface % 4]) / 255. * w;
            }
        }
    }

    let sum = result.iter().sum::<f64>().max(1e-9);

    for value in result {
        *value /= sum;
    }
}

/// A parent's samples from its four children (in [`TerrainNodeKey::children`] order; a missing
/// child adds nothing). The children's samples form a 129² grid sharing their inner edges; each
/// parent sample filters the three around its own along each axis by [1, 2, 1], except on the
/// tile's edges, which filter only along the edge: neighbouring parents then compute their shared
/// edge from the same samples and agree.
pub fn filter_control_parent(children: [Option<&[u32]>; 4]) -> Vec<u32> {
    const N: usize = CONTROL_INTERVALS;
    let at = |x: usize, z: usize| -> Option<u32> {
        // The child holding a combined sample; shared edges belong to either.
        let (cx, cz) = ((x / N).min(1), (z / N).min(1));
        let child = children[cx + 2 * cz]?;
        Some(child[(z - cz * N) * CONTROL_SAMPLES + (x - cx * N)])
    };
    let taps = |i: usize| -> Vec<(usize, f32)> {
        if i == 0 || i == N {
            vec![(2 * i, 1.0)]
        } else {
            vec![(2 * i - 1, 0.25), (2 * i, 0.5), (2 * i + 1, 0.25)]
        }
    };
    let mut out = Vec::with_capacity(CONTROL_SAMPLES * CONTROL_SAMPLES);

    for j in 0..CONTROL_SAMPLES {
        for i in 0..CONTROL_SAMPLES {
            let mut mix = ControlMix::default();

            for (z, wz) in taps(j) {
                for &(x, wx) in &taps(i) {
                    if let Some(sample) = at(x, z) {
                        mix.add_packed(sample, wx * wz);
                    }
                }
            }

            out.push(mix.pack());
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_samples_keep_layers_heaviest_first_and_shares_summing_to_one() {
        let packed = pack_control([(7, 0.5), (3, 0.3), (24, 0.2)]);
        let [a, b, c] = unpack_control(packed);
        assert_eq!((a.0, b.0, c.0), (7, 3, 24));
        assert!((a.1 - 0.5).abs() < 0.003 && (b.1 - 0.3).abs() < 0.003);
        assert!((a.1 + b.1 + c.1 - 1.0).abs() < 1e-6);

        // One surface: the others repeat it with no share.
        let [a, b, c] = unpack_control(pack_control([(5, 2.0), (0, 0.0), (0, 0.0)]));
        assert_eq!((a, b.0, b.1, c.0, c.1), ((5, 1.0), 5, 0.0, 5, 0.0));
    }

    #[test]
    fn mixes_merge_layers_and_rank_ties_by_layer() {
        let mut mix = ControlMix::default();

        for (layer, w) in [(9, 0.2), (4, 0.2), (9, 0.3), (1, 0.1), (2, 0.05)] {
            mix.add(layer, w);
        }

        let top = mix.top();
        assert_eq!(top.map(|t| t.0), [9, 4, 1]);
        assert!((top[0].1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn parents_filter_shares_and_agree_on_shared_edges() {
        let uniform = |layer| vec![pack_control([(layer, 1.0), (0, 0.0), (0, 0.0)]); 65 * 65];
        let (grass, rock) = (uniform(2), uniform(11));

        // Left children grass, right children rock.
        let parent = filter_control_parent([Some(&grass), Some(&rock), Some(&grass), Some(&rock)]);
        let sample = |x: usize, z: usize| unpack_control(parent[z * 65 + x]);
        assert_eq!(sample(10, 30)[0], (2, 1.0));
        assert_eq!(sample(50, 30)[0], (11, 1.0));

        // On the seam between the children the right child holds the shared column.
        let seam = sample(32, 30);
        assert_eq!((seam[0].0, seam[1].0), (11, 2));
        assert!((seam[0].1 - 0.75).abs() < 0.01);

        // The right edge filters only along itself: the parent to the right, made of rock
        // children, has the same samples on its left edge.
        let next = filter_control_parent([Some(&rock), Some(&rock), Some(&rock), Some(&rock)]);

        for z in 0..65 {
            assert_eq!(parent[z * 65 + 64], next[z * 65]);
        }

        // Missing children add nothing.
        let half = filter_control_parent([Some(&grass), None, None, None]);
        assert_eq!(unpack_control(half[10 * 65 + 10])[0], (2, 1.0));
    }
}
