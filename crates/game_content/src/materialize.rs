//! Explicit eager tool import. Never used by ContentRepository::open/load.
use crate::{asset::*, bundle::publication_hash, *};
use game_types::require;
use gameplay::inventory;
use gameplay::{GameContent, GameDefinitions, KeyedMap};
use std::collections::BTreeMap;
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
            categories: Default::default(),
            items: Default::default(),
        };
        let mut rules = None;
        let mut world = gameplay::WorldDefinitions::default();
        let mut claims = BTreeMap::new();
        let mut dialogue_contracts = BTreeMap::new();
        let mut quests = BTreeMap::new();
        let mut profiles = BTreeMap::new();
        let mut predicates = BTreeMap::new();
        let mut actors = BTreeMap::new();
        let mut dialogues = BTreeMap::new();
        let mut variables = BTreeMap::new();
        let mut scripts = BTreeMap::new();
        let mut loot = BTreeMap::new();
        let mut text = BTreeMap::new();
        for record in records {
            match record {
                Asset::DialogueContract(v) => {
                    dialogue_contracts.add(v);
                }
                Asset::Object(v) => {
                    world.objects.add(v);
                }
                Asset::Area(v) => {
                    world.areas.insert(v);
                }
                Asset::Trigger(v) => {
                    world.triggers.add(v);
                }
                Asset::Claim(v) => {
                    claims.add(v);
                }
                Asset::Quest(v) => {
                    quests.add(v);
                }
                Asset::Profile(v) => {
                    profiles.add(v);
                }
                Asset::Predicate(v) => {
                    predicates.add(v);
                }
                Asset::Category(v) => {
                    items.categories.add(v);
                }
                Asset::Item(v) => {
                    items.items.add(v);
                }
                Asset::Actor(v) => {
                    actors.add(v);
                }
                Asset::Rules(v) => rules = Some(v),
                Asset::Dialogue(v) => {
                    dialogues.add(v);
                }
                Asset::Script(v) => {
                    scripts.add(v);
                }
                Asset::Loot(v) => {
                    loot.add(v);
                }
                Asset::Variable(v) => {
                    variables.add(v);
                }
                Asset::Text(v) => {
                    text.add(v);
                }
            }
        }
        let content = GameContent {
            text,
            manifest: manifest.content,
            items,
            scripts: scripting::LuauScripts::install(scripts.values())
                .map_err(game_types::Invalid)?,
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
        require(
            content.fingerprint()? == manifest.mechanical_fingerprint,
            "mechanical fingerprint mismatch",
        )?;
        Self::from_parts(content, scenario, manifest.source_locale, Vec::new())
    }
}
