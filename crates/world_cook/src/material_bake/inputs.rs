use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};
use world::TerrainTextureSet;

const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;
/// Texture array layers one input may describe.
const MAX_LAYERS: usize = 32;
const MAX_LIBRARY_BYTES: usize = 16 * 1024 * 1024;
const SIZE: usize = 128;
const CHAIN_BYTES: usize = 21845 * 4;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    entries: Vec<Entry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    base_color: String,
    normal_material: String,
    macro_variation: String,
    input: String,
}

/// Immutable, bounded preprocessed texture inputs. Loading never requires a GPU.
pub struct TerrainBakeLibrary {
    entries: BTreeMap<String, (String, String, Inputs)>,
    fingerprint: [u8; 32],
}
impl TerrainBakeLibrary {
    pub fn load(asset_root: &Path) -> Result<Self> {
        let manifest = read_limited(&asset_root.join("packs/terrain/bake.ron"), 64 * 1024)?;
        let manifest: Manifest = ron::de::from_bytes(&manifest)?;
        if manifest.version != 1 || manifest.entries.is_empty() || manifest.entries.len() > 8 {
            bail!("terrain bake manifest version/count");
        }
        let mut entries = BTreeMap::new();
        let mut total = 0;
        for e in manifest.entries {
            let relative = Path::new(&e.input);
            if relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            {
                bail!("terrain bake input must be asset-relative");
            }
            let data = read_limited(
                &asset_root.join(relative),
                MAX_INPUT_BYTES.min((MAX_LIBRARY_BYTES - total) as u64),
            )
            .with_context(|| "prepare terrain bake inputs with tools/prepare_terrain_bake.py")?;
            total += data.len();
            if total > MAX_LIBRARY_BYTES {
                bail!("terrain bake library byte limit");
            }
            let input = Inputs::decode(data)?;
            if entries
                .insert(e.base_color, (e.normal_material, e.macro_variation, input))
                .is_some()
            {
                bail!("duplicate terrain bake texture set");
            }
        }
        Ok(Self::new(entries))
    }
    fn new(entries: BTreeMap<String, (String, String, Inputs)>) -> Self {
        let mut hash = blake3::Hasher::new();
        hash.update(b"terrain-bake-library-v1");
        for (base_color, (normal, macro_uri, inputs)) in &entries {
            for uri in [base_color, normal, macro_uri] {
                hash.update(&(uri.len() as u64).to_le_bytes());
                hash.update(uri.as_bytes());
            }
            hash.update(&inputs.hash);
        }
        Self {
            entries,
            fingerprint: *hash.finalize().as_bytes(),
        }
    }
    /// Identifies every input a bake may read; published composites record it.
    pub(super) fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
    pub(super) fn get(&self, set: &TerrainTextureSet) -> Result<&Inputs> {
        let (normal, macro_uri, inputs) = self
            .entries
            .get(&set.base_color_universal_uri)
            .context("no CPU bake input for terrain texture set")?;
        if normal != &set.normal_material_universal_uri
            || macro_uri != &set.macro_variation_universal_uri
        {
            bail!("terrain bake texture set does not match packed inputs");
        }
        Ok(inputs)
    }
    /// Sixteen layers alternating two contrasting surfaces: even layers green, odd brown, at
    /// equal blend heights.
    #[cfg(test)]
    pub(super) fn fixture(set: &TerrainTextureSet) -> Self {
        const LAYERS: usize = 16;
        let mut entries = BTreeMap::new();
        let mut bytes = b"YTRBAKE\0".to_vec();
        for n in [2_u32, 128, LAYERS as u32] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend(0_f32.to_le_bytes());
        bytes.extend([0; 32]);
        // Known linear-light means; BA supplies AO/roughness data; the macro field is neutral.
        let base = [[40, 130, 25, 255], [180, 120, 70, 255]];
        let material = [[128, 128, 255, 100], [128, 128, 200, 200]];
        let chains = (0..LAYERS)
            .map(|l| base[l % 2])
            .chain((0..LAYERS).map(|l| material[l % 2]))
            .chain([[128, 128, 128, 255]]);
        for rgba in chains {
            for _ in 0..CHAIN_BYTES / 4 {
                bytes.extend(rgba);
            }
        }
        entries.insert(
            set.base_color_universal_uri.clone(),
            (
                set.normal_material_universal_uri.clone(),
                set.macro_variation_universal_uri.clone(),
                Inputs::decode(bytes).unwrap(),
            ),
        );
        Self::new(entries)
    }
}
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > limit {
        bail!("terrain bake file byte limit");
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("terrain bake file byte limit");
    }
    Ok(bytes)
}

/// 128-pixel mip chains of a texture set's surfaces, in array-layer order: base colours (each
/// surface's blend height in alpha), normal/material images, then the macro variation.
pub(super) struct Inputs {
    data: Vec<u8>,
    pub layers: usize,
    pub hash: [u8; 32],
}
impl Inputs {
    fn decode(data: Vec<u8>) -> Result<Self> {
        if data.len() < 56 || &data[..8] != b"YTRBAKE\0" {
            bail!("terrain bake input header");
        }
        let number = |i| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let layers = number(16) as usize;
        if number(8) != 2 {
            bail!("terrain bake input version; rebuild it with tools/prepare_terrain_bake.py");
        }
        if number(12) != SIZE as u32
            || !(1..=MAX_LAYERS).contains(&layers)
            || data.len() != 56 + (layers * 2 + 1) * CHAIN_BYTES
        {
            bail!("terrain bake input dimensions");
        }
        let hash = *blake3::hash(&data).as_bytes();
        Ok(Self { data, layers, hash })
    }
    fn chains(&self) -> usize {
        (self.data.len() - 56) / CHAIN_BYTES
    }
    pub fn base_color(&self, layer: usize) -> usize {
        layer
    }
    pub fn normal_material(&self, layer: usize) -> usize {
        self.chains() - 1 - self.layers + layer
    }
    pub fn macro_variation(&self) -> usize {
        self.chains() - 1
    }
    /// Bilinear alpha of a base colour's finest level, repeating: the surface's blend height
    /// at full detail. One channel of one level, for the many samples blending averages.
    pub fn height(&self, layer: usize, uv: [f64; 2]) -> f32 {
        let start = 56 + self.base_color(layer) * CHAIN_BYTES;
        let p = uv.map(|x| x.rem_euclid(1.) * SIZE as f64 - 0.5);
        let x = p[0].floor();
        let y = p[1].floor();
        let (fx, fy) = ((p[0] - x) as f32, (p[1] - y) as f32);
        let at = |dx: i32, dy: i32| {
            let column = (x as i32 + dx).rem_euclid(SIZE as i32) as usize;
            let row = (y as i32 + dy).rem_euclid(SIZE as i32) as usize;
            f32::from(self.data[start + (row * SIZE + column) * 4 + 3]) / 255.
        };
        let top = at(0, 0) + (at(1, 0) - at(0, 0)) * fx;
        let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * fx;
        top + (bottom - top) * fy
    }
    /// Repeat + bilinear + trilinear, matching source mip semantics. Coordinates
    /// remain f64 until wrapping; rebasing can never change the baked pattern.
    pub fn sample(&self, texture: usize, uv: [f64; 2], footprint: f64, color: bool) -> [f32; 4] {
        assert!(texture < self.chains());
        let lod = (footprint * SIZE as f64).max(1.).log2().clamp(0., 7.);
        let a = lod.floor() as usize;
        let b = (a + 1).min(7);
        let decode = decode_table();
        let sample = |level| {
            let n = SIZE >> level;
            let offset: usize = (0..level).map(|l| (SIZE >> l).pow(2) * 4).sum();
            let p = uv.map(|x| x.rem_euclid(1.) * n as f64 - 0.5);
            let base = p.map(|x| x.floor() as i32);
            let f = [p[0] - p[0].floor(), p[1] - p[1].floor()];
            let mut result = [0.; 4];
            for y in 0..2 {
                for x in 0..2 {
                    let index = ((base[1] + y).rem_euclid(n as i32) as usize * n
                        + (base[0] + x).rem_euclid(n as i32) as usize)
                        * 4;
                    let weight = (if x == 0 { 1. - f[0] } else { f[0] })
                        * (if y == 0 { 1. - f[1] } else { f[1] });
                    for (c, channel) in result.iter_mut().enumerate() {
                        let v = self.data[56 + texture * CHAIN_BYTES + offset + index + c];
                        let v = if color && c < 3 {
                            decode[usize::from(v)]
                        } else {
                            f32::from(v) / 255.
                        };
                        *channel += v * weight as f32;
                    }
                }
            }
            result
        };
        let lo = sample(a);
        let t = (lod - a as f64) as f32;
        if t == 0. || a == b {
            return lo;
        }
        let hi = sample(b);
        std::array::from_fn(|c| lo[c] + (hi[c] - lo[c]) * t)
    }
}
/// `linear` of every sRGB byte, as `sample` decodes texels.
fn decode_table() -> &'static [f32; 256] {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|i| linear(i as f32 / 255.)))
}
pub(super) fn linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
pub(super) fn srgb(v: f32) -> u8 {
    let v = v.clamp(0., 1.);
    ((if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }) * 255.)
        .round() as u8
}
