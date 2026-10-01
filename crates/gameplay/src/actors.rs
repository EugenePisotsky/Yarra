//! Players, companions and NPCs share the same durable character model.
use crate::rules::{ActiveEffect, Rules, SkillRanks, Stats};
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How many things a character can have lined up to do.
pub const MAX_INTENTS: usize = 8;

/// Something a character means to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub ability: Key,
    /// Whom it is aimed at, for an ability that has a target.
    #[serde(default)]
    pub target: Option<ActorId>,
    /// Lined up again each time it has been done: a basic attack that goes on until the
    /// character is told to stop.
    #[serde(default)]
    pub repeat: bool,
}
/// What a character is in the middle of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acting {
    pub intent: Intent,
    /// When it takes effect.
    pub completes_at: GameTime,
}

fn first_level() -> u32 {
    1
}
/// A character the content knows by name: who it is built from and what it is called.
/// Content may name only declared characters, so a mistyped name fails the build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterDefinition {
    pub id: ActorId,
    pub template: ActorTemplateId,
    /// Its own name; without one it goes by its template's.
    #[serde(default)]
    pub name: Option<TextRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorTemplate {
    pub interaction: Option<InteractionProfileId>,
    pub id: ActorTemplateId,
    pub name: TextRef,
    pub class: Key,
    #[serde(default = "first_level")]
    pub level: u32,
    /// Primary stats that differ from the class's starting values.
    #[serde(default, deserialize_with = "game_types::deserialize_unique_map")]
    pub base: Stats,
    /// Skill ranks the character starts with.
    #[serde(default, deserialize_with = "game_types::deserialize_unique_map")]
    pub skills: SkillRanks,
    /// What the character wears and wields. It counts towards the stats from the start and
    /// is in the inventory once that is opened.
    #[serde(default)]
    pub equipment: Vec<ItemDefinitionId>,
    /// What else the inventory holds when it is first opened: loot, or a merchant's stock.
    #[serde(default)]
    pub loot: Option<LootId>,
}
/// Logical world coordinates in millimetres; never an ECS transform/handle.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub millimetres: [i64; 3],
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub id: ActorId,
    pub template: ActorTemplateId,
    pub name_override: Option<TextRef>,
    pub position: Position,
    pub class: Key,
    pub level: u32,
    /// Primary stats before equipment and effects.
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub base: Stats,
    /// Points not spent yet.
    pub attribute_points: u32,
    pub learning_points: u32,
    /// Skill ranks above zero.
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub skills: SkillRanks,
    pub abilities: BTreeSet<Key>,
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub equipment: BTreeMap<Key, ItemId>,
    pub effects: Vec<ActiveEffect>,
    /// How much of each resource the character has now.
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub resources: Stats,
    /// Every primary and derived stat after equipment and effects. Worked out again
    /// whenever something it depends on changes, so reading a stat is a lookup.
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub stats: Stats,
    pub acting: Option<Acting>,
    /// What the character means to do next, first first.
    pub intents: VecDeque<Intent>,
    /// When an ability that was used can be begun again.
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub cooldowns: BTreeMap<Key, GameTime>,
}
impl ActorTemplate {
    pub fn validate(&self, rules: &Rules) -> Result<()> {
        self.name.validate()?;
        rules.class(&self.class)?;
        require(
            (1..=rules.max_level()).contains(&self.level),
            "template level cannot be reached",
        )?;
        for (stat, value) in &self.base {
            let (minimum, maximum) = rules.primary(stat)?;
            require(
                (minimum..=maximum).contains(value),
                "template stat outside its bounds",
            )?;
        }
        for (skill, rank) in &self.skills {
            require(
                *rank > 0 && usize::from(*rank) <= rules.skill(skill)?.ranks.len(),
                "template skill rank out of range",
            )?;
        }
        Ok(())
    }
}
impl Actor {
    /// A character as its template and class describe it, levels included. Its stats and
    /// resources are filled in by the state, which knows the formulas.
    pub fn from_template(template: &ActorTemplate, rules: &Rules) -> Result<Self> {
        template.validate(rules)?;
        let class = rules.class(&template.class)?;
        let mut base = class.starting.clone();
        base.extend(template.base.clone());
        let mut actor = Self {
            id: ActorId::random(),
            template: template.id,
            name_override: None,
            position: Position::default(),
            class: template.class.clone(),
            level: 0,
            base,
            attribute_points: 0,
            learning_points: 0,
            skills: template.skills.clone(),
            abilities: BTreeSet::new(),
            equipment: BTreeMap::new(),
            effects: Vec::new(),
            resources: Stats::new(),
            stats: Stats::new(),
            acting: None,
            intents: VecDeque::new(),
            cooldowns: BTreeMap::new(),
        };
        while actor.level < template.level {
            actor.gain_level(rules)?;
        }
        Ok(actor)
    }
    /// Reaches the next level and takes what the class grants for it.
    pub fn gain_level(&mut self, rules: &Rules) -> Result<()> {
        require(
            self.level < rules.max_level(),
            "already at the highest level",
        )?;
        self.level += 1;
        let class = rules.class(&self.class)?;
        for grants in rules.grants(class, self.level) {
            self.attribute_points = self
                .attribute_points
                .saturating_add(grants.attribute_points);
            self.learning_points = self.learning_points.saturating_add(grants.learning_points);
            for (stat, bonus) in &grants.bonuses {
                let (minimum, maximum) = rules.primary(stat)?;
                let value = self.base.entry(stat.clone()).or_insert(minimum);
                *value = value.saturating_add(*bonus).clamp(minimum, maximum);
            }
            self.abilities.extend(grants.abilities.iter().cloned());
        }
        Ok(())
    }
    pub fn alive(&self, rules: &Rules) -> bool {
        self.resources
            .get(&rules.life)
            .is_some_and(|life| *life > 0)
    }
    pub fn skill(&self, skill: &Key) -> u8 {
        self.skills.get(skill).copied().unwrap_or(0)
    }
    /// Takes stats just worked out and keeps every resource within its cap. A resource the
    /// rules have gained starts full; one they no longer have is dropped.
    pub(crate) fn set_stats(&mut self, stats: Stats, rules: &Rules) {
        self.stats = stats;
        self.resources
            .retain(|resource, _| rules.resources().any(|(known, _)| known == resource));
        for (resource, maximum) in rules.resources() {
            let cap = self.stats[maximum].max(0);
            let amount = self.resources.entry(resource.clone()).or_insert(cap);
            *amount = (*amount).clamp(0, cap);
        }
    }
    /// Whether everything the stats are worked out from is the same as in `other`.
    pub fn same_build(&self, other: &Self) -> bool {
        self.class == other.class
            && self.level == other.level
            && self.base == other.base
            && self.skills == other.skills
            && self.equipment == other.equipment
            && self.effects.len() == other.effects.len()
            && self
                .effects
                .iter()
                .zip(&other.effects)
                .all(|(a, b)| a.effect == b.effect)
    }
    /// When something next happens to this character by itself: an effect ends or ticks,
    /// what it is doing takes effect, or the ability it is waiting to use is ready again.
    pub fn next_event(&self) -> Option<GameTime> {
        let waiting = match (&self.acting, self.intents.front()) {
            (None, Some(intent)) => self.cooldowns.get(&intent.ability).copied(),
            _ => None,
        };
        self.effects
            .iter()
            .flat_map(|e| [e.expires_at, e.next_tick])
            .flatten()
            .chain(self.acting.as_ref().map(|a| a.completes_at))
            .chain(waiting)
            .min()
    }
    /// The record's own shape. What it owns and what its stats should be is checked by
    /// the state.
    pub fn validate(&self, rules: &Rules) -> Result<()> {
        if let Some(name) = &self.name_override {
            name.validate()?;
        }
        let class = rules.class(&self.class)?;
        require(
            (1..=rules.max_level()).contains(&self.level),
            "level cannot be reached",
        )?;
        require(
            self.base.len() == class.starting.len(),
            "base stats must be the primary stats",
        )?;
        for (stat, value) in &self.base {
            let (minimum, maximum) = rules.primary(stat)?;
            require(
                (minimum..=maximum).contains(value),
                "base stat outside its bounds",
            )?;
        }
        require(
            self.effects.len() <= rules.effects.len() && self.equipment.len() <= rules.slots.len(),
            "actor exceeds limits",
        )?;
        for (skill, rank) in &self.skills {
            require(
                *rank > 0 && usize::from(*rank) <= rules.skill(skill)?.ranks.len(),
                "skill rank out of range",
            )?;
        }
        for ability in &self.abilities {
            rules.ability(ability)?;
        }
        require(
            self.intents.len() <= MAX_INTENTS && self.cooldowns.len() <= self.abilities.len(),
            "actor exceeds limits",
        )?;
        let intents = self
            .intents
            .iter()
            .chain(self.acting.as_ref().map(|a| &a.intent));
        for intent in intents {
            require(
                self.abilities.contains(&intent.ability)
                    && rules.ability(&intent.ability)?.targeted == intent.target.is_some(),
                "intent does not fit the ability",
            )?;
        }
        require(
            self.alive(rules) || (self.acting.is_none() && self.intents.is_empty()),
            "the dead do nothing",
        )?;
        for slot in self.equipment.keys() {
            require(rules.slots.contains(slot), "unknown equipment slot")?;
        }
        let mut effects = BTreeSet::new();
        for effect in &self.effects {
            let definition = rules.effect(&effect.effect)?;
            require(effects.insert(&effect.effect), "effect applied twice")?;
            require(
                effect.next_tick.is_some() == definition.periodic.is_some(),
                "effect tick does not match its definition",
            )?;
        }
        require(
            self.resources.len() == rules.resources().count(),
            "resources must be the rules' resources",
        )?;
        for (resource, maximum) in rules.resources() {
            let (Some(amount), Some(cap)) = (self.resources.get(resource), self.stats.get(maximum))
            else {
                return Err(Invalid("missing resource or its cap".into()));
            };
            require(
                (0..=(*cap).max(0)).contains(amount),
                "resource outside zero and its cap",
            )?;
        }
        Ok(())
    }
}

/// A directed NPC/actor attitude. A-to-B and B-to-A are independent records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RelationshipKey {
    pub from: ActorId,
    pub to: ActorId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub key: RelationshipKey,
    pub attitude: i16,
}
impl Relationship {
    pub fn neutral(key: RelationshipKey) -> Self {
        Self { key, attitude: 0 }
    }
    pub fn validate(&self) -> Result<()> {
        require(
            self.key.from != self.key.to && (-100..=100).contains(&self.attitude),
            "invalid directed relationship",
        )
    }
    pub fn adjust(&mut self, amount: i16) -> Result<()> {
        self.validate()?;
        self.attitude = (i32::from(self.attitude) + i32::from(amount)).clamp(-100, 100) as i16;
        Ok(())
    }
}
