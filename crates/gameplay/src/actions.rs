//! Actions content runs when a line is chosen, a rule is checked or a trigger fires: what
//! they may name, and carrying them out within a command.
use crate::{
    ActScope, Condition, GameContent, GameEvent, Pending, Rejection, Result, ScriptName, Value,
    VariableKey, WorldEvent, actors, character, character::sync_equipment, conditions::Participant,
    quests, tx::Tx,
};

use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// A script function making changes through the script API.
    Script(ScriptName),
    SetLocked {
        object: ObjectId,
        locked: bool,
    },
    /// `then` when the condition holds, otherwise `otherwise`. With a variable that the
    /// actions set, it gives a reward once however often the choice is offered.
    If {
        condition: Condition,
        then: Vec<Action>,
        #[serde(default)]
        otherwise: Vec<Action>,
    },
    Quest {
        quest: QuestId,
        transition: quests::Transition,
    },
    Relationship {
        from: Participant,
        to: Participant,
        amount: i16,
    },
    ConsumeItem {
        definition: ItemDefinitionId,
        quantity: u32,
    },
    GrantItem {
        definition: ItemDefinitionId,
        quantity: u32,
    },
    /// Experience for the whole party; members gain the levels it earns.
    AwardExperience {
        amount: u64,
    },
    /// A trainer teaches the player the next rank of a skill for learning points.
    Teach {
        skill: Key,
    },
    /// Gold leaves the party's shared purse.
    Pay {
        amount: u64,
    },
    /// Adds to a resource, or takes from it with a negative amount: healing, a trap.
    ChangeResource {
        #[serde(default)]
        of: Participant,
        resource: Key,
        amount: i32,
    },
    ApplyEffect {
        #[serde(default)]
        of: Participant,
        effect: Key,
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    RemoveEffect {
        #[serde(default)]
        of: Participant,
        effect: Key,
    },
    /// Asks the engine to walk an actor into an area. `Arrived` or `MoveFailed` follows.
    Move {
        actor: Participant,
        to: AreaId,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Starts a conversation with the player once the current command is done. Without a
    /// speaker named, it is the speaker of the rule this action belongs to.
    StartDialogue {
        dialogue: DialogueId,
        #[serde(default)]
        speaker: Option<Participant>,
    },
    Set {
        variable: VariableId,
        #[serde(default)]
        of: Option<Participant>,
        value: Value,
    },
    /// Adds to a whole-number variable; the amount may be negative.
    Add {
        variable: VariableId,
        #[serde(default)]
        of: Option<Participant>,
        amount: i64,
    },
    /// A skill check by the rules' formula. Failing is an accepted outcome with its own
    /// effects. Invalid effects reject the entire command, restoring the random state along
    /// with everything else.
    Check {
        skill: Key,
        difficulty: u32,
        success: Vec<Action>,
        failure: Vec<Action>,
    },
}

impl Action {
    /// The participants this action names itself, not those of actions inside it.
    pub fn participants(&self) -> Vec<&Participant> {
        match self {
            Self::Relationship { from, to, .. } => vec![from, to],
            Self::ChangeResource { of, .. }
            | Self::ApplyEffect { of, .. }
            | Self::RemoveEffect { of, .. } => vec![of],
            Self::Move { actor, .. } => vec![actor],
            Self::StartDialogue { speaker, .. } => speaker.iter().collect(),
            Self::Set { of, .. } | Self::Add { of, .. } => of.iter().collect(),
            _ => vec![],
        }
    }
}

impl GameContent {
    /// Actions nest only so deep, so running them cannot exhaust the stack.
    pub(crate) fn validate_action(&self, action: &Action, depth: usize) -> Result<()> {
        self.validate_action_naming(action, depth, &|p| self.validate_participant(p))
    }

    /// As `validate_action`, with `participant` judging whom the action names: authored
    /// content names declared characters, a running script anyone in the playthrough.
    pub(crate) fn validate_action_naming(
        &self,
        action: &Action,
        depth: usize,
        participant: &dyn Fn(&Participant) -> Result<()>,
    ) -> Result<()> {
        require(depth <= 8, "actions nested too deeply")?;

        for named in action.participants() {
            participant(named)?;
        }

        match action {
            Action::Script(name) => {
                self.scripts.engine(name)?;
            }
            Action::SetLocked { object, .. } => {
                self.object(*object)?;
            }
            Action::If {
                condition,
                then,
                otherwise,
            } => {
                self.validate_condition(condition)?;
                require(
                    !then.is_empty() || !otherwise.is_empty(),
                    "a conditional action does nothing either way",
                )?;

                for action in then.iter().chain(otherwise) {
                    self.validate_action_naming(action, depth + 1, participant)?;
                }
            }
            Action::Quest { quest, transition } => {
                let q = self.quest(*quest)?;
                if let quests::Transition::CompleteObjective(id) = transition {
                    q.objective(id)?;
                }
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
                self.items.item(*definition)?;
                require(*quantity > 0, "zero item action")?;
            }
            Action::AwardExperience { amount } => {
                require(*amount > 0, "zero XP reward")?;
            }
            Action::Teach { skill } => {
                self.game.rules.skill(skill)?;
            }
            Action::Pay { amount } => require(*amount > 0, "paying nothing")?,
            Action::ChangeResource {
                resource, amount, ..
            } => {
                self.game.rules.resource(resource)?;
                require(*amount != 0, "changing a resource by nothing")?;
            }
            Action::ApplyEffect {
                effect,
                duration_ms,
                ..
            } => {
                self.game.rules.effect(effect)?;
                require(
                    duration_ms.is_none_or(|ms| ms > 0),
                    "effect duration must be positive",
                )?;
            }
            Action::RemoveEffect { effect, .. } => {
                self.game.rules.effect(effect)?;
            }
            Action::Move { to, timeout_ms, .. } => {
                self.area(*to)?;
                require(
                    timeout_ms.is_none_or(|ms| ms > 0 && ms <= 86_400_000),
                    "movement timeout must be 1ms..1 day",
                )?;
            }
            Action::StartDialogue { dialogue, .. } => {
                self.dialogue_contract(*dialogue)?;
            }
            Action::Set {
                variable,
                of,
                value,
            } => {
                value.validate()?;
                require(
                    self.variable_use(*variable, of)?.initial.same_type(value),
                    "value has a different type than the variable",
                )?
            }
            Action::Add { variable, of, .. } => require(
                matches!(self.variable_use(*variable, of)?.initial, Value::Int(_)),
                "only whole-number variables can be added to",
            )?,
            Action::Check {
                skill,
                difficulty,
                success,
                failure,
            } => {
                self.game.rules.skill(skill)?;
                require(*difficulty > 0, "zero check difficulty")?;

                for action in success.iter().chain(failure) {
                    self.validate_action_naming(action, depth + 1, participant)?;
                }
            }
        }

        Ok(())
    }
}

pub(crate) fn run_action(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    speaker: ActorId,
    // Everyone else taking part in the conversation the action belongs to.
    others: &BTreeSet<ActorId>,
    action: &Action,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match action {
        Action::Script(name) => content.scripts.engine(name)?.action(
            name,
            &mut ActScope {
                content,
                tx,
                player: actor,
                speaker,
                others,
                events,
            },
        )?,
        Action::SetLocked { object, locked } => {
            let o = tx.object_mut(content, *object)?;
            Rejection::Destroyed(*object).when(o.destroyed)?;
            o.set_locked(*locked);

            events.push(GameEvent::World(WorldEvent::ObjectChanged(*object)));
        }
        Action::If {
            condition,
            then,
            otherwise,
        } => {
            let holds = content.evaluate_among(condition, tx, actor, speaker, others)?;
            for action in if holds { then } else { otherwise } {
                run_action(content, tx, actor, speaker, others, action, events)?;
            }
        }
        Action::Quest { quest, transition } => {
            change_quest(content, tx, *quest, transition, events)?
        }
        Action::Relationship { from, to, amount } => change_relationship(
            tx,
            actors::RelationshipKey {
                from: from.resolve(actor, speaker),
                to: to.resolve(actor, speaker),
            },
            *amount,
            events,
        )?,
        Action::GrantItem {
            definition,
            quantity,
        } => {
            let id = character::carried(content, tx, actor)?;
            tx.inventory_mut(id)?
                .grant(&content.items, *definition, *quantity)?;
        }
        Action::ConsumeItem {
            definition,
            quantity,
        } => {
            let id = character::carried(content, tx, actor)?;
            let mut remaining = *quantity;
            let entries = tx.inventory(id)?.entries.clone();

            for entry in entries.iter().filter(|e| e.definition == *definition) {
                let amount = remaining.min(entry.quantity);

                if amount == 0 {
                    break;
                }

                tx.inventory_mut(id)?.remove(entry.id, amount)?;
                remaining -= amount;
            }

            Rejection::NotEnoughItems {
                definition: *definition,
                missing: remaining,
            }
            .when(remaining > 0)?;
            sync_equipment(content, tx, id, events)?;
        }
        Action::AwardExperience { amount } => {
            character::award_experience(content, tx, *amount, events)?
        }
        Action::Teach { skill } => character::teach(content, tx, actor, skill, events)?,
        Action::Pay { amount } => character::pay(tx, *amount, events)?,
        Action::ChangeResource {
            of,
            resource,
            amount,
        } => character::change_resource(
            content,
            tx,
            of.resolve(actor, speaker),
            resource,
            i64::from(*amount),
            events,
        )?,
        Action::ApplyEffect {
            of,
            effect,
            duration_ms,
        } => character::apply_effect(
            content,
            tx,
            of.resolve(actor, speaker),
            effect,
            *duration_ms,
            events,
        )?,
        Action::RemoveEffect { of, effect } => {
            character::remove_effect(content, tx, of.resolve(actor, speaker), effect, events)?
        }
        Action::Move {
            actor: who,
            to,
            timeout_ms,
        } => crate::world_runtime::request_move(
            content,
            tx,
            who.resolve(actor, speaker),
            *to,
            *timeout_ms,
            events,
        )?,
        Action::StartDialogue {
            dialogue,
            speaker: with,
        } => {
            content.dialogue_contract(*dialogue)?;
            let speaker = with.map_or(speaker, |p| p.resolve(actor, speaker));
            require(speaker != actor, "a conversation needs two actors")?;
            tx.actor(speaker)?;
            tx.pending_mut().push_back(Pending::Start {
                dialogue: *dialogue,
                participant: actor,
                speaker,
            });
        }
        Action::Set {
            variable,
            of,
            value,
        } => {
            require(
                content
                    .variable_use(*variable, of)?
                    .initial
                    .same_type(value),
                "value has a different type than the variable",
            )?;
            let key = VariableKey {
                variable: *variable,
                actor: of.map(|p| p.resolve(actor, speaker)),
            };
            tx.check_variable(content, key)?;
            tx.set_variable(key, value.clone());
        }
        Action::Add {
            variable,
            of,
            amount,
        } => {
            content.variable_use(*variable, of)?;
            let key = VariableKey {
                variable: *variable,
                actor: of.map(|p| p.resolve(actor, speaker)),
            };
            let Value::Int(current) = tx.variable(content, key)? else {
                return Err(Invalid("only whole-number variables can be added to".into()).into());
            };
            let sum = current
                .checked_add(*amount)
                .ok_or_else(|| Invalid("variable overflow".into()))?;
            tx.set_variable(key, Value::Int(sum));
        }
        Action::Check {
            skill,
            difficulty,
            success,
            failure,
        } => {
            let passed = character::check(content, tx, actor, skill, *difficulty, events)?;
            for action in if passed { success } else { failure } {
                run_action(content, tx, actor, speaker, others, action, events)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn change_quest(
    content: &GameContent,
    tx: &mut Tx,
    id: QuestId,
    transition: &quests::Transition,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let definition = content.quest(id)?;
    let progress = tx.quest_mut(id);
    progress.apply(definition, transition)?;
    events.push(GameEvent::QuestChanged {
        quest: id,
        status: progress.status,
    });
    Ok(())
}

pub(crate) fn change_relationship(
    tx: &mut Tx,
    key: actors::RelationshipKey,
    amount: i16,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let relationship = tx.relationship_mut(key);
    relationship.adjust(amount)?;
    events.push(GameEvent::RelationshipChanged {
        key,
        attitude: relationship.attitude,
    });
    Ok(())
}
