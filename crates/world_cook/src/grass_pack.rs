//! The grass worlds are created with, read from the tracked `assets/packs/grass/baltic.ron`:
//! its types and what grows on each surface of the terrain pack. Like terrain surfaces, the
//! catalog lives in the project once a world exists; `import-grass` replaces it from the pack.
use super::TerrainPack;
use vegetation::{GrassPack, VegetationCatalog};

const PACK: &str = include_str!("../../../assets/packs/grass/baltic.ron");

/// The tracked pack's catalog for the terrain pack's surfaces.
pub fn baltic(terrain: &TerrainPack) -> VegetationCatalog {
    catalog(PACK, terrain).expect("the tracked grass pack matches the terrain pack")
}

/// The catalog of the grass pack `text` for the terrain pack's surfaces.
pub fn catalog(text: &str, terrain: &TerrainPack) -> Result<VegetationCatalog, String> {
    let pack = GrassPack::parse(text)?;
    if pack.terrain != terrain.texture_set.key {
        return Err(format!(
            "grass pack is for terrain pack {}, not {}",
            pack.terrain, terrain.texture_set.key
        ));
    }
    pack.catalog(|key| {
        terrain
            .surfaces
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.id.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_pack_grows_on_the_meadows_and_leaves_rock_bare() {
        let terrain = TerrainPack::baltic();
        let catalog = baltic(&terrain);
        assert!(!catalog.on_surface(terrain.surface("meadow").0).is_empty());
        assert!(catalog.on_surface(terrain.surface("rock").0).is_empty());
        assert!(
            catalog
                .on_surface(terrain.surface("beach-sand").0)
                .is_empty()
        );
    }
}
