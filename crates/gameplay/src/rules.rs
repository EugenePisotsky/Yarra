//! Pure mechanics. Callers supply source stats, time and controlled randomness.
use game_types::{GameTime, Invalid, Key, Result, TextRef, require};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub id: Key,
    pub name: TextRef,
    pub minimum: i32,
    pub maximum: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub id: Key,
    pub name: TextRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub attributes: Vec<Attribute>,
    pub skills: Vec<Skill>,
    pub slots: BTreeSet<Key>,
    pub health_attribute: Key,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifier {
    pub attribute: Key,
    pub amount: i32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Heal(u32),
    Buff {
        modifier: Modifier,
        duration_ms: u64,
    },
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemMechanics {
    pub slot: Option<Key>,
    pub modifiers: Vec<Modifier>,
    pub on_use: Vec<Effect>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveEffect {
    pub modifier: Modifier,
    pub expires_at: GameTime,
}
pub type Attributes = BTreeMap<Key, i32>;
pub type SkillExperience = BTreeMap<Key, u64>;

impl Rules {
    pub fn validate(&self) -> Result<()> {
        require(
            !self.attributes.is_empty()
                && self.attributes.len() <= 128
                && self.skills.len() <= 256
                && self.slots.len() <= 64,
            "rules exceed limits",
        )?;
        let mut ids = BTreeSet::new();
        for a in &self.attributes {
            require(
                ids.insert(&a.id) && a.minimum <= a.maximum,
                "invalid/duplicate attribute",
            )?;
            a.name.validate()?;
        }
        let health = self.attribute(&self.health_attribute)?;
        require(health.minimum >= 1, "maximum health must be positive")?;
        let mut ids = BTreeSet::new();
        for skill in &self.skills {
            require(ids.insert(&skill.id), "duplicate skill")?;
            skill.name.validate()?;
        }
        Ok(())
    }
    pub fn attribute(&self, id: &Key) -> Result<&Attribute> {
        self.attributes
            .iter()
            .find(|a| &a.id == id)
            .ok_or_else(|| Invalid(format!("unknown attribute {}", id.as_str())))
    }
    pub fn skill(&self, id: &Key) -> Result<&Skill> {
        self.skills
            .iter()
            .find(|s| &s.id == id)
            .ok_or_else(|| Invalid(format!("unknown skill {}", id.as_str())))
    }
    pub fn validate_base(&self, base: &Attributes) -> Result<()> {
        require(
            base.len() == self.attributes.len(),
            "base attributes must match rules",
        )?;
        for (id, value) in base {
            let a = self.attribute(id)?;
            require(
                (a.minimum..=a.maximum).contains(value),
                "base attribute outside bounds",
            )?;
        }
        Ok(())
    }
    pub fn validate_mechanics(&self, mechanics: &ItemMechanics) -> Result<()> {
        mechanics.validate()?;
        if let Some(slot) = &mechanics.slot {
            require(self.slots.contains(slot), "unknown equipment slot")?;
        }
        for m in &mechanics.modifiers {
            self.attribute(&m.attribute)?;
        }
        for effect in &mechanics.on_use {
            if let Effect::Buff { modifier, .. } = effect {
                self.attribute(&modifier.attribute)?;
            }
        }
        Ok(())
    }
    pub fn derive<'a>(
        &self,
        base: &Attributes,
        modifiers: impl IntoIterator<Item = &'a Modifier>,
    ) -> Result<Attributes> {
        self.validate_base(base)?;
        // Sum first, then clamp once: results are independent of equipment order.
        let mut totals: BTreeMap<_, i64> = base
            .iter()
            .map(|(k, v)| (k.clone(), i64::from(*v)))
            .collect();
        for m in modifiers {
            let total = totals
                .get_mut(&m.attribute)
                .ok_or_else(|| Invalid("unknown modifier attribute".into()))?;
            *total = total
                .checked_add(i64::from(m.amount))
                .ok_or_else(|| Invalid("attribute overflow".into()))?;
        }
        totals
            .into_iter()
            .map(|(id, value)| {
                let a = self.attribute(&id)?;
                Ok((
                    id,
                    value.clamp(i64::from(a.minimum), i64::from(a.maximum)) as i32,
                ))
            })
            .collect()
    }
}
impl ItemMechanics {
    pub fn validate(&self) -> Result<()> {
        require(
            self.modifiers.len() <= 64 && self.on_use.len() <= 64,
            "too many item effects",
        )?;
        require(
            self.slot.is_some() || self.modifiers.is_empty(),
            "equipment modifiers need a slot",
        )?;
        for effect in &self.on_use {
            match effect {
                Effect::Heal(amount) => require(*amount > 0, "healing must be positive")?,
                Effect::Buff { duration_ms, .. } => {
                    require(*duration_ms > 0, "buff duration must be positive")?
                }
            }
        }
        Ok(())
    }
}

/// SplitMix64 algorithm/state is part of the save contract; no ambient RNG calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RandomState(pub u64);
impl RandomState {
    pub fn roll_d20(&mut self) -> u32 {
        self.below(20).expect("nonzero die size") + 1
    }
    /// Uniform draw in 0..upper, with rejection to avoid modulo bias.
    pub fn below(&mut self, upper: u32) -> Result<u32> {
        require(upper > 0, "empty random range")?;
        loop {
            self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
            z ^= z >> 31;
            if z > u64::MAX % u64::from(upper) {
                return Ok((z % u64::from(upper)) as u32);
            }
        }
    }
}
