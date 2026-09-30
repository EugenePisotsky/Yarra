//! Reusable predicates and NPC entry profiles. All evaluation uses an already-resolved snapshot.
use crate::Result;
use crate::*;
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Participant {
    #[default]
    Player,
    Speaker,
    /// A named character, whoever is talking.
    Actor(ActorId),
}
impl Participant {
    pub fn resolve(self, player: ActorId, speaker: ActorId) -> ActorId {
        match self {
            Self::Player => player,
            Self::Speaker => speaker,
            Self::Actor(actor) => actor,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedPredicate {
    pub id: PredicateId,
    pub condition: Condition,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueVariant {
    pub dialogue: DialogueId,
    pub weight: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryRule {
    pub id: Key,
    pub priority: i32,
    /// Explicit tie order within a priority tier. Duplicate (priority, order) pairs are errors.
    pub order: u16,
    /// Topics are independently discoverable; opening selection considers non-topic rules.
    pub topic: Option<TextRef>,
    pub condition: Condition,
    pub variants: Vec<DialogueVariant>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProfile {
    pub id: InteractionProfileId,
    pub rules: Vec<EntryRule>,
}
impl InteractionProfile {
    pub fn validate(&self) -> Result<()> {
        require(
            !self.rules.is_empty() && self.rules.len() <= 64,
            "profile requires 1..64 rules",
        )?;
        let mut ids = BTreeSet::new();
        let mut precedence = BTreeSet::new();
        for rule in &self.rules {
            require(
                ids.insert(&rule.id) && precedence.insert((rule.priority, rule.order)),
                "duplicate rule or ambiguous interaction precedence",
            )?;
            if let Some(text) = &rule.topic {
                text.validate()?;
            }
            require(
                !rule.variants.is_empty() && rule.variants.len() <= 16,
                "rule requires 1..16 variants",
            )?;
            let mut dialogues = BTreeSet::new();
            let mut weight = 0u32;
            for variant in &rule.variants {
                require(
                    variant.weight > 0 && dialogues.insert(variant.dialogue),
                    "duplicate or zero-weight variant",
                )?;
                weight = weight
                    .checked_add(variant.weight)
                    .ok_or_else(|| Invalid("variant weight overflow".into()))?;
            }
            rule.condition.visit(&mut |_| Ok(()))?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observed {
    Boolean(bool),
    Quantity(u64),
    Attitude(i16),
    Quest(quests::Status),
    Value(Value),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionCheck {
    pub predicate: Condition,
    pub observed: Observed,
    pub matched: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub matched: bool,
    pub checks: Vec<ConditionCheck>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPreview {
    pub rule: Key,
    pub priority: i32,
    pub order: u16,
    pub topic: Option<TextRef>,
    pub evaluation: Evaluation,
    /// Variants excluded by their scoped repeat/cooldown policy.
    pub unavailable_variants: Vec<DialogueId>,
    pub variants: Vec<DialogueVariant>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionPreview {
    pub profile: InteractionProfileId,
    pub candidates: Vec<EntryPreview>,
}
impl InteractionPreview {
    pub fn opening(&self) -> Option<&EntryPreview> {
        self.candidates
            .iter()
            .find(|r| r.topic.is_none() && r.evaluation.matched)
    }
    pub fn topics(&self) -> impl Iterator<Item = &EntryPreview> {
        self.candidates
            .iter()
            .filter(|r| r.topic.is_some() && r.evaluation.matched)
    }
    pub(crate) fn selected(&self, topic: Option<&Key>) -> Result<&EntryPreview> {
        match topic {
            None => self.opening(),
            Some(id) => self.topics().find(|r| &r.rule == id),
        }
        .ok_or_else(|| Invalid("no eligible conversation entry".into()).into())
    }
}
impl Condition {
    /// Visit a bounded authored tree. Named references are reported; the content source loads them.
    pub fn visit(&self, visitor: &mut impl FnMut(&Condition) -> Result<()>) -> Result<()> {
        fn walk(
            c: &Condition,
            depth: usize,
            budget: &mut usize,
            f: &mut impl FnMut(&Condition) -> Result<()>,
        ) -> Result<()> {
            require(depth <= 16 && *budget > 0, "condition complexity exceeded")?;
            *budget -= 1;
            f(c)?;
            match c {
                Condition::All(v) | Condition::Any(v) => {
                    require(!v.is_empty(), "empty condition group")?;
                    for c in v {
                        walk(c, depth + 1, budget, f)?;
                    }
                }
                Condition::Not(c) => walk(c, depth + 1, budget, f)?,
                _ => {}
            }
            Ok(())
        }
        walk(self, 0, &mut 256, visitor)
    }
    /// Shared traversal for publication and runtime validation. Named expansions use the
    /// same total depth/work budget as evaluation, while each authored tree stays bounded.
    pub fn visit_resolved<'a>(
        &'a self,
        resolve: &impl Fn(PredicateId) -> Result<&'a Condition>,
        visitor: &mut impl FnMut(&Condition) -> Result<()>,
    ) -> Result<()> {
        fn walk<'a>(
            c: &'a Condition,
            resolve: &impl Fn(PredicateId) -> Result<&'a Condition>,
            visitor: &mut impl FnMut(&Condition) -> Result<()>,
            active: &mut BTreeSet<PredicateId>,
            depth: usize,
            budget: &mut usize,
        ) -> Result<()> {
            require(
                depth <= 32 && *budget > 0,
                "resolved predicate complexity exceeded",
            )?;
            *budget -= 1;
            visitor(c)?;
            match c {
                Condition::All(children) | Condition::Any(children) => {
                    for child in children {
                        walk(child, resolve, visitor, active, depth + 1, budget)?;
                    }
                }
                Condition::Not(child) => walk(child, resolve, visitor, active, depth + 1, budget)?,
                Condition::Named(id) => {
                    require(active.insert(*id), "named predicate cycle")?;
                    let child = resolve(*id)?;
                    child.visit(&mut |_| Ok(()))?;
                    walk(child, resolve, visitor, active, depth + 1, budget)?;
                    active.remove(id);
                }
                _ => {}
            }
            Ok(())
        }
        self.visit(&mut |_| Ok(()))?;
        walk(self, resolve, visitor, &mut BTreeSet::new(), 0, &mut 1024)
    }
}
impl Action {
    pub fn visit(&self, visitor: &mut impl FnMut(&Action) -> Result<()>) -> Result<()> {
        fn walk(
            a: &Action,
            depth: usize,
            budget: &mut usize,
            f: &mut impl FnMut(&Action) -> Result<()>,
        ) -> Result<()> {
            require(depth <= 8 && *budget > 0, "action complexity exceeded")?;
            *budget -= 1;
            f(a)?;
            match a {
                Action::Check {
                    success, failure, ..
                } => {
                    for a in success.iter().chain(failure) {
                        walk(a, depth + 1, budget, f)?;
                    }
                }
                Action::Claim { actions, .. } => {
                    for a in actions {
                        walk(a, depth + 1, budget, f)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        walk(self, 0, &mut 1024, visitor)
    }
}
impl GameContent {
    pub fn quest(&self, id: QuestId) -> Result<&quests::Quest> {
        self.game
            .quests
            .iter()
            .find(|q| q.id == id)
            .ok_or_else(|| Invalid("unknown quest".into()).into())
    }
    pub fn profile(&self, id: InteractionProfileId) -> Result<&InteractionProfile> {
        self.game
            .profiles
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| Invalid("unknown interaction profile".into()).into())
    }
    pub fn predicate(&self, id: PredicateId) -> Result<&Condition> {
        self.game
            .predicates
            .iter()
            .find(|p| p.id == id)
            .map(|p| &p.condition)
            .ok_or_else(|| Invalid("unknown named predicate".into()).into())
    }
    pub fn validate_condition(&self, condition: &Condition) -> Result<()> {
        condition.visit_resolved(&|id| self.predicate(id), &mut |c| {
            match c {
                Condition::Script(name) => {
                    self.scripts.engine(name)?;
                }
                Condition::InsideArea { area } => {
                    self.area(*area)?;
                }
                Condition::History {
                    dialogue,
                    event,
                    minimum,
                } => {
                    require(*minimum > 0, "zero history threshold")?;
                    let d = self.dialogue_contract(*dialogue)?;
                    if let dialogue::HistoryEvent::Node(id) = event {
                        require(d.nodes.contains(id), "unknown history node")?
                    }
                }
                Condition::Claimed { claim, .. } => {
                    self.claim(*claim)?;
                }

                Condition::HasItem {
                    definition,
                    quantity,
                } => {
                    self.items.item(*definition)?;
                    require(*quantity > 0, "zero item condition")?;
                }
                Condition::Variable { variable, of, test } => {
                    let initial = &self.variable_use(*variable, of)?.initial;
                    let fits = match test {
                        Test::Is(value) => initial.same_type(value),
                        Test::AtLeast(_) | Test::AtMost(_) => matches!(initial, Value::Int(_)),
                    };
                    require(fits, "test does not fit the variable's type")?
                }
                Condition::Stat { stat, .. } => {
                    self.game.rules.stat(stat)?;
                }
                Condition::Skill { skill, .. } | Condition::CanLearn { skill } => {
                    self.game.rules.skill(skill)?;
                }
                Condition::Level { minimum } => require(
                    (1..=self.game.rules.max_level()).contains(minimum),
                    "level condition cannot be met",
                )?,
                Condition::Class(class) => {
                    self.game.rules.class(class)?;
                }
                Condition::QuestStatus { quest, .. } => {
                    self.quest(*quest)?;
                }
                Condition::ObjectiveCompleted { quest, objective } => {
                    self.quest(*quest)?.objective(objective)?;
                }
                Condition::Relationship { from, to, minimum } => require(
                    from != to && (-100..=100).contains(minimum),
                    "invalid relationship condition",
                )?,
                _ => {}
            }
            Ok(())
        })
    }
    pub fn evaluate(
        &self,
        condition: &Condition,
        state: &SessionState,
        player: ActorId,
        speaker: ActorId,
    ) -> Result<Evaluation> {
        self.evaluate_among(condition, state, player, speaker, &BTreeSet::new())
    }
    /// `others` are further actors taking part, beyond the pair and the party.
    pub fn evaluate_among(
        &self,
        condition: &Condition,
        state: &SessionState,
        player: ActorId,
        speaker: ActorId,
        others: &BTreeSet<ActorId>,
    ) -> Result<Evaluation> {
        fn eval(
            content: &GameContent,
            c: &Condition,
            state: &SessionState,
            pair: (ActorId, ActorId, &BTreeSet<ActorId>),
            checks: &mut Vec<ConditionCheck>,
            budget: &mut usize,
            depth: usize,
        ) -> Result<bool> {
            require(
                depth <= 32 && *budget > 0,
                "predicate evaluation budget exceeded",
            )?;
            *budget -= 1;
            let (observed, matched) = match c {
                Condition::Script(name) => {
                    let matched = content.scripts.engine(name)?.condition(
                        name,
                        &ReadScope {
                            content,
                            state,
                            player: pair.0,
                            speaker: pair.1,
                            others: pair.2,
                        },
                    )?;
                    (Observed::Boolean(matched), matched)
                }
                Condition::Present(actor) => {
                    let present = *actor == pair.0
                        || *actor == pair.1
                        || pair.2.contains(actor)
                        || state.party.members.contains(actor);
                    (Observed::Boolean(present), present)
                }
                Condition::InsideArea { area } => {
                    let inside = state.areas(pair.0).contains(area);
                    (Observed::Boolean(inside), inside)
                }
                Condition::History {
                    dialogue,
                    event,
                    minimum,
                } => {
                    let d = content.dialogue_contract(*dialogue)?;
                    let n = state.history(d.history_key(pair.0, pair.1)).count(event);
                    (Observed::Quantity(n), n >= *minimum)
                }
                Condition::Claimed { claim, value } => {
                    let d = content.claim(*claim)?;
                    let found = state.claimed(d.key(pair.0, pair.1));
                    (Observed::Boolean(found), found == *value)
                }
                Condition::All(v) | Condition::Any(v) => {
                    let mut result = matches!(c, Condition::All(_));
                    for child in v {
                        let value = eval(content, child, state, pair, checks, budget, depth + 1)?;
                        if matches!(c, Condition::All(_)) {
                            result &= value;
                        } else {
                            result |= value;
                        }
                    }
                    return Ok(result);
                }
                Condition::Not(v) => {
                    return Ok(!eval(content, v, state, pair, checks, budget, depth + 1)?);
                }
                Condition::Named(id) => {
                    return eval(
                        content,
                        content.predicate(*id)?,
                        state,
                        pair,
                        checks,
                        budget,
                        depth + 1,
                    );
                }
                Condition::HasItem {
                    definition,
                    quantity,
                } => {
                    let n = state.item_count(content, pair.0, *definition)?;
                    (Observed::Quantity(n), n >= u64::from(*quantity))
                }
                Condition::Variable { variable, of, test } => {
                    let value = state.variable(
                        content,
                        VariableKey {
                            variable: *variable,
                            actor: of.map(|p| p.resolve(pair.0, pair.1)),
                        },
                    )?;
                    let matched = match (test, &value) {
                        (Test::Is(expected), value) => expected == value,
                        (Test::AtLeast(n), Value::Int(value)) => value >= n,
                        (Test::AtMost(n), Value::Int(value)) => value <= n,
                        _ => false,
                    };
                    (Observed::Value(value), matched)
                }
                Condition::Stat { stat, minimum } => {
                    let n = state.stat(content, pair.0, stat)?;
                    (Observed::Value(Value::Int(i64::from(n))), n >= *minimum)
                }
                Condition::Skill { skill, minimum } => {
                    let n = state.actor(pair.0)?.skill(skill);
                    (Observed::Quantity(u64::from(n)), n >= *minimum)
                }
                Condition::Level { minimum } => {
                    let n = state.actor(pair.0)?.level;
                    (Observed::Quantity(u64::from(n)), n >= *minimum)
                }
                Condition::Class(class) => {
                    let matched = &state.actor(pair.0)?.class == class;
                    (Observed::Boolean(matched), matched)
                }
                Condition::CanLearn { skill } => {
                    let can =
                        crate::teachable(&content.game.rules, state.actor(pair.0)?, skill)?.is_ok();
                    (Observed::Boolean(can), can)
                }
                Condition::Gold { minimum } => {
                    let n = state.gold()?;
                    (Observed::Quantity(n), n >= *minimum)
                }
                Condition::QuestStatus { quest, status } => {
                    let actual = state.quest(*quest).status;
                    (Observed::Quest(actual), actual == *status)
                }
                Condition::ObjectiveCompleted { quest, objective } => {
                    let found = state.quest(*quest).completed.contains(objective);
                    (Observed::Boolean(found), found)
                }
                Condition::Relationship { from, to, minimum } => {
                    let value = state
                        .relationship(actors::RelationshipKey {
                            from: from.resolve(pair.0, pair.1),
                            to: to.resolve(pair.0, pair.1),
                        })
                        .attitude;
                    (Observed::Attitude(value), value >= *minimum)
                }
            };
            checks.push(ConditionCheck {
                predicate: c.clone(),
                observed,
                matched,
            });
            Ok(matched)
        }
        let mut checks = Vec::new();
        let matched = eval(
            self,
            condition,
            state,
            (player, speaker, others),
            &mut checks,
            &mut 1024,
            0,
        )?;
        Ok(Evaluation { matched, checks })
    }
    pub fn preview(
        &self,
        profile: InteractionProfileId,
        state: &SessionState,
        player: ActorId,
        speaker: ActorId,
    ) -> Result<InteractionPreview> {
        let mut candidates = Vec::new();
        for rule in &self.profile(profile)?.rules {
            let mut variants = Vec::new();
            let mut unavailable_variants = Vec::new();
            for variant in &rule.variants {
                let contract = self.dialogue_contract(variant.dialogue)?;
                if contract.repeat.eligible(
                    &state.history(contract.history_key(player, speaker)),
                    state.time,
                ) {
                    variants.push(variant.clone());
                } else {
                    unavailable_variants.push(variant.dialogue);
                }
            }
            let mut evaluation = self.evaluate(&rule.condition, state, player, speaker)?;
            evaluation.matched &= !variants.is_empty();
            candidates.push(EntryPreview {
                rule: rule.id.clone(),
                priority: rule.priority,
                order: rule.order,
                topic: rule.topic.clone(),
                evaluation,
                variants,
                unavailable_variants,
            });
        }
        candidates.sort_by_key(|c| (std::cmp::Reverse(c.priority), c.order));
        Ok(InteractionPreview {
            profile,
            candidates,
        })
    }
    /// Links used for selection are validated at publication, but are deliberately not eager
    /// runtime dependencies: inspecting an NPC must not load every potential conversation.
    pub fn validate_selection_links(&self) -> Result<()> {
        for actor in &self.game.actors {
            if let Some(id) = actor.interaction {
                self.profile(id)?;
            }
        }
        for profile in &self.game.profiles {
            for rule in &profile.rules {
                for variant in &rule.variants {
                    self.dialogue(variant.dialogue)?;
                }
            }
        }
        Ok(())
    }
}
