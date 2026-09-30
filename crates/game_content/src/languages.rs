//! Independently published wording. A mechanical save identity never includes these databases.
use crate::*;
use game_types::{ContentId, TextContract, TextResourceId, require};
use localization::{LanguageResource, LocalizationError, ResourceSource};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};
const APPLICATION_ID: i64 = 0x59474c50;
const VERSION: i64 = 1;
const MAX_PACK_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    content: ContentId,
    locale: String,
    source_locale: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageStats {
    pub payload_queries: usize,
    pub decoded_resources: usize,
    pub payload_bytes: usize,
}
pub struct LanguageRepository {
    connection: Connection,
    manifest: Manifest,
    stats: LanguageStats,
}
impl LanguageRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        require(
            fs::metadata(&path)?.len() <= MAX_PACK_BYTES,
            "language pack exceeds 256 MiB",
        )?;
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.execute_batch(
            "PRAGMA query_only=ON; PRAGMA cache_size=-512; PRAGMA mmap_size=0; BEGIN;",
        )?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        require(
            application == APPLICATION_ID && version == VERSION,
            "unsupported language pack database",
        )?;
        let bytes: Vec<u8> = connection.query_row("SELECT CASE WHEN length(payload)<=65536 THEN payload END FROM manifest WHERE singleton=1", [], |r| r.get(0))?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        asset::validate_locale(&manifest.locale)?;
        asset::validate_locale(&manifest.source_locale)?;
        Ok(Self {
            connection,
            manifest,
            stats: LanguageStats::default(),
        })
    }
    pub fn locale(&self) -> &str {
        &self.manifest.locale
    }
    pub fn stats(&self) -> &LanguageStats {
        &self.stats
    }
    pub fn load(&mut self, id: TextResourceId) -> Result<Option<LanguageResource>> {
        self.stats.payload_queries += 1;
        let row: Option<(Vec<u8>,Vec<u8>)> = self.connection.query_row(
            "SELECT CASE WHEN length(payload)<=2097152 THEN payload END, hash FROM resources WHERE id=?1", [id.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((payload, hash)) = row else {
            return Ok(None);
        };
        require(
            blake3::hash(&payload).as_bytes().as_slice() == hash.as_slice(),
            "language resource checksum mismatch",
        )?;
        let resource: LanguageResource = serde_json::from_slice(&payload)?;
        require(
            resource.id == id && resource.locale == self.manifest.locale,
            "language resource identity mismatch",
        )?;
        self.stats.decoded_resources += 1;
        self.stats.payload_bytes += payload.len();
        Ok(Some(resource))
    }
    fn all_for_tools(&mut self) -> Result<Vec<LanguageResource>> {
        let ids: Vec<String> = self
            .connection
            .prepare("SELECT id FROM resources ORDER BY id LIMIT 2049")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        require(ids.len() <= 2048, "too many language resources")?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for id in ids {
            let resource = self
                .load(TextResourceId::try_from(id)?)?
                .ok_or_else(|| game_types::Invalid("missing language resource".into()))?;
            bytes += resource.source.len();
            require(bytes <= 16 * 1024 * 1024, "tool language budget exceeded")?;
            result.push(resource);
        }
        Ok(result)
    }
}
pub struct LanguageSource {
    content: ContentRepository,
    languages: BTreeMap<String, LanguageRepository>,
}
impl LanguageSource {
    pub fn new(content: ContentRepository, languages: Vec<LanguageRepository>) -> Result<Self> {
        let mut by_locale = BTreeMap::new();
        require(languages.len() <= 64, "too many language packs")?;
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
        match self.content.read(&AssetId::Text(id)) {
            Ok(Asset::Text(contract)) => Ok(contract),
            Ok(_) => Err(LocalizationError::Resource("missing text contract".into())),
            Err(error) => Err(presentation_error(error)),
        }
    }
    fn resource(
        &mut self,
        id: TextResourceId,
        locale: &str,
    ) -> localization::Result<Option<LanguageResource>> {
        self.languages
            .get_mut(locale)
            .map_or(Ok(None), |pack| pack.load(id).map_err(presentation_error))
    }
}
impl LoadedProject {
    /// Publish one locale independently. A wording/review edit does not rebuild mechanics.
    pub fn build_language_pack(&self, locale: &str, output: impl AsRef<Path>) -> Result<()> {
        asset::validate_locale(locale)?;
        let resources: Vec<_> = self
            .translations
            .iter()
            .filter(|r| r.locale == locale)
            .collect();
        require(!resources.is_empty(), "locale has no resources")?;
        let output = output.as_ref();
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let stage = parent.join(format!(".language-{}.building", ContentId::new()));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        let result: Result<()> = (|| {
            let mut connection =
                Connection::open_with_flags(&stage, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            connection.execute_batch("PRAGMA synchronous=FULL;")?;
            let tx = connection.transaction()?;
            tx.execute_batch("CREATE TABLE manifest(singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload BLOB NOT NULL CHECK(length(payload)<=65536)) STRICT;
                CREATE TABLE resources(id TEXT PRIMARY KEY, payload BLOB NOT NULL CHECK(length(payload)<=2097152), hash BLOB NOT NULL CHECK(length(hash)=32)) STRICT, WITHOUT ROWID;")?;
            let manifest = Manifest {
                content: self.content.manifest.id,
                locale: locale.into(),
                source_locale: self.source_locale.clone(),
            };
            tx.execute(
                "INSERT INTO manifest VALUES(1,?1)",
                [serde_json::to_vec(&manifest)?],
            )?;
            for resource in resources {
                let payload = serde_json::to_vec(resource)?;
                require(
                    payload.len() <= MAX_ASSET_BYTES,
                    "language resource exceeds 2 MiB publication limit",
                )?;
                tx.execute(
                    "INSERT INTO resources VALUES(?1,?2,?3)",
                    params![
                        resource.id.to_string(),
                        &payload,
                        blake3::hash(&payload).as_bytes()
                    ],
                )?;
            }
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", VERSION)?;
            tx.commit()?;
            connection.close().map_err(|(_, e)| e)?;
            fs::File::open(&stage)?.sync_all()?;
            fs::hard_link(&stage, output)?;
            Ok(())
        })();
        let cleanup = fs::remove_file(&stage);
        result?;
        cleanup?;
        Ok(())
    }
    pub fn materialize_with_languages_for_tools(
        path: impl AsRef<Path>,
        languages: &[PathBuf],
    ) -> Result<Self> {
        let content = Self::materialize_bundle_for_tools(path)?;
        let mut translations = Vec::new();
        require(languages.len() <= 64, "too many language packs")?;
        for path in languages {
            let mut language = LanguageRepository::open(path)?;
            require(
                language.manifest.content == content.content.manifest.id
                    && language.manifest.source_locale == content.source_locale,
                "incompatible language pack",
            )?;
            translations.extend(language.all_for_tools()?);
        }
        Self::from_parts(
            content.content,
            content.scenario,
            content.source_locale,
            translations,
        )
    }
}
