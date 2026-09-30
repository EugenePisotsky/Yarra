//! Players, companions and NPCs share the same durable actor model.
use game_types::*;
use rules::{ActiveEffect, Attributes, Rules, SkillExperience};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorTemplate {
    pub interaction: Option<InteractionProfileId>,
    pub id: ActorTemplateId,
    pub name: TextRef,
    #[serde(deserialize_with = "game_types::deserialize_key_map")]
    pub base: Attributes,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorRole {
    Player,
    Companion,
    Npc,
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
    pub role: ActorRole,
    pub position: Position,
    #[serde(deserialize_with = "game_types::deserialize_key_map")]
    pub base: Attributes,
    pub health: u32,
    #[serde(deserialize_with = "game_types::deserialize_key_map")]
    pub skills: SkillExperience,
    #[serde(deserialize_with = "game_types::deserialize_key_map")]
    pub equipment: BTreeMap<Key, ItemId>,
    pub effects: Vec<ActiveEffect>,
}
impl ActorTemplate {
    pub fn validate(&self, rules: &Rules) -> Result<()> {
        self.name.validate()?;
        rules.validate_base(&self.base)
    }
}
impl Actor {
    pub fn from_template(template: &ActorTemplate, rules: &Rules, role: ActorRole) -> Result<Self> {
        rules.validate()?;
        template.validate(rules)?;
        Ok(Self {
            id: ActorId::new(),
            template: template.id,
            name_override: None,
            role,
            position: Position::default(),
            base: template.base.clone(),
            health: template.base[&rules.health_attribute] as u32,
            skills: BTreeMap::new(),
            equipment: BTreeMap::new(),
            effects: Vec::new(),
        })
    }
    /// Cross-domain ownership and derived health bounds are checked by gameplay.
    pub fn validate(&self, rules: &Rules) -> Result<()> {
        rules.validate_base(&self.base)?;
        if let Some(name) = &self.name_override {
            name.validate()?;
        }
        require(
            self.effects.len() <= 256
                && self.equipment.len() <= rules.slots.len()
                && self.skills.len() <= rules.skills.len(),
            "actor exceeds limits",
        )?;
        for skill in self.skills.keys() {
            rules.skill(skill)?;
        }
        for slot in self.equipment.keys() {
            require(rules.slots.contains(slot), "unknown equipment slot")?;
        }
        for effect in &self.effects {
            rules.attribute(&effect.modifier.attribute)?;
        }
        Ok(())
    }
    pub fn award_experience(&mut self, rules: &Rules, skill: &Key, amount: u64) -> Result<()> {
        rules.skill(skill)?;
        let xp = self
            .skills
            .get(skill)
            .copied()
            .unwrap_or(0)
            .checked_add(amount)
            .ok_or_else(|| Invalid("skill XP overflow".into()))?;
        self.skills.insert(skill.clone(), xp);
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
