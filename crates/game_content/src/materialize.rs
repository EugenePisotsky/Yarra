//! Explicit eager tool import. Never used by ContentRepository::open/load.
use crate::{asset::*, bundle::publication_hash, *};
use game_types::require;
use gameplay::inventory;
use gameplay::{GameContent, GameDefinitions};
use std::path::Path;

impl LoadedProject {
    /// Full validation/demo tool only: reads every mechanical asset and executes the scenario.
    /// Runtime clients use ContentRepository instead. No old bundle schemas are accepted.
    pub fn materialize_bundle_for_tools(path: impl AsRef<Path>) -> Result<Self> {
        let repository = ContentRepository::open(path)?;
        let manifest = repository.manifest().clone();
        let mut records = Vec::new();
        let mut headers = Vec::new();
        for kind in AssetKind::ALL {
            headers.extend(repository.headers(kind)?);
            records.extend(repository.read_kind(kind)?);
        }
        require(
            headers.len() == records.len(),
            "publication asset count mismatch",
        )?;
        require(
            publication_hash(&manifest, &headers)? == manifest.publication_hash,
            "publication fingerprint mismatch",
        )?;
        let scenario: Vec<u8> = repository.connection.query_row(
            "SELECT CASE WHEN length(payload)<=16777216 THEN payload END FROM tool_scenario WHERE singleton=1", [], |r| r.get(0))?;
        require(
            blake3::hash(&scenario).as_bytes() == &manifest.scenario_hash,
            "scenario checksum mismatch",
        )?;
        let scenario = serde_json::from_slice(&scenario)?;
        let mut items = inventory::ItemCatalog {
            id: manifest.catalog_id,
            revision: manifest.catalog_revision,
            categories: vec![],
            items: vec![],
        };
        let mut rules = None;
        let mut world = gameplay::WorldDefinitions::default();
        let mut claims = Vec::new();
        let mut dialogue_contracts = Vec::new();
        let mut quests = Vec::new();
        let mut profiles = Vec::new();
        let mut predicates = Vec::new();
        let mut actors = Vec::new();
        let mut dialogues = Vec::new();
        let mut variables = Vec::new();
        let mut scripts = Vec::new();
        let mut loot = Vec::new();
        let mut text = Vec::new();
        for record in records {
            match record {
                Asset::DialogueContract(v) => dialogue_contracts.push(v),
                Asset::Object(v) => world.objects.push(v),
                Asset::Area(v) => {
                    world.areas.insert(v);
                }
                Asset::Trigger(v) => world.triggers.push(v),
                Asset::Claim(v) => claims.push(v),
                Asset::Quest(v) => quests.push(v),
                Asset::Profile(v) => profiles.push(v),
                Asset::Predicate(v) => predicates.push(v),
                Asset::Category(v) => items.categories.push(v),
                Asset::Item(v) => items.items.push(v),
                Asset::Actor(v) => actors.push(v),
                Asset::Rules(v) => rules = Some(v),
                Asset::Dialogue(v) => dialogues.push(v),
                Asset::Script(v) => scripts.push(v),
                Asset::Loot(v) => loot.push(v),
                Asset::Variable(v) => variables.push(v),
                Asset::Text(v) => text.push(v),
            }
        }
        let mut content = GameContent {
            text,
            manifest: manifest.content,
            items,
            scripts: scripting::LuauScripts::install(&scripts).map_err(game_types::Invalid)?,
            graphs: Default::default(),
            game: GameDefinitions {
                world,
                dialogue_contracts,
                claims,
                quests,
                profiles,
                predicates,
                rules: rules.ok_or(ContentError::MissingAsset(AssetId::Rules))?,
                actors,
                dialogues,
                variables,
                scripts,
                loot,
            },
        };
        content.sort();
        content.game.dialogues.sort_by_key(|v| v.id);
        require(
            content.fingerprint()? == manifest.mechanical_fingerprint,
            "mechanical fingerprint mismatch",
        )?;
        Self::from_parts(content, scenario, manifest.source_locale, Vec::new())
    }
}
