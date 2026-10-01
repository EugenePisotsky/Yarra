//! Immutable record contracts. Identity is independent of authoring paths.
use crate::{ContentError, Result};
use game_types::*;
use gameplay::{actors, dialogue, inventory, quests, rules};
use serde::{Deserialize, Serialize};

pub const MAX_ASSET_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(i64)]
pub enum AssetKind {
    Category = 1,
    Item,
    Actor,
    Rules,
    Dialogue,
    Variable,
    Text,
    Quest,
    Profile,
    Predicate,
    DialogueContract,
    Object,
    Area,
    Trigger,
    Script,
    Loot,
    Character,
}
impl AssetKind {
    pub const ALL: [Self; 17] = [
        Self::Category,
        Self::Item,
        Self::Actor,
        Self::Rules,
        Self::Dialogue,
        Self::Variable,
        Self::Text,
        Self::Quest,
        Self::Profile,
        Self::Predicate,
        Self::DialogueContract,
        Self::Object,
        Self::Area,
        Self::Trigger,
        Self::Script,
        Self::Loot,
        Self::Character,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AssetId {
    Category(CategoryId),
    Item(ItemDefinitionId),
    Actor(ActorTemplateId),
    Rules,
    Dialogue(DialogueId),
    Variable(VariableId),
    Text(TextResourceId),
    Quest(QuestId),
    Profile(InteractionProfileId),
    Predicate(PredicateId),
    DialogueContract(DialogueId),
    Object(ObjectId),
    Area(AreaId),
    Trigger(TriggerId),
    Script(Key),
    Loot(LootId),
    Character(ActorId),
}
impl AssetId {
    pub fn kind(&self) -> AssetKind {
        match self {
            Self::Category(_) => AssetKind::Category,
            Self::Item(_) => AssetKind::Item,
            Self::Actor(_) => AssetKind::Actor,
            Self::Rules => AssetKind::Rules,
            Self::Dialogue(_) => AssetKind::Dialogue,
            Self::Variable(_) => AssetKind::Variable,
            Self::Text(_) => AssetKind::Text,
            Self::Quest(_) => AssetKind::Quest,
            Self::Profile(_) => AssetKind::Profile,
            Self::Predicate(_) => AssetKind::Predicate,
            Self::DialogueContract(_) => AssetKind::DialogueContract,
            Self::Object(_) => AssetKind::Object,
            Self::Area(_) => AssetKind::Area,
            Self::Trigger(_) => AssetKind::Trigger,
            Self::Script(_) => AssetKind::Script,
            Self::Loot(_) => AssetKind::Loot,
            Self::Character(_) => AssetKind::Character,
        }
    }
    /// Storage key: the identity bytes, never the name, so lookups do not depend on which
    /// names this process has seen.
    pub fn key(&self) -> String {
        match self {
            Self::Category(id) => id.raw(),
            Self::Item(id) => id.raw(),
            Self::Actor(id) => id.raw(),
            Self::Rules => "rules".into(),
            Self::Dialogue(id) => id.raw(),
            Self::Variable(id) => id.raw(),
            Self::Script(key) => key.as_str().into(),
            Self::Text(resource) => resource.raw(),
            Self::Quest(id) => id.raw(),
            Self::Profile(id) => id.raw(),
            Self::Predicate(id) => id.raw(),
            Self::DialogueContract(id) => id.raw(),
            Self::Object(id) => id.raw(),
            Self::Area(id) => id.raw(),
            Self::Trigger(id) => id.raw(),
            Self::Loot(id) => id.raw(),
            Self::Character(id) => id.raw(),
        }
    }
    pub(crate) fn from_key(kind: AssetKind, key: String) -> Result<Self> {
        require(key.len() <= 192, "asset key exceeds limit")?;
        let id = match kind {
            AssetKind::Category => Self::Category(CategoryId::try_from(key.clone())?),
            AssetKind::Item => Self::Item(ItemDefinitionId::try_from(key.clone())?),
            AssetKind::Actor => Self::Actor(ActorTemplateId::try_from(key.clone())?),
            AssetKind::Rules => Self::Rules,
            AssetKind::Dialogue => Self::Dialogue(DialogueId::try_from(key.clone())?),
            AssetKind::Variable => Self::Variable(VariableId::try_from(key.clone())?),
            AssetKind::Script => Self::Script(Key::new(&key)?),
            AssetKind::Text => Self::Text(TextResourceId::try_from(key.clone())?),
            AssetKind::Quest => Self::Quest(QuestId::try_from(key.clone())?),
            AssetKind::Profile => Self::Profile(InteractionProfileId::try_from(key.clone())?),
            AssetKind::DialogueContract => {
                Self::DialogueContract(DialogueId::try_from(key.clone())?)
            }
            AssetKind::Object => Self::Object(ObjectId::try_from(key.clone())?),
            AssetKind::Area => Self::Area(AreaId::try_from(key.clone())?),
            AssetKind::Trigger => Self::Trigger(TriggerId::try_from(key.clone())?),
            AssetKind::Predicate => Self::Predicate(PredicateId::try_from(key.clone())?),
            AssetKind::Loot => Self::Loot(LootId::try_from(key.clone())?),
            AssetKind::Character => Self::Character(ActorId::try_from(key.clone())?),
        };
        require(id.key() == key, "noncanonical asset key")?;
        Ok(id)
    }
}
pub(crate) fn validate_locale(locale: &str) -> Result<()> {
    require(locale.len() <= 64, "locale exceeds limit")?;
    let parsed: unic_langid::LanguageIdentifier = locale
        .parse()
        .map_err(|_| Invalid("invalid locale".into()))?;
    require(parsed == locale, "locale must be canonical")?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Asset {
    Category(inventory::Category),
    Item(inventory::ItemDefinition),
    Actor(actors::ActorTemplate),
    Rules(rules::Rules),
    Dialogue(dialogue::Dialogue),
    Variable(gameplay::VariableDefinition),
    Text(TextContract),
    Quest(quests::Quest),
    Profile(gameplay::InteractionProfile),
    Predicate(gameplay::NamedPredicate),
    DialogueContract(dialogue::DialogueContract),
    Object(gameplay::ObjectDefinition),
    Area(AreaId),
    Trigger(gameplay::TriggerDefinition),
    Script(gameplay::ScriptModule),
    Loot(inventory::LootTable),
    Character(actors::CharacterDefinition),
}
impl Asset {
    pub(crate) fn text_references(&self) -> Vec<&MessageRef> {
        let texts: Vec<&TextRef> = match self {
            Self::Quest(q) => std::iter::once(&q.title)
                .chain(q.objectives.iter().map(|o| &o.title))
                .collect(),
            Self::Profile(p) => p.rules.iter().filter_map(|r| r.topic.as_ref()).collect(),
            Self::Object(v) => vec![&v.name],
            Self::Category(v) => vec![&v.name],
            Self::Item(v) => vec![&v.name, &v.description],
            Self::Actor(v) => vec![&v.name],
            Self::Character(v) => v.name.iter().collect(),
            Self::Rules(v) => v.names().collect(),
            Self::Dialogue(v) => v
                .messages()
                .flat_map(|(t, args)| {
                    std::iter::once(t).chain(args.values().filter_map(|a| {
                        if let dialogue::ArgumentSource::Text(t) = a {
                            Some(t)
                        } else {
                            None
                        }
                    }))
                })
                .collect(),
            _ => Vec::new(),
        };
        texts
            .into_iter()
            .filter_map(|t| {
                if let TextRef::Message(m) = t {
                    Some(m)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn id(&self) -> AssetId {
        match self {
            Self::Category(v) => AssetId::Category(v.id),
            Self::Item(v) => AssetId::Item(v.id),
            Self::Actor(v) => AssetId::Actor(v.id),
            Self::Rules(_) => AssetId::Rules,
            Self::Dialogue(v) => AssetId::Dialogue(v.id),
            Self::Variable(v) => AssetId::Variable(v.id),
            Self::Script(v) => AssetId::Script(v.name.clone()),
            Self::Text(v) => AssetId::Text(v.id),
            Self::Quest(v) => AssetId::Quest(v.id),
            Self::Profile(v) => AssetId::Profile(v.id),
            Self::Predicate(v) => AssetId::Predicate(v.id),
            Self::DialogueContract(v) => AssetId::DialogueContract(v.id),
            Self::Object(v) => AssetId::Object(v.id),
            Self::Area(id) => AssetId::Area(*id),
            Self::Trigger(v) => AssetId::Trigger(v.id),
            Self::Loot(v) => AssetId::Loot(v.id),
            Self::Character(v) => AssetId::Character(v.id),
        }
    }
    pub(crate) fn validate_local(&self) -> Result<()> {
        match self {
            Self::Object(v) => v.name.validate()?,
            Self::Trigger(v) => v.validate()?,
            Self::DialogueContract(v) => v.validate()?,
            Self::Quest(v) => v.validate()?,
            Self::Profile(v) => v.validate()?,
            Self::Category(v) => {
                // Reuse category validation, including its distinct semantic key syntax.
                inventory::ItemCatalog {
                    id: CatalogId([0; 16]),
                    revision: 1,
                    categories: [(v.id, v.clone())].into(),
                    items: Default::default(),
                }
                .validate()?;
            }
            Self::Item(v) => v.validate()?,
            Self::Actor(v) => v.name.validate()?,
            Self::Character(v) => {
                if let Some(name) = &v.name {
                    name.validate()?;
                }
            }
            Self::Rules(v) => v.validate()?,
            Self::Dialogue(v) => v.validate()?,
            Self::Text(v) => {
                v.validate()?;
            }
            _ => {}
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetHeader {
    pub id: AssetId,
    /// Authoring order retained for tools; keyset pagination uses identity instead.
    pub position: u32,
    pub payload_bytes: usize,
    pub hash: [u8; 32],
}
pub(crate) fn corrupt(id: &AssetId, message: impl Into<String>) -> ContentError {
    ContentError::CorruptAsset {
        id: id.clone(),
        message: message.into(),
    }
}
