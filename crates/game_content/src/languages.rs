//! Wording published on its own, a pack per locale. A save's identity never includes it, so a
//! translation can ship or change without touching the mechanics.
use crate::{ContentError, ContentRepository, LoadedProject, Result, sqlite};
use game_types::{ContentId, TextContract, TextResourceId, require};
use localization::{LanguageResource, LocalizationError, ResourceSource};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

const APPLICATION_ID: i64 = 0x59474c50;
const VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE manifest (singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload BLOB NOT NULL) STRICT;
CREATE TABLE resources (id TEXT PRIMARY KEY, payload BLOB NOT NULL) STRICT, WITHOUT ROWID;
";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    content: ContentId,
    locale: String,
    source_locale: String,
}

pub struct LanguageRepository {
    connection: Connection,
    manifest: Manifest,
}

impl LanguageRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let (connection, version) = sqlite::open(path.as_ref(), APPLICATION_ID)?;
        require(version == VERSION, "unsupported language pack version")?;
        let payload: Vec<u8> =
            connection.query_row("SELECT payload FROM manifest", [], |row| row.get(0))?;
        let manifest: Manifest = serde_json::from_slice(&payload)?;
        localization::validate_locale(&manifest.locale)?;
        Ok(Self {
            connection,
            manifest,
        })
    }

    pub fn locale(&self) -> &str {
        &self.manifest.locale
    }

    /// One resource's wording, read when it is first needed.
    pub fn load(&self, id: TextResourceId) -> Result<Option<LanguageResource>> {
        let payload: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT payload FROM resources WHERE id=?1",
                [id.raw()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        let resource: LanguageResource = serde_json::from_slice(&payload)?;
        require(
            resource.id == id && resource.locale == self.manifest.locale,
            "language resource identity mismatch",
        )?;
        Ok(Some(resource))
    }
}

/// A bundle's text contracts with the wording of its language packs, for presentation.
pub struct LanguageSource {
    content: ContentRepository,
    languages: BTreeMap<String, LanguageRepository>,
}

impl LanguageSource {
    pub fn new(content: ContentRepository, languages: Vec<LanguageRepository>) -> Result<Self> {
        let mut by_locale = BTreeMap::new();

        for language in languages {
            require(
                language.manifest.content == content.manifest().content.id
                    && language.manifest.source_locale == content.manifest().source_locale,
                "language pack belongs to another project or source locale",
            )?;
            require(
                by_locale
                    .insert(language.manifest.locale.clone(), language)
                    .is_none(),
                "duplicate language pack locale",
            )?;
        }

        Ok(Self {
            content,
            languages: by_locale,
        })
    }
}

fn presentation_error(error: ContentError) -> LocalizationError {
    LocalizationError::Resource(error.to_string())
}

impl ResourceSource for LanguageSource {
    fn contract(&mut self, id: TextResourceId) -> localization::Result<TextContract> {
        self.content.read_text(id).map_err(presentation_error)
    }

    fn resource(
        &mut self,
        id: TextResourceId,
        locale: &str,
    ) -> localization::Result<Option<LanguageResource>> {
        self.languages
            .get(locale)
            .map_or(Ok(None), |pack| pack.load(id).map_err(presentation_error))
    }
}

impl LoadedProject {
    /// Publishes one locale on its own: a change of wording does not rebuild the mechanics.
    pub fn build_language_pack(&self, locale: &str, output: impl AsRef<Path>) -> Result<()> {
        let resources: Vec<_> = self
            .translations
            .iter()
            .filter(|r| r.locale == locale)
            .collect();
        require(!resources.is_empty(), "locale has no resources")?;
        let manifest = Manifest {
            content: self.content().manifest.id,
            locale: locale.into(),
            source_locale: self.source_locale.clone(),
        };

        sqlite::publish(output.as_ref(), APPLICATION_ID, VERSION, SCHEMA, |tx| {
            tx.execute(
                "INSERT INTO manifest VALUES (1, ?1)",
                [serde_json::to_vec(&manifest)?],
            )?;

            for resource in resources {
                tx.execute(
                    "INSERT INTO resources VALUES (?1, ?2)",
                    params![resource.id.raw(), serde_json::to_vec(resource)?],
                )?;
            }

            Ok(())
        })
    }
}
