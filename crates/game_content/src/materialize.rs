//! Explicit eager tool import. Never used by ContentRepository::open/load.
use crate::{asset::*, bundle::publication_hash, *};
use game_types::{Invalid, require};
use gameplay::inventory;
use gameplay::{GameContent, GameDefinitions};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

impl LoadedProject {
    /// Full validation/demo tool only: reads every mechanical asset and executes the scenario.
    /// Runtime clients use ContentRepository instead. No old bundle schemas are accepted.
    pub fn materialize_bundle_for_tools(path: impl AsRef<Path>) -> Result<Self> {
        let mut repository = ContentRepository::open(
            path,
            RepositoryLimits {
                cache_charge_bytes: 64 * 1024 * 1024,
                ..Default::default()
            },
        )?;
        let manifest = repository.manifest().clone();
        let mut records = Vec::new();
        let mut headers = Vec::new();
        let mut bytes = 0usize;
        for kind in AssetKind::ALL {
            let mut cursor = None;
            loop {
                let page = repository.list(kind, cursor.as_deref(), 128)?;
                if page.is_empty() {
                    break;
                }
                for header in &page {
                    bytes = bytes
                        .checked_add(header.payload_bytes)
                        .ok_or_else(|| Invalid("tool byte budget overflow".into()))?;
                    require(
                        bytes <= 64 * 1024 * 1024 && headers.len() < 100_000,
                        "tool materialization budget exceeded",
                    )?;
                    records.push((
                        header.position,
                        repository.fetch(&header.id)?.as_ref().clone(),
                    ));
                    headers.push(header.clone());
                }
                cursor = page.last().map(|h| h.id.key());
            }
        }
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
        // Restore canonical asset order; meaningful order inside each asset is preserved.
        records.sort_by_key(|(position, asset)| (asset.id().kind(), *position));
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
        let mut facts = BTreeSet::new();
        let mut conditions = BTreeMap::new();
        let mut actions = BTreeMap::new();
        let mut text = Vec::new();
        for (_, record) in records {
            match record {
                Asset::DialogueContract(v) => dialogue_contracts.push(v),
                Asset::Object(v) => world.objects.push(v),
                Asset::Area(v) => world.areas.push(v),
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
                Asset::Condition { id, value } => {
                    conditions.insert(id, value);
                }
                Asset::Action { id, value } => {
                    actions.insert(id, value);
                }
                Asset::Fact(v) => {
                    facts.insert(v);
                }
                Asset::Text(v) => text.push(v),
            }
        }
        let content = GameContent {
            text,
            manifest: manifest.content,
            items,
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
                facts,
                conditions,
                actions,
            },
        };
        require(
            content.fingerprint()? == manifest.mechanical_fingerprint,
            "mechanical fingerprint mismatch",
        )?;
        Self::from_parts(content, scenario, manifest.source_locale, Vec::new())
    }
}
