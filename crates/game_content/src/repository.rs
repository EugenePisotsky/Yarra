//! Reading one published bundle. Opening reads only the manifest; a session reads the core
//! once and each dialogue graph when a conversation first needs it.
use crate::{
    BUNDLE_APPLICATION_ID, BUNDLE_SCHEMA_VERSION, ContentError, Result,
    bundle::{BundleManifest, Core},
    sqlite,
};

use game_types::{DialogueId, Invalid, TextContract, TextResourceId};
use gameplay::dialogue::Dialogue;
use gameplay::{ContentIdentity, ContentSource, GameContent, Graphs};
use rusqlite::{Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use std::path::Path;

pub struct ContentRepository {
    connection: Connection,
    manifest: BundleManifest,
}

impl ContentRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let (connection, version) = sqlite::open(path.as_ref(), BUNDLE_APPLICATION_ID)?;

        if version != BUNDLE_SCHEMA_VERSION {
            return Err(ContentError::BundleVersion(version));
        }

        let manifest: BundleManifest = row(&connection, "SELECT payload FROM manifest", [])?
            .ok_or_else(|| ContentError::NotInBundle("manifest".into()))?;
        localization::validate_locale(&manifest.source_locale)?;
        Ok(Self {
            connection,
            manifest,
        })
    }

    pub fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }

    /// The definitions a session reads up front, with their scripts compiled, checked once.
    pub fn read_core(&self) -> Result<GameContent> {
        let core: Core = row(&self.connection, "SELECT payload FROM core", [])?
            .ok_or_else(|| ContentError::NotInBundle("core".into()))?;
        let scripts =
            scripting::LuauScripts::install(core.game.scripts.values()).map_err(Invalid)?;
        let content = GameContent {
            manifest: self.manifest.content.clone(),
            items: core.items,
            game: core.game,
            scripts,
            graphs: Graphs::default(),
        };
        content.validate()?;
        Ok(content)
    }

    pub fn read_dialogue(&self, id: DialogueId) -> Result<Dialogue> {
        let sql = "SELECT payload FROM dialogues WHERE id=?1";
        row(&self.connection, sql, [id.raw()])?
            .ok_or_else(|| ContentError::NotInBundle(format!("dialogue {id}")))
    }

    pub fn read_text(&self, id: TextResourceId) -> Result<TextContract> {
        let sql = "SELECT payload FROM texts WHERE id=?1";
        row(&self.connection, sql, [id.raw()])?
            .ok_or_else(|| ContentError::NotInBundle(format!("text {id}")))
    }

    /// Every text contract, for tools that check wording against the bundle.
    pub fn read_texts(&self) -> Result<Vec<TextContract>> {
        let mut statement = self
            .connection
            .prepare("SELECT payload FROM texts ORDER BY id")?;
        let payloads = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        let mut texts = Vec::new();

        for payload in payloads {
            texts.push(serde_json::from_slice(&payload?)?);
        }

        Ok(texts)
    }

    /// Reads everything a session could, the core and then every graph against its
    /// contract, as a tool checks a bundle. Returns the core and how many graphs it has.
    pub fn check(self) -> Result<(GameContent, usize)> {
        let ids: Vec<String> = self
            .connection
            .prepare("SELECT id FROM dialogues ORDER BY id")?
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut content = self.read_core()?;
        content.graphs = Graphs::reading(move |id| self.read_dialogue(id).map_err(runtime_error));

        for id in &ids {
            content.dialogue(DialogueId::try_from(id.clone())?)?;
        }

        Ok((content, ids.len()))
    }
}

/// One row's JSON payload, if the row is there.
fn row<T: DeserializeOwned>(
    connection: &Connection,
    sql: &str,
    key: impl rusqlite::Params,
) -> Result<Option<T>> {
    let payload: Option<Vec<u8>> = connection
        .query_row(sql, key, |row| row.get(0))
        .optional()?;
    Ok(payload
        .map(|payload| serde_json::from_slice(&payload))
        .transpose()?)
}

fn runtime_error(error: ContentError) -> gameplay::GameplayError {
    gameplay::GameplayError::Runtime(error.to_string())
}

impl ContentSource for ContentRepository {
    fn identity(&self) -> ContentIdentity {
        ContentIdentity {
            manifest: self.manifest.content.clone(),
            fingerprint: self.manifest.fingerprint,
        }
    }

    fn core(&mut self) -> gameplay::Result<GameContent> {
        self.read_core().map_err(runtime_error)
    }

    fn dialogue(&mut self, id: DialogueId) -> gameplay::Result<Dialogue> {
        self.read_dialogue(id).map_err(runtime_error)
    }
}
