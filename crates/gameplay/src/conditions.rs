//! Conditions content puts on lines, entry rules and triggers: what they may name, and
//! evaluating them against a playthrough without changing it.
use crate::{
    GameContent, ReadScope, Result, ScriptName, SessionState, Value, VariableKey, actors, dialogue,
    quests,
};

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
pub enum Test {
    Is(Value),
    AtLeast(i64),
    AtMost(i64),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    /// A script function returning a boolean.
    Script(ScriptName),
    /// The actor takes part in the conversation or travels with the party.
    Present(ActorId),
    InsideArea {
        area: AreaId,
    },
    History {
        dialogue: DialogueId,
        event: dialogue::HistoryEvent,
        minimum: u64,
    },
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    Named(PredicateId),
    QuestStatus {
        quest: QuestId,
        status: quests::Status,
    },
    ObjectiveCompleted {
        quest: QuestId,
        objective: Key,
    },
    Relationship {
        from: Participant,
        to: Participant,
        minimum: i16,
    },
    HasItem {
        definition: ItemDefinitionId,
        quantity: u32,
    },
    Variable {
        variable: VariableId,
        /// Whose value, for a per-actor variable.
        #[serde(default)]
        of: Option<Participant>,
        test: Test,
    },
    /// A stat of the player's after equipment and effects, or how much of a resource is
    /// left.
    Stat {
        stat: Key,
        minimum: i32,
    },
    /// The player's rank in a skill.
    Skill {
        skill: Key,
        minimum: u8,
    },
    Level {
        minimum: u32,
    },
    Class(Key),
    /// A trainer could teach the player the next rank: the class allows it and the
    /// learning points are there.
    CanLearn {
        skill: Key,
    },
    /// The party's shared purse holds at least this much.
    Gold {
        minimum: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedPredicate {
    pub id: PredicateId,
    pub condition: Condition,
}

/// How deep a condition may nest and how many conditions it may take, named predicates
/// expanded, so that checking or evaluating one cannot exhaust the stack or run long.
const MAX_CONDITION_DEPTH: usize = 32;

const MAX_CONDITION_STEPS: usize = 1024;

impl Condition {
    /// Visits the tree with named predicates expanded, within the limits above. Groups are
    /// never empty, and a named predicate never names itself.
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
            steps: &mut usize,
        ) -> Result<()> {
            require(
                depth <= MAX_CONDITION_DEPTH && *steps > 0,
                "condition complexity exceeded",
            )?;
            *steps -= 1;
            visitor(c)?;

            match c {
                Condition::All(children) | Condition::Any(children) => {
                    require(!children.is_empty(), "empty condition group")?;

                    for child in children {
                        walk(child, resolve, visitor, active, depth + 1, steps)?;
                    }
                }
                Condition::Not(child) => walk(child, resolve, visitor, active, depth + 1, steps)?,
                Condition::Named(id) => {
                    require(active.insert(*id), "named predicate cycle")?;
                    walk(resolve(*id)?, resolve, visitor, active, depth + 1, steps)?;
                    active.remove(id);
                }
                _ => {}
            }

            Ok(())
        }

        let mut steps = MAX_CONDITION_STEPS;
        walk(self, resolve, visitor, &mut BTreeSet::new(), 0, &mut steps)
    }
}

/// One evaluation of a condition: whom it is about, and how many steps it may still take.
struct Evaluation<'a> {
    content: &'a GameContent,
    state: &'a SessionState,
    player: ActorId,
    speaker: ActorId,
    /// Further actors taking part, beyond the pair and the party.
    others: &'a BTreeSet<ActorId>,
    steps: usize,
}

impl Evaluation<'_> {
    /// Groups stop at the first child that decides them.
    fn holds(&mut self, condition: &Condition, depth: usize) -> Result<bool> {
        require(
            depth <= MAX_CONDITION_DEPTH && self.steps > 0,
            "condition complexity exceeded",
        )?;
        self.steps -= 1;
        let (content, state, player, speaker) =
            (self.content, self.state, self.player, self.speaker);

        // What a script reads, and who counts as present, as conditions see it.
        let scope = ReadScope {
            content,
            state,
            player,
            speaker,
            others: self.others,
        };

        Ok(match condition {
            Condition::Script(name) => content.scripts.engine(name)?.condition(name, &scope)?,
            Condition::Present(actor) => scope.present(*actor),
            Condition::InsideArea { area } => state.areas(player).contains(area),
            Condition::History {
                dialogue,
                event,
                minimum,
            } => {
                let contract = content.dialogue_contract(*dialogue)?;
                let history = state.history(contract.history_key(player, speaker));
                history.count(event) >= *minimum
            }
            Condition::All(children) => {
                for child in children {
                    if !self.holds(child, depth + 1)? {
                        return Ok(false);
                    }
                }

                true
            }
            Condition::Any(children) => {
                for child in children {
                    if self.holds(child, depth + 1)? {
                        return Ok(true);
                    }
                }

                false
            }
            Condition::Not(child) => !self.holds(child, depth + 1)?,
            Condition::Named(id) => self.holds(content.predicate(*id)?, depth + 1)?,
            Condition::HasItem {
                definition,
                quantity,
            } => state.item_count(content, player, *definition)? >= u64::from(*quantity),
            Condition::Variable { variable, of, test } => {
                let key = VariableKey {
                    variable: *variable,
                    actor: of.map(|p| p.resolve(player, speaker)),
                };

                match (test, &state.variable(content, key)?) {
                    (Test::Is(expected), value) => expected == value,
                    (Test::AtLeast(n), Value::Int(value)) => value >= n,
                    (Test::AtMost(n), Value::Int(value)) => value <= n,
                    _ => false,
                }
            }
            Condition::Stat { stat, minimum } => state.stat(content, player, stat)? >= *minimum,
            Condition::Skill { skill, minimum } => state.actor(player)?.skill(skill) >= *minimum,
            Condition::Level { minimum } => state.actor(player)?.level >= *minimum,
            Condition::Class(class) => &state.actor(player)?.class == class,
            Condition::CanLearn { skill } => {
                crate::teachable(&content.game.rules, state.actor(player)?, skill)?.is_ok()
            }
            Condition::Gold { minimum } => state.gold()? >= *minimum,
            Condition::QuestStatus { quest, status } => state.quest(*quest).status == *status,
            Condition::ObjectiveCompleted { quest, objective } => {
                state.quest(*quest).completed.contains(objective)
            }
            Condition::Relationship { from, to, minimum } => {
                let key = actors::RelationshipKey {
                    from: from.resolve(player, speaker),
                    to: to.resolve(player, speaker),
                };
                state.relationship(key).attitude >= *minimum
            }
        })
    }
}

impl GameContent {
    pub fn predicate(&self, id: PredicateId) -> Result<&Condition> {
        self.game
            .predicates
            .get(&id)
            .map(|p| &p.condition)
            .ok_or_else(|| Invalid(format!("unknown named predicate {id}")).into())
    }

    pub fn validate_condition(&self, condition: &Condition) -> Result<()> {
        condition.visit_resolved(&|id| self.predicate(id), &mut |c| {
            match c {
                Condition::Present(actor) => {
                    self.character(*actor)?;
                }
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

                Condition::HasItem {
                    definition,
                    quantity,
                } => {
                    self.items.item(*definition)?;
                    require(*quantity > 0, "zero item condition")?;
                }
                Condition::Variable { variable, of, test } => {
                    if let Some(of) = of {
                        self.validate_participant(of)?;
                    }

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
                Condition::Relationship { from, to, minimum } => {
                    self.validate_participant(from)?;
                    self.validate_participant(to)?;
                    require(
                        from != to && (-100..=100).contains(minimum),
                        "invalid relationship condition",
                    )?
                }
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
    ) -> Result<bool> {
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
    ) -> Result<bool> {
        let mut evaluation = Evaluation {
            content: self,
            state,
            player,
            speaker,
            others,
            steps: MAX_CONDITION_STEPS,
        };
        evaluation.holds(condition, 0)
    }

    /// A participant naming a character names a declared one.
    pub(crate) fn validate_participant(&self, participant: &Participant) -> Result<()> {
        if let Participant::Actor(id) = participant {
            self.character(*id)?;
        }
        Ok(())
    }
}
