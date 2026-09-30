//! The rules of a game as data: which stats exist, what modifies them, how a character is
//! built from a class and levels. The formulas themselves are two functions in the rules
//! script the content names, so a game changes its numbers without changing this crate.
use crate::ScriptName;
use game_types::{GameTime, Invalid, Key, Result, TextRef, require};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatKind {
    /// Stored for each character and raised by spending attribute points.
    Primary { minimum: i32, maximum: i32 },
    /// Worked out from a character's primaries, level, class and skills by the rules script.
    Derived { minimum: i32, maximum: i32 },
    /// An amount that is spent and restored, between zero and another stat.
    Resource { maximum: Key },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stat {
    pub id: Key,
    pub name: TextRef,
    pub kind: StatKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub id: Key,
    pub name: TextRef,
    /// Learning points each rank costs, the first rank first.
    pub ranks: Vec<u32>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Add(i32),
    /// Percent of the value: `Multiply(150)` is one and a half times. Several multipliers
    /// add their differences from 100, so two of 150 make 200.
    Multiply(i32),
    /// The stat is exactly this, whatever else applies. The largest of several wins.
    Override(i32),
}
/// A change to one stat. Where it comes from is where it is written: an item, an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifier {
    pub stat: Key,
    pub op: Operation,
}
/// The value after modifiers, independent of the order they are listed in.
pub fn modified(base: i64, operations: impl IntoIterator<Item = Operation>) -> i64 {
    let (mut sum, mut percent, mut fixed) = (base, 100i64, None::<i64>);
    for operation in operations {
        match operation {
            Operation::Add(amount) => sum = sum.saturating_add(i64::from(amount)),
            Operation::Multiply(p) => percent = percent.saturating_add(i64::from(p) - 100),
            Operation::Override(value) => {
                fixed = Some(fixed.map_or(i64::from(value), |v| v.max(i64::from(value))))
            }
        }
    }
    fixed.unwrap_or_else(|| sum.saturating_mul(percent.max(0)).div_euclid(100))
}
/// A change to a resource at regular intervals while an effect lasts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Periodic {
    pub resource: Key,
    /// Negative takes away: poison. Positive gives back: regeneration.
    pub amount: i32,
    pub period_ms: u64,
}
/// A condition a character can be in: a blessing, a poison. At most one of each at a time;
/// applying it again starts its time over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusEffect {
    pub id: Key,
    pub name: TextRef,
    #[serde(default)]
    pub modifiers: Vec<Modifier>,
    #[serde(default)]
    pub periodic: Option<Periodic>,
}
/// What a character receives on reaching a level.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    /// Spent freely on primary stats.
    #[serde(default)]
    pub attribute_points: u32,
    /// Spent with trainers on skill ranks.
    #[serde(default)]
    pub learning_points: u32,
    /// Added to primary stats outright.
    #[serde(default, deserialize_with = "game_types::deserialize_key_map")]
    pub bonuses: BTreeMap<Key, i32>,
    /// Abilities the character knows from then on.
    #[serde(default)]
    pub abilities: BTreeSet<Key>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Class {
    pub id: Key,
    pub name: TextRef,
    /// Primary stats of a first-level character.
    #[serde(deserialize_with = "game_types::deserialize_key_map")]
    pub starting: Stats,
    /// Granted at every level after the first.
    #[serde(default)]
    pub per_level: Grants,
    /// Granted on reaching particular levels, level 1 included, on top of `per_level`.
    #[serde(default)]
    pub at_level: BTreeMap<u32, Grants>,
    /// The highest rank of each skill a trainer can teach this class. A skill that is not
    /// listed cannot be learned.
    #[serde(default, deserialize_with = "game_types::deserialize_key_map")]
    pub skills: BTreeMap<Key, u8>,
}
/// Something a character does that takes time: a strike, a spell. It takes effect
/// `duration_ms` after it is begun, by running its script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ability {
    pub id: Key,
    pub name: TextRef,
    pub duration_ms: u64,
    /// Time after it takes effect before the same character can begin it again.
    #[serde(default)]
    pub cooldown_ms: u64,
    /// Resources paid when it is begun.
    #[serde(default, deserialize_with = "game_types::deserialize_key_map")]
    pub costs: BTreeMap<Key, u32>,
    /// Whether it is aimed at another character.
    pub targeted: bool,
    pub resolve: ScriptName,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub stats: Vec<Stat>,
    pub skills: Vec<Skill>,
    pub slots: BTreeSet<Key>,
    #[serde(default)]
    pub effects: Vec<StatusEffect>,
    pub classes: Vec<Class>,
    #[serde(default)]
    pub abilities: Vec<Ability>,
    /// Total experience needed to reach level 2, then 3, and so on. Its length sets the
    /// highest level.
    pub levels: Vec<u64>,
    /// The resource whose loss is death.
    pub life: Key,
    /// How many characters travel together at most.
    pub party_size: usize,
    /// `derive(character)` returns every derived stat of a character.
    pub derive: ScriptName,
    /// `check(character, skill, difficulty, roll)` says whether a check passes.
    pub check: ScriptName,
}
/// What a consumable does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Use {
    Restore {
        resource: Key,
        amount: u32,
    },
    Apply {
        effect: Key,
        /// Without a duration the effect lasts until something removes it.
        #[serde(default)]
        duration_ms: Option<u64>,
    },
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemMechanics {
    pub slot: Option<Key>,
    pub modifiers: Vec<Modifier>,
    pub on_use: Vec<Use>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveEffect {
    pub effect: Key,
    pub expires_at: Option<GameTime>,
    /// When its periodic change next happens.
    pub next_tick: Option<GameTime>,
}
pub type Stats = BTreeMap<Key, i32>;
pub type SkillRanks = BTreeMap<Key, u8>;

/// What the rules script is told about a character.
#[derive(Debug, Clone, Copy)]
pub struct Sheet<'a> {
    pub level: u32,
    pub class: &'a Key,
    pub stats: &'a Stats,
    pub skills: &'a SkillRanks,
}

fn find<'a, T>(list: &'a [T], id: &Key, of: impl Fn(&T) -> &Key, what: &str) -> Result<&'a T> {
    list.iter()
        .find(|v| of(v) == id)
        .ok_or_else(|| Invalid(format!("unknown {what} {}", id.as_str())))
}
fn unique<'a>(ids: impl Iterator<Item = &'a Key>, what: &str) -> Result<()> {
    let mut seen = BTreeSet::new();
    for id in ids {
        require(
            seen.insert(id),
            &format!("duplicate {what} {}", id.as_str()),
        )?;
    }
    Ok(())
}
impl Rules {
    pub fn stat(&self, id: &Key) -> Result<&Stat> {
        find(&self.stats, id, |v| &v.id, "stat")
    }
    pub fn skill(&self, id: &Key) -> Result<&Skill> {
        find(&self.skills, id, |v| &v.id, "skill")
    }
    pub fn effect(&self, id: &Key) -> Result<&StatusEffect> {
        find(&self.effects, id, |v| &v.id, "effect")
    }
    pub fn class(&self, id: &Key) -> Result<&Class> {
        find(&self.classes, id, |v| &v.id, "class")
    }
    pub fn ability(&self, id: &Key) -> Result<&Ability> {
        find(&self.abilities, id, |v| &v.id, "ability")
    }
    /// Every name the rules show to players.
    pub fn names(&self) -> impl Iterator<Item = &TextRef> {
        self.stats
            .iter()
            .map(|v| &v.name)
            .chain(self.skills.iter().map(|v| &v.name))
            .chain(self.effects.iter().map(|v| &v.name))
            .chain(self.classes.iter().map(|v| &v.name))
            .chain(self.abilities.iter().map(|v| &v.name))
    }
    /// The resource and the stat that caps it.
    pub fn resource(&self, id: &Key) -> Result<&Key> {
        match &self.stat(id)?.kind {
            StatKind::Resource { maximum } => Ok(maximum),
            _ => Err(Invalid(format!("{} is not a resource", id.as_str()))),
        }
    }
    /// A primary stat's bounds.
    pub fn primary(&self, id: &Key) -> Result<(i32, i32)> {
        match self.stat(id)?.kind {
            StatKind::Primary { minimum, maximum } => Ok((minimum, maximum)),
            _ => Err(Invalid(format!("{} is not a primary stat", id.as_str()))),
        }
    }
    pub fn primaries(&self) -> impl Iterator<Item = (&Key, i32, i32)> {
        self.stats.iter().filter_map(|s| match s.kind {
            StatKind::Primary { minimum, maximum } => Some((&s.id, minimum, maximum)),
            _ => None,
        })
    }
    pub fn resources(&self) -> impl Iterator<Item = (&Key, &Key)> {
        self.stats.iter().filter_map(|s| match &s.kind {
            StatKind::Resource { maximum } => Some((&s.id, maximum)),
            _ => None,
        })
    }
    pub fn max_level(&self) -> u32 {
        self.levels.len() as u32 + 1
    }
    /// The level a total of experience has earned.
    pub fn level_for(&self, experience: u64) -> u32 {
        1 + self.levels.partition_point(|needed| *needed <= experience) as u32
    }
    /// What a class receives on reaching a level.
    pub fn grants<'a>(&self, class: &'a Class, level: u32) -> impl Iterator<Item = &'a Grants> {
        (level > 1)
            .then_some(&class.per_level)
            .into_iter()
            .chain(class.at_level.get(&level))
    }
    fn validate_modifier(&self, modifier: &Modifier) -> Result<()> {
        require(
            !matches!(self.stat(&modifier.stat)?.kind, StatKind::Resource { .. }),
            "a modifier changes a primary or derived stat, not a resource",
        )?;
        match modifier.op {
            Operation::Multiply(percent) => {
                require((0..=1000).contains(&percent), "multiplier outside 0..1000%")
            }
            _ => Ok(()),
        }
    }
    fn validate_grants(&self, grants: &Grants) -> Result<()> {
        require(
            grants.attribute_points <= 100 && grants.learning_points <= 100,
            "too many points in one grant",
        )?;
        for stat in grants.bonuses.keys() {
            self.primary(stat)?;
        }
        for ability in &grants.abilities {
            self.ability(ability)?;
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        require(
            !self.stats.is_empty() && !self.classes.is_empty(),
            "rules need stats and classes",
        )?;
        require(
            (1..=16).contains(&self.party_size),
            "a party has 1..16 members",
        )?;
        unique(self.stats.iter().map(|v| &v.id), "stat")?;
        unique(self.skills.iter().map(|v| &v.id), "skill")?;
        unique(self.effects.iter().map(|v| &v.id), "effect")?;
        unique(self.classes.iter().map(|v| &v.id), "class")?;
        unique(self.abilities.iter().map(|v| &v.id), "ability")?;
        for stat in &self.stats {
            stat.name.validate()?;
            match &stat.kind {
                StatKind::Primary { minimum, maximum } | StatKind::Derived { minimum, maximum } => {
                    require(minimum <= maximum, "stat minimum above its maximum")?
                }
                StatKind::Resource { maximum } => require(
                    !matches!(self.stat(maximum)?.kind, StatKind::Resource { .. }),
                    "a resource is capped by a primary or derived stat",
                )?,
            }
        }
        self.resource(&self.life)?;
        for skill in &self.skills {
            skill.name.validate()?;
            // Ranks are counted in a byte.
            require(
                (1..=usize::from(u8::MAX)).contains(&skill.ranks.len()),
                "a skill has 1..255 ranks",
            )?;
        }
        for effect in &self.effects {
            effect.name.validate()?;
            for modifier in &effect.modifiers {
                self.validate_modifier(modifier)?;
            }
            if let Some(periodic) = &effect.periodic {
                self.resource(&periodic.resource)?;
                require(
                    periodic.period_ms > 0 && periodic.amount != 0,
                    "a periodic change needs a period and an amount",
                )?;
            }
        }
        for ability in &self.abilities {
            ability.name.validate()?;
            // Taking no time at all, a repeating ability would go on forever at one instant.
            require(
                (1..=3_600_000).contains(&ability.duration_ms) && ability.cooldown_ms <= 86_400_000,
                "an ability takes 1 ms to an hour, and cools down within a day",
            )?;
            for resource in ability.costs.keys() {
                self.resource(resource)?;
            }
        }
        require(
            self.levels.windows(2).all(|pair| pair[0] < pair[1]),
            "experience thresholds must increase",
        )?;
        for class in &self.classes {
            class.name.validate()?;
            require(
                class.starting.len() == self.primaries().count(),
                "a class gives a starting value for every primary stat",
            )?;
            for (stat, value) in &class.starting {
                let (minimum, maximum) = self.primary(stat)?;
                require(
                    (minimum..=maximum).contains(value),
                    "class starting stat outside its bounds",
                )?;
            }
            self.validate_grants(&class.per_level)?;
            for (level, grants) in &class.at_level {
                require(
                    (1..=self.max_level()).contains(level),
                    "class grant for a level that cannot be reached",
                )?;
                self.validate_grants(grants)?;
            }
            for (skill, rank) in &class.skills {
                require(
                    usize::from(*rank) <= self.skill(skill)?.ranks.len(),
                    "class skill cap above the skill's ranks",
                )?;
            }
        }
        Ok(())
    }
    pub fn validate_mechanics(&self, mechanics: &ItemMechanics) -> Result<()> {
        mechanics.validate()?;
        if let Some(slot) = &mechanics.slot {
            require(self.slots.contains(slot), "unknown equipment slot")?;
        }
        for modifier in &mechanics.modifiers {
            self.validate_modifier(modifier)?;
        }
        for effect in &mechanics.on_use {
            match effect {
                Use::Restore { resource, .. } => {
                    self.resource(resource)?;
                }
                Use::Apply { effect, .. } => {
                    self.effect(effect)?;
                }
            }
        }
        Ok(())
    }
    /// Primary stats after modifiers, within their bounds.
    pub fn effective_primaries<'a>(
        &self,
        base: &Stats,
        modifiers: impl Iterator<Item = &'a Modifier> + Clone,
    ) -> Result<Stats> {
        self.primaries()
            .map(|(id, minimum, maximum)| {
                let base = base
                    .get(id)
                    .ok_or_else(|| Invalid(format!("no base value for {}", id.as_str())))?;
                let operations = modifiers.clone().filter(|m| &m.stat == id).map(|m| m.op);
                let value = modified(i64::from(*base), operations);
                Ok((
                    id.clone(),
                    value.clamp(i64::from(minimum), i64::from(maximum)) as i32,
                ))
            })
            .collect()
    }
    /// Adds the derived stats the script returned, after modifiers, within their bounds.
    pub fn add_derived<'a>(
        &self,
        stats: &mut Stats,
        derived: &BTreeMap<String, f64>,
        modifiers: impl Iterator<Item = &'a Modifier> + Clone,
    ) -> Result<()> {
        let mut expected = 0;
        for stat in &self.stats {
            let StatKind::Derived { minimum, maximum } = stat.kind else {
                continue;
            };
            expected += 1;
            let raw = derived.get(stat.id.as_str()).ok_or_else(|| {
                Invalid(format!(
                    "{} did not return {}",
                    self.derive,
                    stat.id.as_str()
                ))
            })?;
            require(
                raw.is_finite() && raw.abs() < 1e12,
                &format!("{} returned a bad {}", self.derive, stat.id.as_str()),
            )?;
            let operations = modifiers
                .clone()
                .filter(|m| m.stat == stat.id)
                .map(|m| m.op);
            let value = modified(raw.floor() as i64, operations);
            stats.insert(
                stat.id.clone(),
                value.clamp(i64::from(minimum), i64::from(maximum)) as i32,
            );
        }
        require(
            derived.len() == expected,
            &format!(
                "{} returned something that is not a derived stat",
                self.derive
            ),
        )
    }
}
impl ItemMechanics {
    pub fn validate(&self) -> Result<()> {
        require(
            self.slot.is_some() || self.modifiers.is_empty(),
            "equipment modifiers need a slot",
        )?;
        for effect in &self.on_use {
            match effect {
                Use::Restore { amount, .. } => require(*amount > 0, "restoring nothing")?,
                Use::Apply { duration_ms, .. } => require(
                    duration_ms.is_none_or(|ms| ms > 0),
                    "effect duration must be positive",
                )?,
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
    /// One of 1..=sides.
    pub fn die(&mut self, sides: u32) -> Result<u32> {
        Ok(self.below(sides)? + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modifiers_combine_the_same_in_any_order() {
        use Operation::*;
        assert_eq!(modified(10, []), 10);
        assert_eq!(modified(10, [Add(2), Add(-5)]), 7);
        assert_eq!(modified(10, [Multiply(150), Add(2)]), 18);
        assert_eq!(modified(10, [Add(2), Multiply(150)]), 18);
        // Two +50% make +100%, not +125%.
        assert_eq!(modified(10, [Multiply(150), Multiply(150)]), 20);
        assert_eq!(modified(10, [Multiply(50), Multiply(25)]), 0);
        assert_eq!(modified(7, [Multiply(50)]), 3);
        assert_eq!(modified(10, [Override(19), Add(5), Override(12)]), 19);
    }
}
