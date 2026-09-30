use crate::{Asset, AssetId, ContentError, Result, Scenario, contextual};
use game_types::*;
use gameplay::actors::ActorTemplate;
use gameplay::dialogue::Dialogue;
use gameplay::inventory::ItemCatalog;
use gameplay::rules::Rules;
use gameplay::{ContentManifest, GameContent, GameDefinitions};
use gameplay::{dialogue, quests};
use localization::{LanguageResource, Localization, contract_hash};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const SOURCE_FORMAT_VERSION: u32 = 8;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_PROJECT_BYTES: usize = 64 * 1024 * 1024;
const MAX_TRANSLATION_BYTES: usize = 2 * 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    pub format: u32,
    pub manifest: ContentManifest,
    pub source_locale: String,
    pub packages: Vec<String>,
    pub scenario: String,
    /// Locales that must be complete and reviewed at publication.
    pub shipping_locales: BTreeSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageFile {
    pub objects: Vec<String>,
    pub areas: Vec<String>,
    pub triggers: Vec<String>,
    pub claims: Vec<String>,
    pub quests: Vec<String>,
    pub profiles: Vec<String>,
    pub predicates: Vec<String>,
    pub id: PackageId,
    pub dependencies: BTreeSet<PackageId>,
    pub catalogs: Vec<String>,
    pub rules: Option<String>,
    pub actors: Vec<String>,
    pub conversations: Vec<String>,
    pub resources: Vec<String>,
    /// Luau files; each is a module named after its file.
    #[serde(default)]
    pub scripts: Vec<String>,
    /// Variables this package introduces, each with its initial value.
    #[serde(default)]
    pub variables: Vec<gameplay::VariableDefinition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationFile {
    pub graph: String,
    pub resources: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceFile {
    pub contract: TextContract,
    pub locales: Vec<LocaleFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocaleFile {
    pub locale: String,
    pub path: String,
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub reviewed: BTreeMap<TextKey, String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationReview {
    pub resource: TextResourceId,
    pub key: TextKey,
    pub locale: String,
    pub source_revision: String,
    pub complete: bool,
    pub reviewed: bool,
}
/// Eager authoring snapshot. Runtime uses indexed content and language repositories.
pub struct LoadedProject {
    pub(crate) content: GameContent,
    pub(crate) scenario: Scenario,
    pub(crate) source_locale: String,
    pub(crate) translations: Vec<LanguageResource>,
    localization: Localization,
    warnings: Vec<String>,
    reviews: Vec<TranslationReview>,
}
impl LoadedProject {
    pub fn load_directory(root: impl AsRef<Path>) -> Result<Self> {
        Self::load_project(root.as_ref(), None)
    }
    /// Tool-only alternative exercise; the path follows the same project-root bounds as assets.
    pub fn load_directory_with_scenario(root: impl AsRef<Path>, scenario: &str) -> Result<Self> {
        Self::load_project(root.as_ref(), Some(scenario))
    }
    fn load_project(root: &Path, exercise: Option<&str>) -> Result<Self> {
        let mut source = SourceReader::new(root)?;
        #[derive(Deserialize)]
        struct Header {
            format: u32,
        }
        let text = source.text("project.ron", MAX_DOCUMENT_BYTES)?;
        let header: Header =
            ron::from_str(&text).map_err(|e| source_error(&root.join("project.ron"), e))?;
        if header.format != SOURCE_FORMAT_VERSION {
            return Err(ContentError::SourceVersion(header.format));
        }
        let project: ProjectFile =
            ron::from_str(&text).map_err(|e| source_error(&root.join("project.ron"), e))?;
        require(
            !project.packages.is_empty() && project.packages.len() <= 256,
            "project requires 1..256 packages",
        )?;
        let mut packages = BTreeMap::new();
        let mut items: Option<ItemCatalog> = None;
        let mut rules: Option<Rules> = None;
        let mut actors: Vec<ActorTemplate> = Vec::new();
        let mut dialogues: Vec<Dialogue> = Vec::new();
        let mut world = gameplay::WorldDefinitions::default();
        let mut claims = Vec::new();
        let mut quests = Vec::new();
        let mut profiles = Vec::new();
        let mut predicates = Vec::new();
        let mut variables = Vec::new();
        let mut scripts = Vec::new();
        let mut contracts = Vec::new();
        let mut translations = Vec::new();
        let mut resource_owners = BTreeMap::new();
        let mut owners = BTreeMap::new();
        let mut paths = BTreeSet::new();
        for path in &project.packages {
            let package: PackageFile = source.ron(path)?;
            macro_rules! own {
                ($id:expr) => {
                    require(
                        owners.insert($id, package.id).is_none(),
                        "duplicate asset identity",
                    )?
                };
            }
            for path in &package.objects {
                let d: gameplay::ObjectDefinition = source.ron(path)?;
                own!(AssetId::Object(d.id));
                world.objects.push(d);
            }
            for path in &package.areas {
                let d: gameplay::AreaDefinition = source.ron(path)?;
                own!(AssetId::Area(d.id));
                world.areas.push(d);
            }
            for path in &package.triggers {
                let d: gameplay::TriggerDefinition = source.ron(path)?;
                own!(AssetId::Trigger(d.id));
                world.triggers.push(d);
            }
            for path in &package.claims {
                let c: dialogue::ClaimDefinition = source.ron(path)?;
                own!(AssetId::Claim(c.id));
                claims.push(c);
            }
            for path in &package.quests {
                let q: quests::Quest = source.ron(path)?;
                own!(AssetId::Quest(q.id));
                quests.push(q);
            }
            for path in &package.profiles {
                let p: gameplay::InteractionProfile = source.ron(path)?;
                own!(AssetId::Profile(p.id));
                profiles.push(p);
            }
            for path in &package.predicates {
                let p: gameplay::NamedPredicate = source.ron(path)?;
                own!(AssetId::Predicate(p.id));
                predicates.push(p);
            }
            let mut resource_paths = package.resources.clone();
            for path in &package.catalogs {
                let catalog: ItemCatalog = source.ron(path)?;
                for v in &catalog.categories {
                    own!(AssetId::Category(v.id));
                }
                for v in &catalog.items {
                    own!(AssetId::Item(v.id));
                }
                if let Some(items) = &mut items {
                    require(
                        items.id == catalog.id && items.revision == catalog.revision,
                        "catalog fragments have different identities",
                    )?;
                    items.categories.extend(catalog.categories);
                    items.items.extend(catalog.items);
                } else {
                    items = Some(catalog);
                }
            }
            if let Some(path) = &package.rules {
                own!(AssetId::Rules);
                require(rules.is_none(), "multiple rules definitions")?;
                rules = Some(source.ron(path)?);
            }
            for path in &package.actors {
                let templates = source.ron::<Vec<ActorTemplate>>(path)?;
                for v in &templates {
                    own!(AssetId::Actor(v.id));
                }
                actors.extend(templates);
            }
            for path in &package.conversations {
                let conversation: ConversationFile = source.ron(path)?;
                let graph: Dialogue = source.ron(&conversation.graph)?;
                own!(AssetId::Dialogue(graph.id));
                own!(AssetId::DialogueContract(graph.id));
                dialogues.push(graph);
                resource_paths.extend(conversation.resources);
            }
            for path in &package.scripts {
                let name = Path::new(path)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .ok_or_else(|| Invalid(format!("script path {path} has no name")))?;
                let name = contextual(path.as_str(), Key::new(name))?;
                own!(AssetId::Script(name.clone()));
                scripts.push(gameplay::ScriptModule {
                    name,
                    source: source.text(path, MAX_SCRIPT_BYTES)?,
                });
            }
            for variable in &package.variables {
                own!(AssetId::Variable(variable.id));
                variables.push(variable.clone());
            }
            require(resource_paths.len() <= 2048, "too many package resources")?;
            for path in resource_paths {
                let resource: ResourceFile = source.ron(&path)?;
                let id = resource.contract.id;
                own!(AssetId::Text(id));
                require(
                    resource_owners.insert(id, package.id).is_none(),
                    "duplicate text resource identity",
                )?;
                require(
                    !resource.locales.is_empty() && resource.locales.len() <= 64,
                    "resource requires 1..64 locales",
                )?;
                let hash = contract_hash(&resource.contract)?;
                for entry in resource.locales {
                    require(
                        paths.insert(entry.path.clone()),
                        "duplicate locale resource path",
                    )?;
                    require(
                        Path::new(&entry.path)
                            .extension()
                            .is_some_and(|e| e == "ftl"),
                        "translation must be an .ftl file",
                    )?;
                    translations.push(LanguageResource {
                        id,
                        locale: entry.locale,
                        source: source.text(&entry.path, MAX_TRANSLATION_BYTES)?,
                        contract_hash: hash,
                        reviewed: entry.reviewed,
                    });
                }
                contracts.push(resource.contract);
            }
            require(
                packages.insert(package.id, package).is_none(),
                "duplicate package identity",
            )?;
        }
        fn dependencies(
            id: PackageId,
            packages: &BTreeMap<PackageId, PackageFile>,
            active: &mut BTreeSet<PackageId>,
            seen: &mut BTreeSet<PackageId>,
        ) -> Result<()> {
            require(!active.contains(&id), "package dependency cycle")?;
            if !seen.insert(id) {
                return Ok(());
            }
            active.insert(id);
            let package = packages
                .get(&id)
                .ok_or_else(|| Invalid("missing package dependency".into()))?;
            for dependency in &package.dependencies {
                dependencies(*dependency, packages, active, seen)?;
            }
            active.remove(&id);
            Ok(())
        }
        for id in packages.keys() {
            let mut reachable = BTreeSet::new();
            dependencies(*id, &packages, &mut BTreeSet::new(), &mut reachable)?;
            for contract in contracts.iter().filter(|c| resource_owners[&c.id] == *id) {
                for import in &contract.imports {
                    let owner = resource_owners
                        .get(import)
                        .ok_or_else(|| Invalid("missing text import".into()))?;
                    require(
                        reachable.contains(owner),
                        "text import needs an explicit package dependency",
                    )?;
                }
            }
        }
        let mut content = GameContent {
            manifest: project.manifest,
            text: contracts,
            items: items.ok_or_else(|| Invalid("missing item catalog".into()))?,
            game: GameDefinitions {
                world,
                dialogue_contracts: dialogues.iter().map(Dialogue::contract).collect(),
                claims,
                quests,
                profiles,
                predicates,
                rules: rules.ok_or_else(|| Invalid("missing rules".into()))?,
                actors,
                dialogues,
                variables,
                scripts,
            },
            scripts: Default::default(),
        };
        // Canonical identities, not manifest/file traversal order, determine publication hashes.
        content.game.world.objects.sort_by_key(|v| v.id);
        content.game.world.areas.sort_by_key(|v| v.id);
        content.game.world.triggers.sort_by_key(|v| v.id);
        content.text.sort_by_key(|v| v.id);
        content.items.categories.sort_by_key(|v| v.id);
        content.items.items.sort_by_key(|v| v.id);
        content.game.claims.sort_by_key(|v| v.id);
        content.game.dialogue_contracts.sort_by_key(|v| v.id);
        content.game.quests.sort_by_key(|v| v.id);
        content.game.profiles.sort_by_key(|v| v.id);
        content.game.predicates.sort_by_key(|v| v.id);
        content.game.actors.sort_by_key(|v| v.id);
        content.game.dialogues.sort_by_key(|v| v.id);
        for contract in &content.text {
            require(
                translations
                    .iter()
                    .any(|t| t.id == contract.id && t.locale == project.source_locale),
                "text resource has no source-locale counterpart",
            )?;
        }
        let scenario = source.ron(exercise.unwrap_or(&project.scenario))?;
        let loaded = Self::from_parts(content, scenario, project.source_locale, translations)?;
        for (_, asset) in loaded.assets() {
            let owner = owners[&asset.id()];
            let mut reachable = BTreeSet::new();
            dependencies(owner, &packages, &mut BTreeSet::new(), &mut reachable)?;
            for dependency in asset.references()? {
                let target = owners
                    .get(&dependency)
                    .ok_or_else(|| Invalid(format!("missing package asset {dependency:?}")))?;
                require(
                    reachable.contains(target),
                    &format!(
                        "asset {:?} needs a package dependency for {dependency:?}",
                        asset.id()
                    ),
                )?;
            }
        }
        for locale in project.shipping_locales {
            crate::asset::validate_locale(&locale)?;
            require(
                locale == loaded.source_locale || loaded.reviews.iter().any(|r| r.locale == locale),
                "shipping locale has no translations",
            )?;
            require(
                loaded
                    .reviews
                    .iter()
                    .filter(|r| r.locale == locale)
                    .all(|r| r.complete && r.reviewed),
                "shipping locale has missing or stale translations",
            )?;
        }
        Ok(loaded)
    }
    pub fn content(&self) -> &GameContent {
        &self.content
    }
    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }
    pub fn localization(&self) -> &Localization {
        &self.localization
    }
    pub fn source_locale(&self) -> &str {
        &self.source_locale
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn translation_reviews(&self) -> &[TranslationReview] {
        &self.reviews
    }
    pub fn start(&self) -> Result<crate::ToolSession> {
        self.scenario.instantiate(&self.content)
    }
    pub fn run_scenario(&self) -> Result<crate::ToolSession> {
        self.scenario.run(&self.content)
    }
    pub(crate) fn from_parts(
        mut content: GameContent,
        scenario: Scenario,
        source_locale: String,
        translations: Vec<LanguageResource>,
    ) -> Result<Self> {
        content.scripts = contextual(
            "scripts",
            scripting::LuauScripts::install(&content.game.scripts),
        )?;
        contextual("game content", content.validate())?;
        content.validate_selection_links()?;
        crate::asset::validate_locale(&source_locale)?;
        require(
            serde_json::to_vec(&content)?.len() <= 2 * MAX_DOCUMENT_BYTES,
            "content exceeds 32 MiB",
        )?;
        require(
            serde_json::to_vec(&scenario)?.len() <= MAX_DOCUMENT_BYTES,
            "scenario exceeds 16 MiB",
        )?;
        require(translations.len() <= 2048, "too many translation resources")?;
        let mut locales = BTreeSet::new();
        let mut bytes = 0;
        for translation in &translations {
            crate::asset::validate_locale(&translation.locale)?;
            locales.insert(translation.locale.clone());
            require(
                translation.source.len() <= MAX_TRANSLATION_BYTES,
                "translation exceeds 2 MiB",
            )?;
            bytes += translation.source.len();
            require(bytes <= MAX_DOCUMENT_BYTES, "translations exceed 16 MiB")?;
            let contract = content
                .text
                .iter()
                .find(|c| c.id == translation.id)
                .ok_or_else(|| Invalid("translation resource has no contract".into()))?;
            for (key, revision) in &translation.reviewed {
                require(
                    contract.messages.contains_key(key),
                    "review metadata names an undeclared message",
                )?;
                require(
                    revision.len() == 64
                        && revision
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "review revision must be a lowercase BLAKE3 hash",
                )?;
            }
        }
        require(locales.len() <= 64, "too many locales")?;
        let localization =
            Localization::new(&source_locale, content.text.clone(), translations.clone())?;
        let mut warnings = Vec::new();
        let mut reviews = Vec::new();
        let contracts: BTreeMap<_, _> = content.text.iter().map(|c| (c.id, c)).collect();
        for reference in content.text_keys().into_iter().chain(scenario.text_keys()) {
            require(
                contracts
                    .get(&reference.resource)
                    .is_some_and(|c| c.messages.contains_key(&reference.key)),
                &format!(
                    "undeclared message {}/{}",
                    reference.resource,
                    reference.key.as_str()
                ),
            )?;
        }
        // Mechanical-only materialization is useful for validation/automation without packs.
        if !translations.is_empty() {
            localization
                .validate_keys(content.text_keys().into_iter().chain(scenario.text_keys()))?;
            for contract in &content.text {
                let source = contextual(
                    format!("Fluent {}/{}", source_locale, contract.id),
                    localization.scope(contract.id, &source_locale),
                )?;
                contextual(
                    format!("Fluent {}/{}", source_locale, contract.id),
                    source.validate_definitions(None),
                )?;
                let revisions: BTreeMap<_, _> = contract
                    .messages
                    .keys()
                    .map(|key| Ok((key.clone(), source.revision(key)?)))
                    .collect::<localization::Result<_>>()?;
                for locale in locales.iter().filter(|l| **l != source_locale) {
                    let scope = contextual(
                        format!("Fluent {}/{}", locale, contract.id),
                        localization.scope(contract.id, locale),
                    )?;
                    contextual(
                        format!("Fluent {}/{}", locale, contract.id),
                        scope.validate_definitions(Some(&source)),
                    )?;
                    for (key, revision) in &revisions {
                        let complete = match scope.validate(key) {
                            Ok(()) => true,
                            Err(localization::LocalizationError::Missing(_)) => false,
                            Err(e) => return Err(e.into()),
                        };
                        let reviewed = translations
                            .iter()
                            .find(|t| t.id == contract.id && t.locale == *locale)
                            .and_then(|t| t.reviewed.get(key))
                            .is_some_and(|r| r == revision);
                        if !complete {
                            warnings.push(format!(
                                "{locale}: {}/{} falls back to {source_locale}",
                                contract.id,
                                key.as_str()
                            ));
                        } else if !reviewed {
                            warnings.push(format!("{locale}: {}/{} needs translation review; source revision {revision}", contract.id, key.as_str()));
                        }
                        reviews.push(TranslationReview {
                            resource: contract.id,
                            key: key.clone(),
                            locale: locale.clone(),
                            source_revision: revision.clone(),
                            complete,
                            reviewed,
                        });
                    }
                }
            }
        }
        for asset in content
            .items
            .categories
            .iter()
            .cloned()
            .map(Asset::Category)
            .chain(content.items.items.iter().cloned().map(Asset::Item))
            .chain(content.game.actors.iter().cloned().map(Asset::Actor))
            .chain(std::iter::once(Asset::Rules(content.game.rules.clone())))
            .chain(content.game.quests.iter().cloned().map(Asset::Quest))
            .chain(content.game.profiles.iter().cloned().map(Asset::Profile))
        {
            for reference in asset.text_references() {
                require(
                    contracts[&reference.resource].messages[&reference.key]
                        .arguments
                        .is_empty(),
                    "static display name/description cannot require arguments",
                )?;
            }
        }
        for reference in scenario.text_keys() {
            require(
                contracts[&reference.resource].messages[&reference.key]
                    .arguments
                    .is_empty(),
                "starting name cannot require arguments",
            )?;
        }
        contextual("scenario", scenario.run(&content))?;
        Ok(Self {
            content,
            scenario,
            source_locale,
            translations,
            localization,
            warnings,
            reviews,
        })
    }
}

struct SourceReader {
    root: PathBuf,
    bytes: usize,
}
impl SourceReader {
    fn new(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|error| source_error(root, error))?;
        require(root.is_dir(), "source project must be a directory")?;
        Ok(Self { root, bytes: 0 })
    }
    fn ron<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        let text = self.text(path, MAX_DOCUMENT_BYTES)?;
        ron::from_str(&text).map_err(|error| source_error(&self.root.join(path), error))
    }
    fn text(&mut self, path: &str, limit: usize) -> Result<String> {
        let relative = Path::new(path);
        require(
            !path.is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "source paths must be relative without . or ..",
        )?;
        let full = self.root.join(relative);
        let canonical = full
            .canonicalize()
            .map_err(|error| source_error(&full, error))?;
        require(
            canonical.starts_with(&self.root),
            "source symlink escapes project directory",
        )?;
        require(canonical.is_file(), "source path must be a regular file")?;
        let mut text = String::new();
        File::open(&canonical)
            .and_then(|file| file.take(limit as u64 + 1).read_to_string(&mut text))
            .map_err(|error| source_error(&full, error))?;
        if text.len() > limit {
            return Err(source_error(&full, format!("file exceeds {limit} bytes")));
        }
        self.bytes += text.len();
        require(
            self.bytes <= MAX_PROJECT_BYTES,
            "source project exceeds 64 MiB",
        )?;
        Ok(text)
    }
}
fn source_error(path: &Path, error: impl std::fmt::Display) -> ContentError {
    ContentError::Source {
        path: path.into(),
        message: error.to_string(),
    }
}
