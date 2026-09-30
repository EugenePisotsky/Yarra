//! Immutable record contracts. Identity is independent of authoring paths.
use crate::{ContentError, Result};
use game_types::*;
use gameplay::{Action, Condition};
use gameplay::{actors, dialogue, inventory, quests, rules};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
    Claim,
    Object,
    Area,
    Trigger,
    Script,
}
impl AssetKind {
    pub const ALL: [Self; 16] = [
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
        Self::Claim,
        Self::Object,
        Self::Area,
        Self::Trigger,
        Self::Script,
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
    Claim(ClaimId),
    Object(ObjectId),
    Area(AreaId),
    Trigger(TriggerId),
    Script(Key),
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
            Self::Claim(_) => AssetKind::Claim,
            Self::Object(_) => AssetKind::Object,
            Self::Area(_) => AssetKind::Area,
            Self::Trigger(_) => AssetKind::Trigger,
            Self::Script(_) => AssetKind::Script,
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
            Self::Claim(id) => id.raw(),
            Self::Object(id) => id.raw(),
            Self::Area(id) => id.raw(),
            Self::Trigger(id) => id.raw(),
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
            AssetKind::Claim => Self::Claim(ClaimId::try_from(key.clone())?),
            AssetKind::Predicate => Self::Predicate(PredicateId::try_from(key.clone())?),
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
    Claim(dialogue::ClaimDefinition),
    Object(gameplay::ObjectDefinition),
    Area(AreaId),
    Trigger(gameplay::TriggerDefinition),
    Script(gameplay::ScriptModule),
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
            Self::Rules(v) => v
                .attributes
                .iter()
                .map(|a| &a.name)
                .chain(v.skills.iter().map(|s| &s.name))
                .collect(),
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
            Self::Claim(v) => AssetId::Claim(v.id),
            Self::Object(v) => AssetId::Object(v.id),
            Self::Area(id) => AssetId::Area(*id),
            Self::Trigger(v) => AssetId::Trigger(v.id),
        }
    }
    /// Other assets this one refers to. Authoring uses it to check that a package declares
    /// the packages it draws from; the runtime never follows these.
    pub(crate) fn references(&self) -> Result<BTreeSet<AssetId>> {
        let mut refs = BTreeSet::new();
        match self {
            Self::Trigger(v) => {
                refs.insert(AssetId::Rules);
                if let Some(condition) = &v.condition {
                    condition_references(condition, &mut refs)?;
                }
                for signal in &v.on {
                    use gameplay::WorldSignal::*;
                    refs.extend(match signal {
                        Entered { area, .. } | Exited { area, .. } | Arrived { area, .. } => {
                            Some(AssetId::Area(*area))
                        }
                        ItemAcquired { definition, .. } => Some(AssetId::Item(*definition)),
                        QuestStarted(id) | QuestChanged(id) => Some(AssetId::Quest(*id)),
                        VariableChanged(id) => Some(AssetId::Variable(*id)),
                        DialogueCompleted(id) => Some(AssetId::DialogueContract(*id)),
                        MoveFailed(_) => None,
                    });
                }
                for action in &v.actions {
                    action_references(action, &mut refs)?;
                }
            }
            Self::Text(v) => refs.extend(v.imports.iter().copied().map(AssetId::Text)),
            Self::Item(v) => {
                refs.insert(AssetId::Category(v.category));
                refs.insert(AssetId::Rules);
            }
            Self::Actor(v) => {
                refs.insert(AssetId::Rules);
                refs.extend(v.interaction.map(AssetId::Profile));
            }
            Self::Dialogue(v) => {
                refs.insert(AssetId::DialogueContract(v.id));
                refs.insert(AssetId::Rules);
                for node in &v.nodes {
                    if let Some(condition) = &node.condition {
                        condition_references(condition, &mut refs)?;
                    }
                    for action in &node.actions {
                        action_references(action, &mut refs)?;
                    }
                }
            }
            Self::Predicate(v) => condition_references(&v.condition, &mut refs)?,
            Self::Profile(v) => {
                for rule in &v.rules {
                    for variant in &rule.variants {
                        refs.insert(AssetId::DialogueContract(variant.dialogue));
                        refs.insert(AssetId::Dialogue(variant.dialogue));
                    }
                    condition_references(&rule.condition, &mut refs)?;
                }
            }
            _ => {}
        }
        refs.extend(
            self.text_references()
                .iter()
                .map(|r| AssetId::Text(r.resource)),
        );
        Ok(refs)
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
                    categories: vec![v.clone()],
                    items: vec![],
                }
                .validate()?;
            }
            Self::Item(v) => v.validate()?,
            Self::Actor(v) => {
                v.name.validate()?;
                require(v.base.len() <= 128, "actor attributes exceed limit")?;
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

fn condition_references(condition: &Condition, refs: &mut BTreeSet<AssetId>) -> Result<()> {
    condition.visit(&mut |c| {
        refs.extend(match c {
            Condition::InsideArea { area } => Some(AssetId::Area(*area)),
            Condition::History { dialogue, .. } => Some(AssetId::DialogueContract(*dialogue)),
            Condition::Claimed { claim, .. } => Some(AssetId::Claim(*claim)),
            Condition::HasItem { definition, .. } => Some(AssetId::Item(*definition)),
            Condition::Variable { variable, .. } => Some(AssetId::Variable(*variable)),
            Condition::QuestStatus { quest, .. } | Condition::ObjectiveCompleted { quest, .. } => {
                Some(AssetId::Quest(*quest))
            }
            Condition::Named(id) => Some(AssetId::Predicate(*id)),
            Condition::Script(name) => Some(AssetId::Script(name.module.clone())),
            Condition::SkillExperience { .. } => Some(AssetId::Rules),
            _ => None,
        });
        Ok(())
    })?;
    Ok(())
}
fn action_references(action: &Action, refs: &mut BTreeSet<AssetId>) -> Result<()> {
    action.visit(&mut |a| {
        refs.extend(match a {
            Action::Script(name) => Some(AssetId::Script(name.module.clone())),
            Action::SetLocked { object, .. } => Some(AssetId::Object(*object)),
            Action::Claim { claim, .. } => Some(AssetId::Claim(*claim)),
            Action::Quest { quest, .. } => Some(AssetId::Quest(*quest)),
            Action::GrantItem { definition, .. } | Action::ConsumeItem { definition, .. } => {
                Some(AssetId::Item(*definition))
            }
            Action::Set { variable, .. } | Action::Add { variable, .. } => {
                Some(AssetId::Variable(*variable))
            }
            Action::AwardExperience { .. } | Action::SkillCheck { .. } => Some(AssetId::Rules),
            Action::Move { to, .. } => Some(AssetId::Area(*to)),
            Action::StartDialogue { dialogue, .. } => Some(AssetId::DialogueContract(*dialogue)),
            Action::Relationship { .. } => None,
        });
        Ok(())
    })?;
    Ok(())
}
