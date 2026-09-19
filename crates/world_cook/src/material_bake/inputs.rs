use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};
use world::TerrainTextureSet;

const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;
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
        Ok(Self { entries })
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
    #[cfg(test)]
    pub(super) fn fixture(set: &TerrainTextureSet) -> Self {
        let mut entries = BTreeMap::new();
        let mut bytes = b"YTRBAKE\0".to_vec();
        for n in [1_u32, 128, 2] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend(4_f32.to_le_bytes());
        bytes.extend([0; 32]);
        // Contrasting known linear-light means; BA supplies AO/roughness data.
        for rgba in [
            [40, 130, 25, 255],
            [180, 120, 70, 255],
            [40, 130, 25, 255],
            [180, 120, 70, 255],
            [128, 128, 255, 100],
            [128, 128, 200, 200],
            [128, 128, 128, 255],
        ] {
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
        Self { entries }
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

pub(super) struct Inputs {
    data: Vec<u8>,
    pub layers: usize,
    pub period: f64,
    pub hash: [u8; 32],
}
impl Inputs {
    fn decode(data: Vec<u8>) -> Result<Self> {
        if data.len() < 56 || &data[..8] != b"YTRBAKE\0" {
            bail!("terrain bake input header");
        }
        let number = |i| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let layers = number(16) as usize;
        let period = f32::from_bits(number(20)) as f64;
        if number(8) != 1
            || number(12) != SIZE as u32
            || !(1..=8).contains(&layers)
            || !period.is_finite()
            || !(2.0..=16.0).contains(&period)
            || data.len() != 56 + (layers * 3 + 1) * CHAIN_BYTES
        {
            bail!("terrain bake input dimensions/version/period");
        }
        let hash = *blake3::hash(&data).as_bytes();
        Ok(Self {
            data,
            layers,
            period,
            hash,
        })
    }
    /// Repeat + bilinear + trilinear, matching source mip semantics. Coordinates
    /// remain f64 until wrapping; rebasing can never change the baked pattern.
    pub fn sample(&self, texture: usize, uv: [f64; 2], footprint: f64, color: bool) -> [f32; 4] {
        assert!(texture < self.layers * 3 + 1);
        let lod = (footprint * SIZE as f64).max(1.).log2().clamp(0., 7.);
        let a = lod.floor() as usize;
        let b = (a + 1).min(7);
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
                        let v = self.data[56 + texture * CHAIN_BYTES + offset + index + c] as f32
                            / 255.;
                        *channel += (if color && c < 3 { linear(v) } else { v }) * weight as f32;
                    }
                }
            }
            result
        };
        let lo = sample(a);
        let hi = sample(b);
        std::array::from_fn(|c| lo[c] + (hi[c] - lo[c]) * (lod - a as f64) as f32)
    }
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
