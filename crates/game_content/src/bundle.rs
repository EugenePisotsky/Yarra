//! Publishing a project as a bundle: the manifest, one row with every definition a session
//! reads up front, and a row per dialogue graph and per text contract, read when needed.
use crate::{LoadedProject, Result, sqlite};
use gameplay::inventory::ItemCatalog;
use gameplay::{ContentManifest, ContentSource, GameDefinitions};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const BUNDLE_APPLICATION_ID: i64 = 0x59474342;
pub const BUNDLE_SCHEMA_VERSION: i64 = 14;

const SCHEMA: &str = "
CREATE TABLE manifest (singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload BLOB NOT NULL) STRICT;
CREATE TABLE core (singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload BLOB NOT NULL) STRICT;
CREATE TABLE dialogues (id TEXT PRIMARY KEY, payload BLOB NOT NULL) STRICT, WITHOUT ROWID;
CREATE TABLE texts (id TEXT PRIMARY KEY, payload BLOB NOT NULL) STRICT, WITHOUT ROWID;
";

/// What a bundle says about itself, read when it is opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub content: ContentManifest,
    pub source_locale: String,
    /// The mechanics' identity, which saves record; see `AuthoredContent::fingerprint`.
    pub fingerprint: [u8; 32],
}

/// The core row: every definition a session reads up front.
#[derive(Serialize)]
struct CoreRow<'a> {
    items: &'a ItemCatalog,
    game: &'a GameDefinitions,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Core {
    pub items: ItemCatalog,
    pub game: GameDefinitions,
}

impl LoadedProject {
    /// Publishes the project as a new bundle; an existing file is never replaced.
    pub fn build(&self, output: impl AsRef<Path>) -> Result<()> {
        let content = self.tool_content();
        let authored = content.authored();
        let manifest = BundleManifest {
            content: authored.core.manifest.clone(),
            source_locale: self.source_locale.clone(),
            fingerprint: content.identity().fingerprint,
        };
        let core = CoreRow {
            items: &authored.core.items,
            game: &authored.core.game,
        };

        sqlite::publish(
            output.as_ref(),
            BUNDLE_APPLICATION_ID,
            BUNDLE_SCHEMA_VERSION,
            SCHEMA,
            |tx| {
                tx.execute(
                    "INSERT INTO manifest VALUES (1, ?1)",
                    [serde_json::to_vec(&manifest)?],
                )?;
                tx.execute(
                    "INSERT INTO core VALUES (1, ?1)",
                    [serde_json::to_vec(&core)?],
                )?;

                for graph in authored.dialogues.values() {
                    tx.execute(
                        "INSERT INTO dialogues VALUES (?1, ?2)",
                        params![graph.id.raw(), serde_json::to_vec(graph)?],
                    )?;
                }

                for contract in authored.text.values() {
                    tx.execute(
                        "INSERT INTO texts VALUES (?1, ?2)",
                        params![contract.id.raw(), serde_json::to_vec(contract)?],
                    )?;
                }

                Ok(())
            },
        )
    }
}
