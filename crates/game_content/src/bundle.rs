use crate::{LoadedProject, Result, asset::*, project::MAX_DOCUMENT_BYTES};
use game_types::{CatalogId, ContentId, require};
use gameplay::ContentManifest;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    path::Path,
};

pub const BUNDLE_APPLICATION_ID: i64 = 0x59474342;
pub const BUNDLE_SCHEMA_VERSION: i64 = 6;
const SCHEMA: &str = "
CREATE TABLE bundle_manifest (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 payload BLOB NOT NULL CHECK(length(payload)<=65536)
) STRICT;
CREATE TABLE assets (
 kind INTEGER NOT NULL CHECK(kind BETWEEN 1 AND 17),
 id TEXT NOT NULL CHECK(length(CAST(id AS BLOB)) BETWEEN 1 AND 192),
 position INTEGER NOT NULL CHECK(position>=0),
 byte_len INTEGER NOT NULL CHECK(byte_len BETWEEN 1 AND 2097152),
 hash BLOB NOT NULL CHECK(length(hash)=32),
 payload BLOB NOT NULL CHECK(length(payload)=byte_len),
 PRIMARY KEY(kind,id), UNIQUE(kind,position)
) STRICT, WITHOUT ROWID;
CREATE TABLE asset_dependencies (
 kind INTEGER NOT NULL, id TEXT NOT NULL,
 dependency TEXT NOT NULL CHECK(length(CAST(dependency AS BLOB))<=512),
 target_kind INTEGER NOT NULL, target_id TEXT NOT NULL,
 PRIMARY KEY(kind,id,dependency),
 FOREIGN KEY(kind,id) REFERENCES assets(kind,id),
 FOREIGN KEY(target_kind,target_id) REFERENCES assets(kind,id)
) STRICT, WITHOUT ROWID;
CREATE INDEX dependency_targets ON asset_dependencies(target_kind,target_id);
CREATE TABLE asset_links (
 kind INTEGER NOT NULL, id TEXT NOT NULL, target_kind INTEGER NOT NULL, target_id TEXT NOT NULL,
 PRIMARY KEY(kind,id,target_kind,target_id),
 FOREIGN KEY(kind,id) REFERENCES assets(kind,id),
 FOREIGN KEY(target_kind,target_id) REFERENCES assets(kind,id)
) STRICT, WITHOUT ROWID;
CREATE INDEX link_targets ON asset_links(target_kind,target_id);
CREATE TABLE trigger_subscriptions(signal TEXT NOT NULL, trigger TEXT NOT NULL, kind INTEGER NOT NULL DEFAULT 17 CHECK(kind=17), PRIMARY KEY(signal,trigger), FOREIGN KEY(kind,trigger) REFERENCES assets(kind,id)) STRICT, WITHOUT ROWID;
CREATE INDEX subscriptions_by_trigger ON trigger_subscriptions(trigger,signal);
CREATE TABLE area_bounds(id TEXT PRIMARY KEY, min_x INTEGER NOT NULL, max_x INTEGER NOT NULL, min_y INTEGER NOT NULL, max_y INTEGER NOT NULL, min_z INTEGER NOT NULL, max_z INTEGER NOT NULL, kind INTEGER NOT NULL DEFAULT 16 CHECK(kind=16), FOREIGN KEY(kind,id) REFERENCES assets(kind,id)) STRICT, WITHOUT ROWID;
CREATE INDEX areas_by_x ON area_bounds(min_x,max_x);
CREATE TABLE tool_scenario (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 payload BLOB NOT NULL CHECK(length(payload)<=16777216)
) STRICT;
";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub content: ContentManifest,
    pub catalog_id: CatalogId,
    pub catalog_revision: u64,
    pub source_locale: String,
    /// Mechanical identity used by the current save contract; excludes translations/scenario.
    pub mechanical_fingerprint: [u8; 32],
    /// Identity of this entire immutable publication, including text contracts and tool scenario.
    pub publication_hash: [u8; 32],
    pub scenario_hash: [u8; 32],
}
impl BundleManifest {
    pub(crate) fn validate(&self) -> Result<()> {
        require(
            self.content.revision > 0
                && !self.content.world_generation.is_empty()
                && self.content.world_generation.len() <= 256
                && self.catalog_revision > 0
                && self.catalog_revision <= i64::MAX as u64,
            "invalid bundle manifest",
        )?;
        validate_locale(&self.source_locale)
    }
}
pub(crate) fn publication_hash(
    manifest: &BundleManifest,
    headers: &[AssetHeader],
) -> Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"yarra-game-content-v6\0");
    hasher.update(&serde_json::to_vec(&(
        &manifest.content,
        manifest.catalog_id,
        manifest.catalog_revision,
        &manifest.source_locale,
        manifest.mechanical_fingerprint,
        manifest.scenario_hash,
    ))?);
    let mut sorted: Vec<_> = headers.iter().collect();
    sorted.sort_by_key(|h| &h.id);
    for header in sorted {
        let encoded = serde_json::to_vec(header)?;
        hasher.update(&(encoded.len() as u64).to_le_bytes());
        hasher.update(&encoded);
    }
    Ok(*hasher.finalize().as_bytes())
}

impl LoadedProject {
    /// Builds one new immutable publication. Source materialization is an authoring operation.
    pub fn build(&self, output: impl AsRef<Path>) -> Result<()> {
        let output = output.as_ref();
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let stage = parent.join(format!(
            ".game-content-{}.sqlite.building",
            ContentId::new()
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        let result: Result<()> = (|| {
            let mut connection =
                Connection::open_with_flags(&stage, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
            let tx = connection.transaction()?;
            tx.execute_batch(SCHEMA)?;
            let assets = self.assets();
            let mut ids = BTreeSet::new();
            let mut headers = Vec::new();
            for (position, asset) in &assets {
                asset.validate_local()?;
                let id = asset.id();
                AssetId::from_key(id.kind(), id.key())?;
                require(ids.insert(id.clone()), "duplicate published asset identity")?;
                let payload = serde_json::to_vec(asset)?;
                require(
                    payload.len() <= MAX_ASSET_BYTES,
                    "asset exceeds 2 MiB publication limit",
                )?;
                let header = AssetHeader {
                    id,
                    position: *position,
                    payload_bytes: payload.len(),
                    hash: *blake3::hash(&payload).as_bytes(),
                };
                tx.execute(
                    "INSERT INTO assets VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        header.id.kind() as i64,
                        header.id.key(),
                        position,
                        payload.len() as i64,
                        header.hash,
                        payload
                    ],
                )?;
                headers.push(header);
            }
            for (_, asset) in &assets {
                let id = asset.id();
                for target in asset.selection_links() {
                    require(ids.contains(&target), "missing selection target")?;
                    tx.execute(
                        "INSERT OR IGNORE INTO asset_links VALUES (?1,?2,?3,?4)",
                        params![
                            id.kind() as i64,
                            id.key(),
                            target.kind() as i64,
                            target.key()
                        ],
                    )?;
                }
                for dependency in asset.dependencies()? {
                    require(ids.contains(&dependency), "missing published dependency")?;
                    tx.execute(
                        "INSERT INTO asset_dependencies VALUES (?1,?2,?3,?4,?5)",
                        params![
                            id.kind() as i64,
                            id.key(),
                            serde_json::to_string(&dependency)?,
                            dependency.kind() as i64,
                            dependency.key()
                        ],
                    )?;
                }
            }
            let scenario = serde_json::to_vec(&self.scenario)?;
            require(
                scenario.len() <= MAX_DOCUMENT_BYTES,
                "scenario exceeds 16 MiB",
            )?;
            let mut manifest = BundleManifest {
                content: self.content.manifest.clone(),
                catalog_id: self.content.items.id,
                catalog_revision: self.content.items.revision,
                source_locale: self.source_locale.clone(),
                mechanical_fingerprint: self.content.fingerprint()?,
                publication_hash: [0; 32],
                scenario_hash: *blake3::hash(&scenario).as_bytes(),
            };
            manifest.validate()?;
            manifest.publication_hash = publication_hash(&manifest, &headers)?;
            tx.execute(
                "INSERT INTO bundle_manifest VALUES (1,?1)",
                [serde_json::to_vec(&manifest)?],
            )?;
            tx.execute("INSERT INTO tool_scenario VALUES (1,?1)", [scenario])?;
            tx.pragma_update(None, "application_id", BUNDLE_APPLICATION_ID)?;
            for d in &self.content.game.world.triggers {
                for signal in d.subscriptions(&self.content)? {
                    tx.execute(
                        "INSERT INTO trigger_subscriptions(signal,trigger) VALUES(?1,?2)",
                        params![serde_json::to_string(&signal)?, d.id.to_string()],
                    )?;
                }
            }
            for a in &self.content.game.world.areas {
                let lo = a.min.millimetres;
                let hi = a.max.millimetres;
                tx.execute("INSERT INTO area_bounds(id,min_x,max_x,min_y,max_y,min_z,max_z) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![a.id.to_string(),lo[0],hi[0],lo[1],hi[1],lo[2],hi[2]])?;
            }
            tx.pragma_update(None, "user_version", BUNDLE_SCHEMA_VERSION)?;
            tx.commit()?;
            connection.close().map_err(|(_, error)| error)?;
            // Closed sibling file: atomic publication that refuses existing targets.
            fs::hard_link(&stage, output)?;
            Ok(())
        })();
        let cleanup = fs::remove_file(&stage);
        result?;
        cleanup?;
        Ok(())
    }
    pub(crate) fn assets(&self) -> Vec<(u32, Asset)> {
        let mut assets = Vec::new();
        macro_rules! append {
            ($iter:expr) => {
                assets.extend($iter.enumerate().map(|(i, a)| (i as u32, a)));
            };
        }
        append!(
            self.content
                .items
                .categories
                .iter()
                .cloned()
                .map(Asset::Category)
        );
        append!(self.content.items.items.iter().cloned().map(Asset::Item));
        append!(self.content.game.actors.iter().cloned().map(Asset::Actor));
        assets.push((0, Asset::Rules(self.content.game.rules.clone())));
        append!(
            self.content
                .game
                .dialogues
                .iter()
                .cloned()
                .map(Asset::Dialogue)
        );
        append!(self.content.game.facts.iter().cloned().map(Asset::Fact));
        append!(
            self.content
                .game
                .conditions
                .iter()
                .map(|(id, value)| Asset::Condition {
                    id: id.clone(),
                    value: value.clone()
                })
        );
        append!(
            self.content
                .game
                .actions
                .iter()
                .map(|(id, value)| Asset::Action {
                    id: id.clone(),
                    value: value.clone()
                })
        );
        append!(self.content.text.iter().cloned().map(Asset::Text));
        append!(
            self.content
                .game
                .dialogue_contracts
                .iter()
                .cloned()
                .map(Asset::DialogueContract)
        );
        append!(
            self.content
                .game
                .world
                .objects
                .iter()
                .cloned()
                .map(Asset::Object)
        );
        append!(
            self.content
                .game
                .world
                .areas
                .iter()
                .cloned()
                .map(Asset::Area)
        );
        append!(
            self.content
                .game
                .world
                .triggers
                .iter()
                .cloned()
                .map(Asset::Trigger)
        );
        append!(self.content.game.claims.iter().cloned().map(Asset::Claim));
        append!(self.content.game.quests.iter().cloned().map(Asset::Quest));
        append!(
            self.content
                .game
                .profiles
                .iter()
                .cloned()
                .map(Asset::Profile)
        );
        append!(
            self.content
                .game
                .predicates
                .iter()
                .cloned()
                .map(Asset::Predicate)
        );
        assets
    }
}
