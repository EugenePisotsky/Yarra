//! Scoped, bounded Fluent presentation. Domain contracts contain no Fluent/runtime types.
mod analysis;
use fluent_bundle::{FluentArgs, FluentResource, concurrent::FluentBundle};
use game_types::{
    ArgumentType, MessageContract, MessageRef, TextContract, TextKey, TextRef, TextResourceId,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};
use thiserror::Error;
use unic_langid::LanguageIdentifier;

#[derive(Debug, Error)]
pub enum LocalizationError {
    #[error("invalid locale: {0}")]
    Locale(String),
    #[error("invalid Fluent resources: {0}")]
    Resource(String),
    #[error("missing message or dependency: {0}")]
    Missing(String),
    #[error("Fluent formatting failed: {0}")]
    Format(String),
    #[error("localization budget exceeded: {0}")]
    Budget(String),
}
pub type Result<T> = std::result::Result<T, LocalizationError>;
pub(crate) fn resource_error(value: impl Into<String>) -> LocalizationError {
    LocalizationError::Resource(value.into())
}
#[derive(Debug, Clone, PartialEq)]
pub enum Argument {
    Text(String),
    Number(i32),
}
pub type Arguments = BTreeMap<String, Argument>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizedText {
    pub value: String,
    pub locale: Option<String>,
    pub used_fallback: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageResource {
    pub id: TextResourceId,
    pub locale: String,
    pub source: String,
    /// Hash of the mechanical contract this wording was validated against.
    pub contract_hash: [u8; 32],
    /// Per-message source dependency revision accepted by the translator.
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub reviewed: BTreeMap<TextKey, String>,
}
pub fn contract_hash(contract: &TextContract) -> Result<[u8; 32]> {
    Ok(
        *blake3::hash(&serde_json::to_vec(contract).map_err(|e| resource_error(e.to_string()))?)
            .as_bytes(),
    )
}
pub trait ResourceSource {
    fn contract(&mut self, id: TextResourceId) -> Result<TextContract>;
    fn resource(&mut self, id: TextResourceId, locale: &str) -> Result<Option<LanguageResource>>;
}
pub struct MemorySource {
    contracts: BTreeMap<TextResourceId, TextContract>,
    resources: BTreeMap<(TextResourceId, String), LanguageResource>,
}
impl MemorySource {
    pub fn new(contracts: Vec<TextContract>, resources: Vec<LanguageResource>) -> Result<Self> {
        let mut source = Self {
            contracts: BTreeMap::new(),
            resources: BTreeMap::new(),
        };
        for contract in contracts {
            contract
                .validate()
                .map_err(|e| resource_error(e.to_string()))?;
            if source.contracts.insert(contract.id, contract).is_some() {
                return Err(resource_error("duplicate text contract"));
            }
        }
        for resource in resources {
            if source
                .resources
                .insert((resource.id, resource.locale.clone()), resource)
                .is_some()
            {
                return Err(resource_error("duplicate locale resource identity"));
            }
        }
        Ok(source)
    }
}
impl ResourceSource for MemorySource {
    fn contract(&mut self, id: TextResourceId) -> Result<TextContract> {
        self.contracts
            .get(&id)
            .cloned()
            .ok_or_else(|| resource_error(format!("unknown text resource {id}")))
    }
    fn resource(&mut self, id: TextResourceId, locale: &str) -> Result<Option<LanguageResource>> {
        Ok(self.resources.get(&(id, locale.into())).cloned())
    }
}
#[derive(Debug, Clone)]
pub struct LocalizationLimits {
    pub scopes: usize,
    pub charge_bytes: usize,
    pub resources_per_scope: usize,
}
impl Default for LocalizationLimits {
    fn default() -> Self {
        Self {
            scopes: 32,
            charge_bytes: 32 * 1024 * 1024,
            resources_per_scope: 32,
        }
    }
}
struct Scope {
    bundle: FluentBundle<FluentResource>,
    patterns: analysis::Patterns,
    messages: BTreeMap<TextKey, MessageContract>,
    all_messages: BTreeMap<TextKey, MessageContract>,
    charge: usize,
}
struct CacheEntry {
    id: TextResourceId,
    locale: String,
    scope: Arc<Scope>,
}
/// The service owns one immutable generation of providers. Replace it to activate new packs.
/// A scope handle pins parsed data; cache pressure never invalidates a live handle.
pub struct Localization {
    source_locale: String,
    provider: RefCell<Box<dyn ResourceSource>>,
    limits: LocalizationLimits,
    cache: RefCell<VecDeque<CacheEntry>>,
}
pub struct TextScope {
    scope: Arc<Scope>,
    locale: String,
}
fn locale(value: &str) -> Result<LanguageIdentifier> {
    value
        .parse()
        .map_err(|_| LocalizationError::Locale(value.into()))
}
impl Localization {
    /// Eager input ownership is for authoring tools. Parsing remains scoped and bounded.
    pub fn new(
        source: &str,
        contracts: Vec<TextContract>,
        resources: Vec<LanguageResource>,
    ) -> Result<Self> {
        Self::with_source(
            source,
            Box::new(MemorySource::new(contracts, resources)?),
            LocalizationLimits {
                charge_bytes: 64 * 1024 * 1024,
                ..Default::default()
            },
        )
    }
    pub fn with_source(
        source: &str,
        provider: Box<dyn ResourceSource>,
        limits: LocalizationLimits,
    ) -> Result<Self> {
        if limits.scopes == 0
            || limits.scopes > 256
            || limits.charge_bytes == 0
            || limits.charge_bytes > 256 * 1024 * 1024
            || limits.resources_per_scope == 0
            || limits.resources_per_scope > 256
        {
            return Err(LocalizationError::Budget("invalid limits".into()));
        }
        Ok(Self {
            source_locale: locale(source)?.to_string(),
            provider: RefCell::new(provider),
            limits,
            cache: RefCell::new(VecDeque::new()),
        })
    }
    pub fn cached_scopes(&self) -> usize {
        self.cache.borrow().len()
    }
    pub fn cached_charge(&self) -> usize {
        self.cache.borrow().iter().map(|e| e.scope.charge).sum()
    }
    pub fn scope(&self, resource: TextResourceId, language: &str) -> Result<TextScope> {
        let language = locale(language)?.to_string();
        let mut cache = self.cache.borrow_mut();
        if let Some(index) = cache
            .iter()
            .position(|e| e.id == resource && e.locale == language)
        {
            let entry = cache.remove(index).unwrap();
            let result = TextScope {
                scope: entry.scope.clone(),
                locale: language,
            };
            cache.push_back(entry);
            return Ok(result);
        }
        let scope = Arc::new(self.prepare(resource, &language)?);
        while cache.len() >= self.limits.scopes
            || cache.iter().map(|e| e.scope.charge).sum::<usize>() + scope.charge
                > self.limits.charge_bytes
        {
            let Some(index) = cache.iter().position(|e| Arc::strong_count(&e.scope) == 1) else {
                return Err(LocalizationError::Budget(
                    "parsed scope cache is pinned or full".into(),
                ));
            };
            cache.remove(index);
        }
        cache.push_back(CacheEntry {
            id: resource,
            locale: language.clone(),
            scope: scope.clone(),
        });
        Ok(TextScope {
            scope,
            locale: language,
        })
    }
    fn prepare(&self, id: TextResourceId, language: &str) -> Result<Scope> {
        fn visit(
            provider: &mut dyn ResourceSource,
            id: TextResourceId,
            active: &mut BTreeSet<TextResourceId>,
            contracts: &mut BTreeMap<TextResourceId, TextContract>,
            limit: usize,
        ) -> Result<()> {
            if active.contains(&id) {
                return Err(resource_error("text import cycle"));
            }
            if contracts.contains_key(&id) {
                return Ok(());
            }
            if active.len() + contracts.len() >= limit {
                return Err(LocalizationError::Budget("text import closure".into()));
            }
            active.insert(id);
            let contract = provider.contract(id)?;
            if contract.id != id {
                return Err(resource_error("text contract identity mismatch"));
            }
            contract
                .validate()
                .map_err(|e| resource_error(e.to_string()))?;
            for dependency in &contract.imports {
                visit(provider, *dependency, active, contracts, limit)?;
            }
            active.remove(&id);
            contracts.insert(id, contract);
            Ok(())
        }
        let mut provider = self.provider.borrow_mut();
        let mut contracts = BTreeMap::new();
        visit(
            provider.as_mut(),
            id,
            &mut BTreeSet::new(),
            &mut contracts,
            self.limits.resources_per_scope,
        )?;
        let mut scope = Scope {
            bundle: FluentBundle::new_concurrent(vec![locale(language)?]),
            patterns: BTreeMap::new(),
            messages: contracts[&id].messages.clone(),
            all_messages: BTreeMap::new(),
            charge: 4096,
        };
        let mut message_names = BTreeSet::new();
        for (resource_id, contract) in &contracts {
            for key in contract.messages.keys() {
                if !message_names.insert(key.clone()) {
                    return Err(resource_error(format!(
                        "ambiguous imported message {}",
                        key.as_str()
                    )));
                }
            }
            scope.all_messages.extend(contract.messages.clone());
            scope.charge += serde_json::to_vec(contract)
                .map_err(|e| resource_error(e.to_string()))?
                .len()
                * 16
                + 4096;
            if let Some(resource) = provider.resource(*resource_id, language)? {
                if resource.id != *resource_id
                    || resource.locale != language
                    || resource.contract_hash != contract_hash(contract)?
                {
                    return Err(resource_error(
                        "language resource contract/identity mismatch",
                    ));
                }
                if resource.source.len() > 2 * 1024 * 1024 {
                    return Err(LocalizationError::Budget(
                        "Fluent resource exceeds 2 MiB".into(),
                    ));
                }
                scope.charge += resource.source.len() * 16 + 4096;
                if scope.charge > self.limits.charge_bytes {
                    return Err(LocalizationError::Budget(
                        "parsed scope exceeds cache charge".into(),
                    ));
                }
                let mut local = BTreeMap::new();
                analysis::parse(&resource.source, &mut local)?;
                for key in local.keys().filter(|k| !k.starts_with('-')) {
                    let name = key.split('.').next().unwrap();
                    if !contract.messages.contains_key(
                        &TextKey::new(name).map_err(|e| resource_error(e.to_string()))?,
                    ) {
                        return Err(resource_error(format!("undeclared message {name}")));
                    }
                }
                for (key, value) in local {
                    if scope.patterns.insert(key.clone(), value).is_some() {
                        return Err(resource_error(format!(
                            "ambiguous imported Fluent key {key}"
                        )));
                    }
                }
                let parsed = FluentResource::try_new(resource.source)
                    .map_err(|(_, e)| resource_error(format!("{e:?}")))?;
                scope
                    .bundle
                    .add_resource(parsed)
                    .map_err(|e| resource_error(format!("{e:?}")))?;
            }
        }
        Ok(scope)
    }
    pub fn validate_keys<'a>(&self, keys: impl IntoIterator<Item = &'a MessageRef>) -> Result<()> {
        for key in keys {
            let scope = self.scope(key.resource, &self.source_locale)?;
            scope.contract(&key.key)?;
            scope.revision(&key.key)?;
        }
        Ok(())
    }
    pub fn format_bound(
        &self,
        locale: &str,
        bound: &game_types::BoundText,
    ) -> Result<LocalizedText> {
        let mut args = Arguments::new();
        for (name, value) in &bound.arguments {
            let value = match value {
                game_types::BoundArgument::Number(n) => Argument::Number(*n),
                game_types::BoundArgument::Select(v) => Argument::Text(v.clone()),
                game_types::BoundArgument::Text(t) => {
                    Argument::Text(self.format(locale, t, &Arguments::new())?.value)
                }
            };
            args.insert(name.clone(), value);
        }
        self.format(locale, &bound.text, &args)
    }
    pub fn format(
        &self,
        requested: &str,
        text: &TextRef,
        arguments: &Arguments,
    ) -> Result<LocalizedText> {
        if let TextRef::Literal(value) = text {
            return Ok(LocalizedText {
                value: value.clone(),
                locale: None,
                used_fallback: false,
            });
        }
        let TextRef::Message(message) = text else {
            unreachable!()
        };
        let requested = locale(requested)?;
        let mut candidates = vec![requested.to_string()];
        if let Some(script) = requested.script {
            candidates.push(format!("{}-{script}", requested.language));
        }
        candidates.push(requested.language.to_string());
        candidates.push(self.source_locale.clone());
        let mut seen = BTreeSet::new();
        for candidate in candidates {
            if !seen.insert(candidate.clone()) {
                continue;
            }
            let scope = self.scope(message.resource, &candidate)?;
            match scope.format(&message.key, arguments) {
                Ok(value) => {
                    return Ok(LocalizedText {
                        value,
                        used_fallback: candidate != requested.to_string(),
                        locale: Some(candidate),
                    });
                }
                Err(LocalizationError::Missing(_)) if candidate != self.source_locale => continue,
                Err(e) => return Err(e),
            }
        }
        Err(LocalizationError::Missing(message.key.as_str().into()))
    }
}
impl TextScope {
    /// Source scopes must resolve every reference. Translations may be incomplete but must
    /// only reference declared dependencies; unknown names and cycles are publication errors.
    pub fn validate_definitions(&self, source: Option<&TextScope>) -> Result<()> {
        analysis::validate_graph(&self.scope.patterns, source.map(|s| &s.scope.patterns))?;
        let mut combined;
        let patterns = if let Some(source) = source {
            combined = source.scope.patterns.clone();
            combined.extend(self.scope.patterns.clone());
            &combined
        } else {
            &self.scope.patterns
        };
        for (key, contract) in &self.scope.messages {
            for name in self.scope.patterns.keys().filter(|name| {
                name.as_str() == key.as_str() || name.starts_with(&format!("{}.", key.as_str()))
            }) {
                match analysis::dependencies(
                    patterns,
                    &self.scope.all_messages,
                    name,
                    &contract.arguments,
                ) {
                    Err(LocalizationError::Missing(_)) if source.is_some() => {}
                    result => {
                        result?;
                    }
                }
            }
        }
        Ok(())
    }
    fn contract(&self, key: &TextKey) -> Result<&MessageContract> {
        self.scope
            .messages
            .get(key)
            .ok_or_else(|| resource_error(format!("undeclared message {}", key.as_str())))
    }
    pub fn revision(&self, key: &TextKey) -> Result<String> {
        analysis::revision(
            &self.scope.patterns,
            &self.scope.all_messages,
            key.as_str(),
            &self.contract(key)?.arguments,
        )
    }
    pub fn validate(&self, key: &TextKey) -> Result<()> {
        self.revision(key).map(|_| ())
    }
    pub fn format(&self, key: &TextKey, arguments: &Arguments) -> Result<String> {
        let contract = self.contract(key)?;
        if contract.arguments.len() != arguments.len() {
            return Err(resource_error("message arguments do not match contract"));
        }
        let mut args = FluentArgs::new();
        for (name, kind) in &contract.arguments {
            match (kind, arguments.get(name)) {
                (ArgumentType::Number, Some(Argument::Number(v))) => args.set(name.as_str(), *v),
                (ArgumentType::Text, Some(Argument::Text(v))) => {
                    args.set(name.as_str(), v.as_str())
                }
                (ArgumentType::Select(values), Some(Argument::Text(v))) if values.contains(v) => {
                    args.set(name.as_str(), v.as_str())
                }
                _ => {
                    return Err(resource_error(format!(
                        "argument ${name} does not match contract"
                    )));
                }
            }
        }
        analysis::dependencies(
            &self.scope.patterns,
            &self.scope.all_messages,
            key.as_str(),
            &contract.arguments,
        )?;
        let pattern = self
            .scope
            .bundle
            .get_message(key.as_str())
            .and_then(|m| m.value())
            .ok_or_else(|| LocalizationError::Missing(key.as_str().into()))?;
        let mut errors = Vec::new();
        let value = self
            .scope
            .bundle
            .format_pattern(pattern, Some(&args), &mut errors)
            .into_owned();
        if !errors.is_empty() {
            return Err(LocalizationError::Format(format!(
                "{}/{}: {errors:?}",
                self.locale,
                key.as_str()
            )));
        }
        Ok(value)
    }
}
