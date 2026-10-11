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
    fn tracked_pack_grows_grass_on_the_meadows_but_none_on_rock_or_beach() {
        let terrain = TerrainPack::baltic();
        let catalog = baltic(&terrain);
        let grass = |key: &str| {
            catalog
                .on_surface(terrain.surface(key).0)
                .iter()
                .filter(|a| catalog.grass(a.grass).is_some())
                .count()
        };
        assert!(grass("meadow") > 0);
        assert_eq!(grass("rock"), 0);
        assert_eq!(grass("beach-sand"), 0);
        // Beaches keep their pebbles; every listed detail is in the catalog.
        assert!(
            catalog
                .on_surface(terrain.surface("beach-sand").0)
                .iter()
                .all(|a| catalog.detail(a.grass).is_some())
        );
    }
}
