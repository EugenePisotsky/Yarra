//! The terrain pack worlds are created with: its surfaces in texture array order and the
//! texture set built from them, read from the tracked `assets/packs/terrain/baltic.toml`.
//! Surface parameters live in the project once a world exists; the pack only seeds them.
use serde::Deserialize;
use world::{TerrainSurface, TerrainSurfaceId, TerrainTextureLayer, TerrainTextureSet};

const MANIFEST: &str = include_str!("../../../assets/packs/terrain/baltic.toml");

#[derive(Deserialize)]
struct Manifest {
    pack_key: String,
    texture_size: u32,
    surfaces: Vec<Surface>,
}

/// The fields worlds read; the import tool also reads the scan sources and blend heights.
#[derive(Deserialize)]
struct Surface {
    key: String,
    name: String,
    tile_size: f32,
    normal_strength: f32,
    roughness: [f32; 2],
    anti_tiling: bool,
}

pub(crate) struct TerrainPack {
    pub texture_set: TerrainTextureSet,
    pub surfaces: Vec<TerrainSurface>,
    pub layers: Vec<TerrainTextureLayer>,
}

impl TerrainPack {
    pub fn baltic() -> Self {
        let manifest: Manifest =
            toml::from_str(MANIFEST).expect("the tracked terrain pack manifest parses");
        let pack = &manifest.pack_key;
        let root = format!("local/terrain/{pack}/runtime");
        let size = manifest.texture_size;
        let layers = manifest.surfaces.len() as u32;
        let texture_set = TerrainTextureSet {
            id: world::TerrainTextureSetId(super::stable_id(&format!("{pack}-texture-set"))),
            key: pack.clone(),
            base_color_universal_uri: format!("{root}/universal/base_color_array.ktx2"),
            normal_material_universal_uri: format!("{root}/universal/normal_material_array.ktx2"),
            macro_variation_universal_uri: format!("{root}/universal/macro_variation.ktx2"),
            base_color_astc_uri: format!("{root}/astc/base_color_array.ktx2"),
            normal_material_astc_uri: format!("{root}/astc/normal_material_array.ktx2"),
            macro_variation_astc_uri: format!("{root}/astc/macro_variation.ktx2"),
            // As `tools/compile_terrain_textures.py` reports them: UASTC transcodes to one byte
            // per texel; iOS keeps ASTC 8x8 colour and macro data and 4x4 normal/material.
            universal_gpu_bytes: mip_texels(size) * u64::from(layers) * 2 + mip_texels(1024),
            astc_gpu_bytes: mip_texels(size) * u64::from(layers) / 8
                + mip_texels(size) * u64::from(layers)
                + mip_texels(1024) / 8,
        };
        let surfaces: Vec<_> = manifest
            .surfaces
            .iter()
            .map(|s| TerrainSurface {
                id: Self::surface_id(pack, &s.key),
                key: s.key.clone(),
                display_name: s.name.clone(),
                tile_size: s.tile_size,
                anti_tiling: s.anti_tiling,
                // The import tool stores every normal in the engine's frame.
                normal_y_sign: 1.0,
                normal_strength: s.normal_strength,
                roughness_min: s.roughness[0],
                roughness_max: s.roughness[1],
            })
            .collect();
        let layers = surfaces
            .iter()
            .enumerate()
            .map(|(layer, s)| TerrainTextureLayer {
                texture_set: texture_set.id,
                surface: s.id,
                layer: layer as u16,
            })
            .collect();
        Self {
            texture_set,
            surfaces,
            layers,
        }
    }

    fn surface_id(pack: &str, key: &str) -> TerrainSurfaceId {
        TerrainSurfaceId(super::stable_id(&format!("{pack}/{key}")))
    }

    /// The surface with `key`; packs are tracked, so a missing key is a programming error.
    pub fn surface(&self, key: &str) -> TerrainSurfaceId {
        self.surfaces
            .iter()
            .find(|s| s.key == key)
            .unwrap_or_else(|| panic!("terrain pack has no {key:?} surface"))
            .id
    }

    /// Every surface ID, sorted as environment definitions list them.
    pub fn surface_ids(&self) -> Vec<TerrainSurfaceId> {
        let mut ids: Vec<_> = self.surfaces.iter().map(|s| s.id).collect();
        ids.sort();
        ids
    }
}

fn mip_texels(size: u32) -> u64 {
    (0..=size.ilog2())
        .map(|level| u64::from(size >> level).pow(2))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tracked_pack_seeds_surfaces_in_array_order() {
        let pack = TerrainPack::baltic();
        assert!(pack.surfaces.len() >= 8);
        for (i, layer) in pack.layers.iter().enumerate() {
            assert_eq!(layer.layer as usize, i);
            assert_eq!(layer.surface, pack.surfaces[i].id);
        }
        assert_ne!(pack.surface("beach-sand"), pack.surface("rock"));
        // compile_terrain_textures.py's mip chain, which reported 43,341,131 universal bytes
        // for fifteen 1024² layers.
        assert_eq!(mip_texels(1024), 1_398_101);
        assert_eq!(mip_texels(1024) * 31, 43_341_131);
    }
}
