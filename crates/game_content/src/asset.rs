//! Immutable record contracts. Identity is independent of authoring paths.
use crate::{ContentError, Result};
use game_types::*;
use gameplay::{Action, Condition};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_ASSET_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ASSET_DEPENDENCIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(i64)]
pub enum AssetKind {
    Category = 1,
    Item,
    Actor,
    Rules,
    Dialogue,
    Condition,
    Action,
    Fact,
    Text,
    Quest,
    Profile,
    Predicate,
    DialogueContract,
    Claim,
    Object,
    Area,
    Trigger,
}
impl AssetKind {
    pub const ALL: [Self; 17] = [
        Self::Category,
        Self::Item,
        Self::Actor,
        Self::Rules,
        Self::Dialogue,
        Self::Condition,
        Self::Action,
        Self::Fact,
        Self::Text,
        Self::Quest,
        Self::Profile,
        Self::Predicate,
        Self::DialogueContract,
        Self::Claim,
        Self::Object,
        Self::Area,
        Self::Trigger,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AssetId {
    Category(CategoryId),
    Item(ItemDefinitionId),
    Actor(ActorTemplateId),
    Rules,
    Dialogue(DialogueId),
    Condition(BindingId),
    Action(BindingId),
    Fact(Key),
    Text(TextResourceId),
    Quest(QuestId),
    Profile(InteractionProfileId),
    Predicate(PredicateId),
    DialogueContract(DialogueId),
    Claim(ClaimId),
    Object(ObjectId),
    Area(AreaId),
    Trigger(TriggerId),
}
impl AssetId {
    pub fn kind(&self) -> AssetKind {
        match self {
            Self::Category(_) => AssetKind::Category,
            Self::Item(_) => AssetKind::Item,
            Self::Actor(_) => AssetKind::Actor,
            Self::Rules => AssetKind::Rules,
            Self::Dialogue(_) => AssetKind::Dialogue,
            Self::Condition(_) => AssetKind::Condition,
            Self::Action(_) => AssetKind::Action,
            Self::Fact(_) => AssetKind::Fact,
            Self::Text(_) => AssetKind::Text,
            Self::Quest(_) => AssetKind::Quest,
            Self::Profile(_) => AssetKind::Profile,
            Self::Predicate(_) => AssetKind::Predicate,
            Self::DialogueContract(_) => AssetKind::DialogueContract,
            Self::Claim(_) => AssetKind::Claim,
            Self::Object(_) => AssetKind::Object,
            Self::Area(_) => AssetKind::Area,
            Self::Trigger(_) => AssetKind::Trigger,
        }
    }
    pub fn key(&self) -> String {
        match self {
            Self::Category(id) => id.to_string(),
            Self::Item(id) => id.to_string(),
            Self::Actor(id) => id.to_string(),
            Self::Rules => "rules".into(),
            Self::Dialogue(id) => id.to_string(),
            Self::Condition(key) | Self::Action(key) => key.clone().into(),
            Self::Fact(key) => key.as_str().into(),
            Self::Text(resource) => resource.to_string(),
            Self::Quest(id) => id.to_string(),
            Self::Profile(id) => id.to_string(),
            Self::Predicate(id) => id.to_string(),
            Self::DialogueContract(id) => id.to_string(),
            Self::Claim(id) => id.to_string(),
            Self::Object(id) => id.to_string(),
            Self::Area(id) => id.to_string(),
            Self::Trigger(id) => id.to_string(),
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
            AssetKind::Condition => Self::Condition(BindingId::try_from(key.clone())?),
            AssetKind::Action => Self::Action(BindingId::try_from(key.clone())?),
            AssetKind::Fact => Self::Fact(Key::new(&key)?),
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
    Condition { id: BindingId, value: Condition },
    Action { id: BindingId, value: Action },
    Fact(Key),
    Text(TextContract),
    Quest(quests::Quest),
    Profile(gameplay::InteractionProfile),
    Predicate(gameplay::NamedPredicate),
    DialogueContract(dialogue::DialogueContract),
    Claim(dialogue::ClaimDefinition),
    Object(gameplay::ObjectDefinition),
    Area(gameplay::AreaDefinition),
    Trigger(gameplay::TriggerDefinition),
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
                .nodes
                .iter()
                .flat_map(|n| {
                    n.lines
                        .iter()
                        .map(|l| (&l.text, &l.arguments))
                        .chain(n.choices.iter().map(|c| (&c.text, &c.arguments)))
                })
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
            Self::Condition { id, .. } => AssetId::Condition(id.clone()),
            Self::Action { id, .. } => AssetId::Action(id.clone()),
            Self::Fact(id) => AssetId::Fact(id.clone()),
            Self::Text(v) => AssetId::Text(v.id),
            Self::Quest(v) => AssetId::Quest(v.id),
            Self::Profile(v) => AssetId::Profile(v.id),
            Self::Predicate(v) => AssetId::Predicate(v.id),
            Self::DialogueContract(v) => AssetId::DialogueContract(v.id),
            Self::Claim(v) => AssetId::Claim(v.id),
            Self::Object(v) => AssetId::Object(v.id),
            Self::Area(v) => AssetId::Area(v.id),
            Self::Trigger(v) => AssetId::Trigger(v.id),
        }
    }
    /// Mechanical dependencies only. Presentation explicitly requests a locale/resource.
    pub fn dependencies(&self) -> Result<BTreeSet<AssetId>> {
        let mut deps = BTreeSet::new();
        match self {
            Self::Trigger(v) => {
                condition_dependencies(&v.condition, &mut deps)?;
                deps.insert(AssetId::Rules);
                let mut request = gameplay::ContentRequest::default();
                v.content_dependencies(&mut request)?;
                deps.extend(request.items.into_iter().map(AssetId::Item));
                deps.extend(request.areas.into_iter().map(AssetId::Area));
                deps.extend(request.quests.into_iter().map(AssetId::Quest));
                deps.extend(
                    request
                        .dialogue_contracts
                        .into_iter()
                        .map(AssetId::DialogueContract),
                );
                deps.extend(request.claims.into_iter().map(AssetId::Claim));
                deps.extend(request.facts.into_iter().map(AssetId::Fact));
                for step in &v.steps {
                    if let gameplay::SequenceStep::Apply(actions) = step {
                        for action in actions {
                            action_dependencies(action, 0, &mut 1024, &mut deps)?;
                        }
                    }
                }
            }
            Self::Text(v) => {
                deps.extend(v.imports.iter().copied().map(AssetId::Text));
            }
            Self::Item(v) => {
                deps.insert(AssetId::Category(v.category));
                deps.insert(AssetId::Rules);
            }
            Self::Actor(_) => {
                deps.insert(AssetId::Rules);
            }
            Self::Dialogue(v) => {
                deps.insert(AssetId::DialogueContract(v.id));
                deps.insert(AssetId::Rules);
                deps.extend(
                    self.text_references()
                        .iter()
                        .map(|r| AssetId::Text(r.resource)),
                );
                for node in &v.nodes {
                    for choice in &node.choices {
                        deps.extend(
                            choice
                                .conditions
                                .iter()
                                .cloned()
                                .map(|key| AssetId::Condition(BindingId::new(v.id, key))),
                        );
                        deps.extend(
                            choice
                                .actions
                                .iter()
                                .cloned()
                                .map(|key| AssetId::Action(BindingId::new(v.id, key))),
                        );
                    }
                }
            }
            Self::Condition { value, .. } => condition_dependencies(value, &mut deps)?,
            Self::Predicate(v) => condition_dependencies(&v.condition, &mut deps)?,
            Self::Profile(v) => {
                for rule in &v.rules {
                    deps.extend(
                        rule.variants
                            .iter()
                            .map(|v| AssetId::DialogueContract(v.dialogue)),
                    );
                    condition_dependencies(&rule.condition, &mut deps)?;
                }
            }
            Self::Action { value, .. } => action_dependencies(value, 0, &mut 1024, &mut deps)?,
            _ => {}
        }
        require(
            deps.len() <= MAX_ASSET_DEPENDENCIES,
            "asset dependency limit exceeded",
        )?;
        Ok(deps)
    }
    /// Published references checked independently of the demand-loaded dependency closure.
    pub(crate) fn selection_links(&self) -> Vec<AssetId> {
        match self {
            Self::Actor(v) => v.interaction.map(AssetId::Profile).into_iter().collect(),
            Self::Profile(v) => v
                .rules
                .iter()
                .flat_map(|r| r.variants.iter().map(|v| AssetId::Dialogue(v.dialogue)))
                .collect(),
            _ => vec![],
        }
    }
    pub(crate) fn validate_local(&self) -> Result<()> {
        match self {
            Self::Object(v) => v.name.validate()?,
            Self::Area(v) => v.validate()?,
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
        self.dependencies()?;
        Ok(())
    }
}
fn action_dependencies(
    action: &Action,
    depth: usize,
    budget: &mut usize,
    deps: &mut BTreeSet<AssetId>,
) -> Result<()> {
    require(depth <= 8 && *budget > 0, "action complexity exceeded")?;
    *budget -= 1;
    match action {
        Action::SetLocked { object, .. } => {
            deps.insert(AssetId::Object(*object));
        }
        Action::Claim { claim, actions } => {
            require(!actions.is_empty(), "empty claimed action group")?;
            deps.insert(AssetId::Claim(*claim));
            for action in actions {
                action_dependencies(action, depth + 1, budget, deps)?;
            }
        }
        Action::Quest { quest, .. } => {
            deps.insert(AssetId::Quest(*quest));
        }
        Action::Relationship { from, to, .. } => {
            require(from != to, "relationship needs two roles")?
        }
        Action::GrantItem {
            definition,
            quantity,
        }
        | Action::ConsumeItem {
            definition,
            quantity,
        } => {
            require(*quantity > 0, "zero item action")?;
            deps.insert(AssetId::Item(*definition));
        }
        Action::AwardExperience { amount, .. } => {
            require(*amount > 0, "zero XP reward")?;
            deps.insert(AssetId::Rules);
        }
        Action::SetFact { key, .. } => {
            deps.insert(AssetId::Fact(key.clone()));
        }
        Action::SkillCheck {
            difficulty,
            success,
            failure,
            ..
        } => {
            require(*difficulty > 0, "zero check difficulty")?;
            deps.insert(AssetId::Rules);
            for child in success.iter().chain(failure) {
                action_dependencies(child, depth + 1, budget, deps)?;
            }
        }
    }
    Ok(())
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

fn condition_dependencies(condition: &Condition, deps: &mut BTreeSet<AssetId>) -> Result<()> {
    let mut request = gameplay::ContentRequest::default();
    condition.content_dependencies(&mut request)?;
    deps.extend(request.areas.into_iter().map(AssetId::Area));
    deps.extend(request.claims.into_iter().map(AssetId::Claim));
    deps.extend(
        request
            .dialogue_contracts
            .into_iter()
            .map(AssetId::DialogueContract),
    );
    deps.extend(request.items.into_iter().map(AssetId::Item));
    deps.extend(request.facts.into_iter().map(AssetId::Fact));
    deps.extend(request.quests.into_iter().map(AssetId::Quest));
    deps.extend(request.predicates.into_iter().map(AssetId::Predicate));
    condition.visit(&mut |c| {
        match c {
            Condition::SkillExperience { .. } => {
                deps.insert(AssetId::Rules);
            }
            Condition::History { minimum, .. } => require(*minimum > 0, "zero history threshold")?,
            Condition::HasItem { quantity, .. } => require(*quantity > 0, "zero item condition")?,
            Condition::Relationship { from, to, minimum } => require(
                from != to && (-100..=100).contains(minimum),
                "invalid relationship condition",
            )?,
            _ => {}
        };
        Ok(())
    })?;
    Ok(())
}
