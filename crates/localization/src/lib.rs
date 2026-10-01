//! Fluent presentation of the content's text. Each package has one text resource. What
//! messages it has, and what arguments they take, comes from its source-language file;
//! translations are checked against that. Gameplay holds only the resulting contracts.
mod analysis;
use fluent_bundle::{FluentArgs, FluentResource, concurrent::FluentBundle};
use game_types::{
    ArgumentType, MessageContract, MessageRef, TextContract, TextKey, TextRef, TextResourceId,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
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
/// One locale's wording of a text resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageResource {
    pub id: TextResourceId,
    pub locale: String,
    pub source: String,
    /// Hash of the contract this wording was checked against.
    pub contract_hash: [u8; 32],
}
pub fn contract_hash(contract: &TextContract) -> Result<[u8; 32]> {
    Ok(
        *blake3::hash(&serde_json::to_vec(contract).map_err(|e| resource_error(e.to_string()))?)
            .as_bytes(),
    )
}
/// The contract of a text resource: every message its source-language Fluent defines, with
/// the arguments it takes as their use there shows.
pub fn contract(id: TextResourceId, source: &str) -> Result<TextContract> {
    analysis::infer(id, source)
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
/// A resource's wording in one locale, parsed once.
struct Scope {
    bundle: FluentBundle<FluentResource>,
    patterns: analysis::Patterns,
    messages: BTreeMap<TextKey, MessageContract>,
    /// Messages this locale can say in full; the rest fall back to another locale.
    complete: BTreeSet<TextKey>,
}
/// Formats the content's text. Each resource is parsed once per locale, the first time it is
/// needed, and kept: a game has a resource per package and a handful of locales.
pub struct Localization {
    source_locale: String,
    provider: RefCell<Box<dyn ResourceSource>>,
    scopes: RefCell<BTreeMap<(TextResourceId, String), Arc<Scope>>>,
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
    /// Over contracts and wording already in memory, as tools have them.
    pub fn new(
        source: &str,
        contracts: Vec<TextContract>,
        resources: Vec<LanguageResource>,
    ) -> Result<Self> {
        Self::with_source(source, Box::new(MemorySource::new(contracts, resources)?))
    }
    pub fn with_source(source: &str, provider: Box<dyn ResourceSource>) -> Result<Self> {
        Ok(Self {
            source_locale: locale(source)?.to_string(),
            provider: RefCell::new(provider),
            scopes: RefCell::default(),
        })
    }
    pub fn scope(&self, resource: TextResourceId, language: &str) -> Result<TextScope> {
        let language = locale(language)?.to_string();
        let key = (resource, language.clone());
        if let Some(scope) = self.scopes.borrow().get(&key) {
            return Ok(TextScope {
                scope: scope.clone(),
                locale: language,
            });
        }
        let scope = Arc::new(self.prepare(resource, &language)?);
        self.scopes.borrow_mut().insert(key, scope.clone());
        Ok(TextScope {
            scope,
            locale: language,
        })
    }
    fn prepare(&self, id: TextResourceId, language: &str) -> Result<Scope> {
        let mut provider = self.provider.borrow_mut();
        let contract = provider.contract(id)?;
        if contract.id != id {
            return Err(resource_error("text contract identity mismatch"));
        }
        contract
            .validate()
            .map_err(|e| resource_error(e.to_string()))?;
        let mut scope = Scope {
            bundle: FluentBundle::new_concurrent(vec![locale(language)?]),
            patterns: BTreeMap::new(),
            messages: contract.messages.clone(),
            complete: BTreeSet::new(),
        };
        let Some(resource) = provider.resource(id, language)? else {
            return Ok(scope);
        };
        if resource.id != id
            || resource.locale != language
            || resource.contract_hash != contract_hash(&contract)?
        {
            return Err(resource_error(
                "language resource contract/identity mismatch",
            ));
        }
        analysis::parse(&resource.source, &mut scope.patterns)?;
        for key in scope.patterns.keys().filter(|k| !k.starts_with('-')) {
            let name = key.split('.').next().unwrap();
            let declared =
                TextKey::new(name).is_ok_and(|name| contract.messages.contains_key(&name));
            if !declared {
                return Err(resource_error(format!(
                    "{name} is not in the source-language text"
                )));
            }
        }
        // Worked out once: whether each message can be said in full in this locale.
        for (key, message) in &contract.messages {
            let walk = analysis::dependencies(
                &scope.patterns,
                &contract.messages,
                key.as_str(),
                &message.arguments,
            );
            match walk {
                Ok(_) => {
                    scope.complete.insert(key.clone());
                }
                Err(LocalizationError::Missing(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let parsed = FluentResource::try_new(resource.source)
            .map_err(|(_, e)| resource_error(format!("{e:?}")))?;
        scope
            .bundle
            .add_resource(parsed)
            .map_err(|e| resource_error(format!("{e:?}")))?;
        Ok(scope)
    }
    /// Every message exists in the source language and can be said in full there.
    pub fn validate_keys<'a>(&self, keys: impl IntoIterator<Item = &'a MessageRef>) -> Result<()> {
        for key in keys {
            self.scope(key.resource, &self.source_locale)?
                .validate(&key.key)?;
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
    /// The text in the requested locale, or the nearest one that has it, the source
    /// language last.
    pub fn format(
        &self,
        requested: &str,
        text: &TextRef,
        arguments: &Arguments,
    ) -> Result<LocalizedText> {
        let message = match text {
            TextRef::Literal(value) => {
                return Ok(LocalizedText {
                    value: value.clone(),
                    locale: None,
                    used_fallback: false,
                });
            }
            TextRef::Message(message) => message,
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
    /// Every reference resolves and every branch fits the contract. A translation may leave
    /// messages out, or lean on terms only the source language defines; those messages fall
    /// back whole.
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
                    &self.scope.messages,
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
    /// The message can be said in full in this locale.
    pub fn validate(&self, key: &TextKey) -> Result<()> {
        self.contract(key)?;
        if self.scope.complete.contains(key) {
            Ok(())
        } else {
            Err(LocalizationError::Missing(key.as_str().into()))
        }
    }
    pub fn format(&self, key: &TextKey, arguments: &Arguments) -> Result<String> {
        let contract = self.contract(key)?;
        if contract.arguments.len() != arguments.len() {
            return Err(resource_error("message arguments do not match contract"));
        }
        let mut args = FluentArgs::new();
        for (name, kind) in &contract.arguments {
            match (kind, arguments.get(name)) {
                (ArgumentType::Number | ArgumentType::Text, Some(Argument::Number(v))) => {
                    args.set(name.as_str(), *v)
                }
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
        if !self.scope.complete.contains(key) {
            return Err(LocalizationError::Missing(key.as_str().into()));
        }
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
