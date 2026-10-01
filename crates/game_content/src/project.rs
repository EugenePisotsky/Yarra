use crate::{Asset, ContentError, Result, Scenario, contextual};
use game_types::*;
use gameplay::actors::ActorTemplate;
use gameplay::dialogue::Dialogue;
use gameplay::inventory::{ItemCatalog, LootTable};
use gameplay::rules::Rules;
use gameplay::{ContentManifest, GameContent, GameDefinitions, KeyedMap, keyed_map};
use localization::{LanguageResource, Localization, contract_hash};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const SOURCE_FORMAT_VERSION: u32 = 11;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TRANSLATION_BYTES: usize = 2 * 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    pub format: u32,
    pub manifest: ContentManifest,
    pub source_locale: String,
    /// Package directories. A package is named after its directory.
    pub packages: Vec<String>,
    pub scenario: String,
}
/// A package's `package.ron`, which it needs only to declare these.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageFile {
    /// Names of the world areas this package refers to.
    #[serde(default)]
    pub areas: BTreeSet<AreaId>,
    /// Variables this package introduces, each with its initial value.
    #[serde(default)]
    pub variables: Vec<gameplay::VariableDefinition>,
}
/// What a file in a package holds, by its name.
enum Kind {
    Package,
    Rules,
    Items,
    Actors,
    Loot,
    Quest,
    Profile,
    Predicate,
    Object,
    Trigger,
    Dialogue,
    Script,
    Text(String),
}
fn kind(path: &str) -> Result<Option<Kind>> {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let named =
        |kind: &str| name == format!("{kind}.ron") || name.ends_with(&format!(".{kind}.ron"));
    Ok(Some(if name == "package.ron" {
        Kind::Package
    } else if name == "rules.ron" {
        Kind::Rules
    } else if named("items") {
        Kind::Items
    } else if named("actors") {
        Kind::Actors
    } else if named("loot") {
        Kind::Loot
    } else if name.ends_with(".quest.ron") {
        Kind::Quest
    } else if name.ends_with(".profile.ron") {
        Kind::Profile
    } else if name.ends_with(".predicate.ron") {
        Kind::Predicate
    } else if name.ends_with(".object.ron") {
        Kind::Object
    } else if name.ends_with(".trigger.ron") {
        Kind::Trigger
    } else if name.ends_with(".dialogue.ron") {
        Kind::Dialogue
    } else if name.ends_with(".luau") {
        Kind::Script
    } else if let Some(locale) = name.strip_suffix(".ftl") {
        crate::asset::validate_locale(locale)?;
        Kind::Text(locale.into())
    } else if name.ends_with(".ron") {
        return Err(Invalid(format!(
            "{path}: no kind of content is named like this; see the package layout in docs/WORKFLOWS.md"
        ))
        .into());
    } else {
        return Ok(None);
    }))
}
/// Eager authoring snapshot. Runtime uses indexed content and language repositories.
pub struct LoadedProject {
    pub(crate) content: GameContent,
    pub(crate) scenario: Scenario,
    pub(crate) source_locale: String,
    pub(crate) translations: Vec<LanguageResource>,
    localization: Localization,
    warnings: Vec<String>,
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
        require(!project.packages.is_empty(), "a project has packages")?;
        crate::asset::validate_locale(&project.source_locale)?;
        let mut items: Option<ItemCatalog> = None;
        let mut rules: Option<Rules> = None;
        let mut actors = BTreeMap::new();
        let mut dialogues = BTreeMap::new();
        let mut world = gameplay::WorldDefinitions::default();
        let mut quests = BTreeMap::new();
        let mut profiles = BTreeMap::new();
        let mut predicates = BTreeMap::new();
        let mut variables = BTreeMap::new();
        let mut scripts = BTreeMap::new();
        let mut loot = BTreeMap::new();
        let mut contracts = Vec::new();
        let mut translations = Vec::new();
        // Which file defined each identity, to name both when one is defined twice.
        let mut defined: BTreeMap<String, String> = BTreeMap::new();
        let mut packages = BTreeSet::new();
        for directory in &project.packages {
            let name = Path::new(directory)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            let text_id = contextual(
                directory.as_str(),
                TextResourceId::try_from(name.to_owned()),
            )?;
            require(packages.insert(text_id), &format!("package {name} twice"))?;
            let mut wording: BTreeMap<String, String> = BTreeMap::new();
            let files = source.files(directory)?;
            // A message written as a bare key is in this package's text.
            in_package(text_id, || -> Result<()> {
                for path in &files {
                    let mut define =
                        |identity: String| match defined.insert(identity.clone(), path.clone()) {
                            Some(first) => Err(Invalid(format!(
                                "{identity} is defined in both {first} and {path}"
                            ))),
                            None => Ok(()),
                        };
                    let Some(kind) = kind(path)? else {
                        continue;
                    };
                    match kind {
                        Kind::Package => {
                            require(
                                Path::new(path).parent() == Some(Path::new(directory)),
                                "package.ron belongs at the top of its package",
                            )?;
                            let package: PackageFile = source.ron(path)?;
                            for area in package.areas {
                                world.areas.insert(area);
                            }
                            for variable in package.variables {
                                define(format!("variable {}", variable.id))?;
                                variables.add(variable);
                            }
                        }
                        Kind::Rules => {
                            define("the rules".into())?;
                            rules = Some(source.ron(path)?);
                        }
                        Kind::Items => {
                            let catalog: ItemCatalog = source.ron(path)?;
                            for id in catalog.categories.keys() {
                                define(format!("item category {id}"))?;
                            }
                            for id in catalog.items.keys() {
                                define(format!("item {id}"))?;
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
                        Kind::Actors => {
                            for template in source.ron::<Vec<ActorTemplate>>(path)? {
                                define(format!("actor template {}", template.id))?;
                                actors.add(template);
                            }
                        }
                        Kind::Loot => {
                            for table in source.ron::<Vec<LootTable>>(path)? {
                                define(format!("loot table {}", table.id))?;
                                loot.add(table);
                            }
                        }
                        Kind::Quest => {
                            let quest: gameplay::quests::Quest = source.ron(path)?;
                            define(format!("quest {}", quest.id))?;
                            quests.add(quest);
                        }
                        Kind::Profile => {
                            let profile: gameplay::InteractionProfile = source.ron(path)?;
                            define(format!("interaction profile {}", profile.id))?;
                            profiles.add(profile);
                        }
                        Kind::Predicate => {
                            let predicate: gameplay::NamedPredicate = source.ron(path)?;
                            define(format!("predicate {}", predicate.id))?;
                            predicates.add(predicate);
                        }
                        Kind::Object => {
                            let object: gameplay::ObjectDefinition = source.ron(path)?;
                            define(format!("object {}", object.id))?;
                            world.objects.add(object);
                        }
                        Kind::Trigger => {
                            let trigger: gameplay::TriggerDefinition = source.ron(path)?;
                            define(format!("trigger {}", trigger.id))?;
                            world.triggers.add(trigger);
                        }
                        Kind::Dialogue => {
                            let graph: Dialogue = source.ron(path)?;
                            define(format!("dialogue {}", graph.id))?;
                            dialogues.add(graph);
                        }
                        Kind::Script => {
                            let stem = Path::new(path).file_stem().and_then(|s| s.to_str());
                            let name =
                                contextual(path.as_str(), Key::new(stem.unwrap_or_default()))?;
                            define(format!("script module {name}"))?;
                            let source = source.text(path, MAX_SCRIPT_BYTES)?;
                            scripts.add(gameplay::ScriptModule { name, source });
                        }
                        Kind::Text(locale) => {
                            let text = source.text(path, MAX_TRANSLATION_BYTES)?;
                            // Several files of one locale are one text, in path order.
                            let joined = wording.entry(locale).or_default();
                            if !joined.is_empty() {
                                joined.push('\n');
                            }
                            joined.push_str(&text);
                        }
                    }
                }
                Ok(())
            })?;
            // The source language says what messages the package has and what they take.
            let Some(source_text) = wording.get(&project.source_locale) else {
                require(
                    wording.is_empty(),
                    &format!(
                        "package {name} has translations but no {}.ftl",
                        project.source_locale
                    ),
                )?;
                continue;
            };
            let contract = contextual(
                format!("{directory}/{}.ftl", project.source_locale),
                localization::contract(text_id, source_text),
            )?;
            let hash = contract_hash(&contract)?;
            for (locale, source) in wording {
                translations.push(LanguageResource {
                    id: text_id,
                    locale,
                    source,
                    contract_hash: hash,
                });
            }
            contracts.push(contract);
        }
        // Kept by identity, so neither the order of packages nor of files in them matters.
        let content = GameContent {
            manifest: project.manifest,
            text: keyed_map(contracts),
            items: items.ok_or_else(|| Invalid("missing item catalog".into()))?,
            game: GameDefinitions {
                world,
                dialogue_contracts: keyed_map(dialogues.values().map(Dialogue::contract)),
                quests,
                profiles,
                predicates,
                rules: rules.ok_or_else(|| Invalid("missing rules".into()))?,
                actors,
                dialogues,
                variables,
                scripts,
                loot,
            },
            scripts: Default::default(),
            graphs: Default::default(),
        };
        let mut scenario: Scenario = source.ron(exercise.unwrap_or(&project.scenario))?;
        if let Some(base) = scenario.base.clone() {
            scenario = contextual(
                format!("scenario base {base}"),
                scenario.starting_from(source.ron(&base)?),
            )?;
        }
        Self::from_parts(content, scenario, project.source_locale, translations)
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
    pub fn start(&self) -> Result<gameplay::GameSession> {
        self.scenario.instantiate(&self.content)
    }
    pub fn run_scenario(&self) -> Result<gameplay::GameSession> {
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
            scripting::LuauScripts::install(content.game.scripts.values()),
        )?;
        contextual("game content", content.validate())?;
        content.validate_selection_links()?;
        crate::asset::validate_locale(&source_locale)?;
        let mut locales = BTreeSet::new();
        for translation in &translations {
            crate::asset::validate_locale(&translation.locale)?;
            locales.insert(translation.locale.clone());
            require(
                content.text.contains_key(&translation.id),
                "translation resource has no contract",
            )?;
        }
        let contracts = content.text.values().cloned().collect();
        let localization = Localization::new(&source_locale, contracts, translations.clone())?;
        let mut warnings = Vec::new();
        for reference in content.text_keys().into_iter().chain(scenario.text_keys()) {
            require(
                content
                    .text
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
            for contract in content.text.values() {
                let context = |locale: &str| format!("Fluent {locale}/{}", contract.id);
                let source = contextual(
                    context(&source_locale),
                    localization.scope(contract.id, &source_locale),
                )?;
                contextual(context(&source_locale), source.validate_definitions(None))?;
                for locale in locales.iter().filter(|l| **l != source_locale) {
                    let scope =
                        contextual(context(locale), localization.scope(contract.id, locale))?;
                    contextual(context(locale), scope.validate_definitions(Some(&source)))?;
                    for key in contract.messages.keys() {
                        if scope.validate(key).is_err() {
                            warnings.push(format!(
                                "{locale}: {}/{} falls back to {source_locale}",
                                contract.id,
                                key.as_str()
                            ));
                        }
                    }
                }
            }
        }
        for asset in content
            .items
            .categories
            .values()
            .cloned()
            .map(Asset::Category)
            .chain(content.items.items.values().cloned().map(Asset::Item))
            .chain(content.game.actors.values().cloned().map(Asset::Actor))
            .chain(std::iter::once(Asset::Rules(content.game.rules.clone())))
            .chain(content.game.quests.values().cloned().map(Asset::Quest))
            .chain(content.game.profiles.values().cloned().map(Asset::Profile))
        {
            for reference in asset.text_references() {
                require(
                    content.text[&reference.resource].messages[&reference.key]
                        .arguments
                        .is_empty(),
                    "static display name/description cannot require arguments",
                )?;
            }
        }
        for reference in scenario.text_keys() {
            require(
                content.text[&reference.resource].messages[&reference.key]
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
        })
    }
}

struct SourceReader {
    root: PathBuf,
}
impl SourceReader {
    fn new(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|error| source_error(root, error))?;
        require(root.is_dir(), "source project must be a directory")?;
        Ok(Self { root })
    }
    fn ron<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        let text = self.text(path, MAX_DOCUMENT_BYTES)?;
        ron::from_str(&text).map_err(|error| source_error(&self.root.join(path), error))
    }
    /// Every file under a directory of the project, by path from the root, in order.
    /// Hidden files are left out.
    fn files(&self, directory: &str) -> Result<Vec<String>> {
        fn walk(root: &Path, at: &Path, into: &mut Vec<String>) -> Result<()> {
            let full = root.join(at);
            let mut entries: Vec<_> = std::fs::read_dir(&full)
                .map_err(|error| source_error(&full, error))?
                .collect::<std::io::Result<_>>()
                .map_err(|error| source_error(&full, error))?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    return Err(source_error(&entry.path(), "file name is not UTF-8"));
                };
                if name.starts_with('.') {
                    continue;
                }
                let path = at.join(name);
                if entry
                    .file_type()
                    .map_err(|e| source_error(&entry.path(), e))?
                    .is_dir()
                {
                    walk(root, &path, into)?;
                } else {
                    into.push(path.to_string_lossy().into_owned());
                }
            }
            Ok(())
        }
        self.check_path(directory)?;
        let mut files = Vec::new();
        walk(&self.root, Path::new(directory), &mut files)?;
        Ok(files)
    }
    /// A path relative to the root that stays inside it.
    fn check_path(&self, path: &str) -> Result<()> {
        let relative = Path::new(path);
        require(
            !path.is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "source paths must be relative without . or ..",
        )
        .map_err(Into::into)
    }
    fn text(&mut self, path: &str, limit: usize) -> Result<String> {
        self.check_path(path)?;
        let relative = Path::new(path);
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
        Ok(text)
    }
}
fn source_error(path: &Path, error: impl std::fmt::Display) -> ContentError {
    ContentError::Source {
        path: path.into(),
        message: error.to_string(),
    }
}
