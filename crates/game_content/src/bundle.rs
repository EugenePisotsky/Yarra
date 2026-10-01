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
pub const BUNDLE_SCHEMA_VERSION: i64 = 13;
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
    hasher.update(b"yarra-game-content-v10\0");
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
            ContentId::random()
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
                .values()
                .cloned()
                .map(Asset::Category)
        );
        append!(self.content.items.items.values().cloned().map(Asset::Item));
        append!(self.content.game.actors.values().cloned().map(Asset::Actor));
        append!(
            self.content
                .game
                .characters
                .values()
                .cloned()
                .map(Asset::Character)
        );
        assets.push((0, Asset::Rules(self.content.game.rules.clone())));
        append!(
            self.content
                .game
                .dialogues
                .values()
                .cloned()
                .map(Asset::Dialogue)
        );
        append!(
            self.content
                .game
                .variables
                .values()
                .cloned()
                .map(Asset::Variable)
        );
        append!(self.content.text.values().cloned().map(Asset::Text));
        append!(
            self.content
                .game
                .scripts
                .values()
                .cloned()
                .map(Asset::Script)
        );
        append!(self.content.game.loot.values().cloned().map(Asset::Loot));
        append!(
            self.content
                .game
                .dialogue_contracts
                .values()
                .cloned()
                .map(Asset::DialogueContract)
        );
        append!(
            self.content
                .game
                .world
                .objects
                .values()
                .cloned()
                .map(Asset::Object)
        );
        append!(
            self.content
                .game
                .world
                .areas
                .iter()
                .copied()
                .map(Asset::Area)
        );
        append!(
            self.content
                .game
                .world
                .triggers
                .values()
                .cloned()
                .map(Asset::Trigger)
        );
        append!(self.content.game.quests.values().cloned().map(Asset::Quest));
        append!(
            self.content
                .game
                .profiles
                .values()
                .cloned()
                .map(Asset::Profile)
        );
        append!(
            self.content
                .game
                .predicates
                .values()
                .cloned()
                .map(Asset::Predicate)
        );
        assets
    }
}
