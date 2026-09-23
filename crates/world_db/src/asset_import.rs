//! Bounded, transactional registration of locally converted scene assets.
use crate::{ProjectWriter, WorldDbError, collection_assets};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Component, Path},
};
use world::AssetId;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetImportCatalog {
    pub schema_version: u32,
    pub assets: Vec<AssetImport>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetImport {
    pub key: String,
    pub display_name: String,
    pub source_uri: String,
    pub variants: Vec<AssetImportVariant>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetImportVariant {
    pub uri: String,
    pub bounds: [f32; 3],
    /// Mesh buffer estimate, matching the source catalog's existing accounting.
    pub gpu_bytes_estimate: u64,
    pub minimum_screen_height: f32,
}

fn invalid(message: &str) -> WorldDbError {
    WorldDbError::Environment(format!("asset import: {message}"))
}
fn valid_uri(uri: &str) -> bool {
    !uri.is_empty()
        && uri.len() <= 4096
        && !uri.contains(['#', '\\', ':'])
        && Path::new(uri)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

impl AssetImportCatalog {
    pub fn validate(&self) -> Result<(), WorldDbError> {
        if self.schema_version != 1 || self.assets.is_empty() || self.assets.len() > 256 {
            return Err(invalid("expected version 1 with 1–256 assets"));
        }
        let mut keys = HashSet::new();
        for asset in &self.assets {
            if !valid_uri(&asset.key)
                || !keys.insert(&asset.key)
                || asset.display_name.trim().is_empty()
                || asset.display_name.len() > 256
                || !valid_uri(&asset.source_uri)
                || asset.variants.is_empty()
                || asset.variants.len() > 16
                || asset.variants.last().unwrap().minimum_screen_height != 0.0
                || asset
                    .variants
                    .windows(2)
                    .any(|v| v[0].minimum_screen_height < v[1].minimum_screen_height)
                || asset.variants.iter().any(|v| {
                    !valid_uri(&v.uri)
                        || !v.uri.ends_with(".gltf")
                        || v.bounds.iter().any(|b| !b.is_finite() || *b <= 0.0)
                        || !v.minimum_screen_height.is_finite()
                        || v.minimum_screen_height < 0.0
                        || v.gpu_bytes_estimate > i64::MAX as u64
                })
            {
                return Err(invalid(&format!(
                    "invalid asset or LOD chain: {}",
                    asset.key
                )));
            }
        }
        Ok(())
    }
}

impl ProjectWriter {
    /// Registers or refreshes only the supplied assets. Existing placements,
    /// definition aliases, collections and unrelated catalog rows are preserved.
    pub fn import_assets(&mut self, catalog: &AssetImportCatalog) -> Result<(), WorldDbError> {
        catalog.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for asset in &catalog.assets {
            let id = AssetId(*blake3::hash(asset.key.as_bytes()).as_bytes());
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT asset_id FROM source_assets WHERE asset_key=?1",
                    [&asset.key],
                    |r| r.get(0),
                )
                .optional()?;
            if existing.as_deref().is_some_and(|value| value != id.0) {
                return Err(invalid("asset key already has a different stable ID"));
            }
            tx.execute(
                "INSERT INTO source_assets(asset_id,asset_key,kind,source_uri) VALUES (?1,?2,'gltf-scene',?3) \
                 ON CONFLICT(asset_id) DO UPDATE SET source_uri=excluded.source_uri \
                 WHERE source_assets.asset_key=excluded.asset_key AND source_assets.kind='gltf-scene'",
                params![id.0.as_slice(), asset.key, asset.source_uri],
            ).and_then(|changed| if changed == 1 { Ok(()) } else { Err(rusqlite::Error::InvalidQuery) })?;
            tx.execute(
                "DELETE FROM source_asset_variants WHERE asset_id=?1",
                [id.0.as_slice()],
            )?;
            for (lod, v) in asset.variants.iter().enumerate() {
                tx.execute(
                    "INSERT INTO source_asset_variants(asset_id,lod,uri,bounds_x,bounds_y,bounds_z,gpu_bytes_estimate,shadow_policy,minimum_screen_height) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8)",
                    params![id.0.as_slice(), lod as i64, v.uri, v.bounds[0], v.bounds[1], v.bounds[2],
                            v.gpu_bytes_estimate as i64, v.minimum_screen_height],
                )?;
            }
            collection_assets::read_asset(&tx, id)?;
            let key = format!("asset/{}", asset.key);
            let hash = blake3::hash(key.as_bytes());
            let changed = tx.execute(
                "INSERT INTO object_definitions(definition_id,definition_key,display_name,visual_asset_id,activation_policy) \
                 VALUES (?1,?2,?3,?4,0) ON CONFLICT(definition_id) DO UPDATE SET display_name=excluded.display_name \
                 WHERE object_definitions.definition_key=excluded.definition_key \
                   AND object_definitions.visual_asset_id=excluded.visual_asset_id AND object_definitions.activation_policy=0",
                params![&hash.as_bytes()[..16], key, asset.display_name, id.0.as_slice()],
            )?;
            if changed != 1 {
                return Err(invalid("definition identity conflict"));
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> AssetImportCatalog {
        AssetImportCatalog {
            schema_version: 1,
            assets: vec![AssetImport {
                key: "forest/tree".into(),
                display_name: "Tree".into(),
                source_uri: "local/tree.fbx".into(),
                variants: vec![AssetImportVariant {
                    uri: "local/tree.gltf".into(),
                    bounds: [2., 10., 2.],
                    gpu_bytes_estimate: 100,
                    minimum_screen_height: 0.,
                }],
            }],
        }
    }
    fn writer() -> ProjectWriter {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::schema::PROJECT_SCHEMA)
            .unwrap();
        ProjectWriter { connection }
    }
    #[test]
    fn repeated_import_keeps_stable_identity_and_unrelated_definitions() {
        let mut w = writer();
        let mut c = catalog();
        w.import_assets(&c).unwrap();
        w.connection.execute("INSERT INTO object_definitions SELECT zeroblob(16),'existing-tree','Existing',visual_asset_id,0 FROM object_definitions", []).unwrap();
        c.assets[0].display_name = "Summer tree".into();
        w.import_assets(&c).unwrap();
        let count: i64 = w
            .connection
            .query_row("SELECT count(*) FROM source_assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let count: i64 = w
            .connection
            .query_row("SELECT count(*) FROM object_definitions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let id = AssetId(*blake3::hash(b"forest/tree").as_bytes());
        assert_eq!(
            collection_assets::read_asset(&w.connection, id)
                .unwrap()
                .variants
                .len(),
            1
        );
    }
    #[test]
    fn later_identity_conflict_rolls_back_earlier_imports() {
        let mut w = writer();
        let mut c = catalog();
        c.assets.push(AssetImport {
            key: "forest/conflict".into(),
            ..catalog().assets.remove(0)
        });
        w.connection.execute("INSERT INTO source_assets VALUES (zeroblob(32),'forest/conflict','gltf-scene','old.fbx')", []).unwrap();
        assert!(w.import_assets(&c).is_err());
        let count: i64 = w
            .connection
            .query_row("SELECT count(*) FROM source_assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
    #[test]
    fn rejects_missing_final_lod_and_paths_outside_asset_root() {
        let mut c = catalog();
        c.assets[0].variants[0].minimum_screen_height = 80.;
        assert!(c.validate().is_err());
        c.assets[0].variants[0].minimum_screen_height = 0.;
        c.assets[0].variants[0].uri = "../tree.gltf".into();
        assert!(c.validate().is_err());
    }
}
