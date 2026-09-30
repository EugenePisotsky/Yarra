//! A bounded scenario exercise. Resolves authored definitions to live stack IDs,
//! then uses the same session commands a future UI will use.
use crate::{ItemAmount, Result};
use game_types::*;
use gameplay::inventory::{TradeLine, TradeOffer, TradeParticipants};
use gameplay::{Command, ContentSource, GameSession};
use gameplay::{actors, inventory, quests};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Step {
    World(gameplay::WorldCommand),
    PumpWorld {
        limit: usize,
    },
    ExpectObject {
        object: ObjectId,
        locked: bool,
        open: bool,
    },
    /// The actor has been asked to walk to this area and has not arrived yet.
    ExpectMovement {
        actor: ActorId,
        to: AreaId,
    },
    ExpectContainer {
        object: ObjectId,
        entries: usize,
    },
    AdvanceLine {
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
    },
    Interrupt {
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
    },
    ExpectLine {
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
        line: Key,
        actor: ActorId,
    },

    Talk {
        bindings: std::collections::BTreeMap<Key, ActorId>,
        participant: ActorId,
        speaker: ActorId,
        topic: Option<Key>,
    },
    Quest {
        quest: QuestId,
        transition: quests::Transition,
    },
    AdjustRelationship {
        key: actors::RelationshipKey,
        amount: i16,
    },
    ExpectOpening {
        participant: ActorId,
        speaker: ActorId,
        rule: Key,
    },
    ExpectTopics {
        participant: ActorId,
        speaker: ActorId,
        topics: Vec<Key>,
    },
    ExpectQuest {
        quest: QuestId,
        status: quests::Status,
    },
    UseItem {
        actor: ActorId,
        definition: ItemDefinitionId,
    },
    Equip {
        actor: ActorId,
        definition: ItemDefinitionId,
    },
    Unequip {
        actor: ActorId,
        slot: Key,
    },
    Transfer {
        source: InventoryId,
        destination: InventoryId,
        item: ItemAmount,
    },
    Trade {
        participants: TradeParticipants,
        purchases: Vec<ItemAmount>,
        sales: Vec<ItemAmount>,
    },
    StartDialogue {
        bindings: std::collections::BTreeMap<Key, ActorId>,
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
    },
    Choose {
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
        choice: Key,
    },
    AdvanceTime {
        millis: u64,
    },
}
fn entry<C: ContentSource>(
    session: &GameSession<C>,
    inventory: InventoryId,
    definition: ItemDefinitionId,
) -> Result<ItemId> {
    session
        .state()
        .inventory(inventory)?
        .entries
        .iter()
        .find(|e| e.definition == definition)
        .map(|e| e.id)
        .ok_or_else(|| Invalid(format!("inventory {inventory} has no item {definition}")).into())
}
fn trade_lines<C: ContentSource>(
    session: &GameSession<C>,
    inventory: InventoryId,
    amounts: &[ItemAmount],
) -> Result<Vec<TradeLine>> {
    require(
        amounts.len() <= inventory::MAX_TRADE_LINES,
        "too many trade requests",
    )?;
    let mut lines = Vec::new();
    for amount in amounts {
        require(amount.quantity > 0, "trade quantity must be positive")?;
        let mut remaining = amount.quantity;
        for entry in &session.state().inventory(inventory)?.entries {
            if entry.definition != amount.definition || remaining == 0 {
                continue;
            }
            let quantity = remaining.min(entry.quantity);
            remaining -= quantity;
            lines.push(TradeLine {
                entry: entry.id,
                quantity,
            });
        }
        require(remaining == 0, "insufficient stock for scenario trade")?;
    }
    Ok(lines)
}
impl Step {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::World(_) => "World",
            Self::PumpWorld { .. } => "PumpWorld",
            Self::ExpectObject { .. } => "ExpectObject",
            Self::ExpectMovement { .. } => "ExpectMovement",
            Self::ExpectContainer { .. } => "ExpectContainer",
            Self::AdvanceLine { .. } => "AdvanceLine",
            Self::Interrupt { .. } => "Interrupt",
            Self::ExpectLine { .. } => "ExpectLine",
            Self::Talk { .. } => "Talk",
            Self::Quest { .. } => "Quest",
            Self::AdjustRelationship { .. } => "AdjustRelationship",
            Self::ExpectOpening { .. } => "ExpectOpening",
            Self::ExpectTopics { .. } => "ExpectTopics",
            Self::ExpectQuest { .. } => "ExpectQuest",
            Self::UseItem { .. } => "UseItem",
            Self::Equip { .. } => "Equip",
            Self::Unequip { .. } => "Unequip",
            Self::Transfer { .. } => "Transfer",
            Self::Trade { .. } => "Trade",
            Self::StartDialogue { .. } => "StartDialogue",
            Self::Choose { .. } => "Choose",
            Self::AdvanceTime { .. } => "AdvanceTime",
        }
    }
    pub fn apply<C: ContentSource>(&self, session: &mut GameSession<C>) -> Result<()> {
        let command = match self {
            Self::World(command) => Command::World(command.clone()),
            Self::PumpWorld { limit } => {
                require(*limit <= 10000, "scenario delivery budget exceeded")?;
                for _ in 0..*limit {
                    if !session.world_work_pending() {
                        return Ok(());
                    }
                    session.apply(Command::World(gameplay::WorldCommand::ProcessNext))?;
                }
                require(
                    !session.world_work_pending(),
                    "scenario delivery budget exhausted; work remains pending",
                )?;
                return Ok(());
            }
            Self::ExpectObject {
                object,
                locked,
                open,
            } => {
                let o = session.state().object(session.content(), *object)?;
                require(
                    o.locked == *locked && o.open == *open && !o.destroyed,
                    "unexpected object state",
                )?;
                return Ok(());
            }
            Self::ExpectMovement { actor, to } => {
                require(
                    session
                        .state()
                        .world
                        .movements
                        .get(actor)
                        .is_some_and(|m| m.to == *to),
                    "unexpected pending movement",
                )?;
                return Ok(());
            }
            Self::ExpectContainer { object, entries } => {
                require(
                    session.container_contents(*object)?.entries.len() == *entries,
                    "unexpected container contents",
                )?;
                return Ok(());
            }
            Self::AdvanceLine {
                dialogue,
                participant,
                speaker,
            }
            | Self::Interrupt {
                dialogue,
                participant,
                speaker,
            } => {
                let key = gameplay::ConversationKey {
                    dialogue: *dialogue,
                    participant: *participant,
                    speaker: *speaker,
                };
                let expected = session.conversation_view(key)?.token;
                if matches!(self, Self::AdvanceLine { .. }) {
                    Command::AdvanceLine { key, expected }
                } else {
                    Command::InterruptDialogue { key, expected }
                }
            }
            Self::ExpectLine {
                dialogue,
                participant,
                speaker,
                line,
                actor,
            } => {
                let view = session.conversation_view(gameplay::ConversationKey {
                    dialogue: *dialogue,
                    participant: *participant,
                    speaker: *speaker,
                })?;
                require(
                    view.line
                        .is_some_and(|v| v.id == *line && v.speaker == *actor),
                    "unexpected conversation line/speaker",
                )?;
                return Ok(());
            }
            Self::Talk {
                participant,
                speaker,
                topic,
                bindings,
            } => Command::Talk {
                bindings: bindings.clone(),
                participant: *participant,
                speaker: *speaker,
                topic: topic.clone(),
            },
            Self::Quest { quest, transition } => Command::Quest {
                quest: *quest,
                transition: transition.clone(),
            },
            Self::AdjustRelationship { key, amount } => Command::AdjustRelationship {
                key: *key,
                amount: *amount,
            },
            Self::ExpectOpening {
                participant,
                speaker,
                rule,
            } => {
                let preview = session.preview_interaction(*participant, *speaker)?;
                require(
                    preview.opening().is_some_and(|r| r.rule == *rule),
                    "unexpected NPC opening",
                )?;
                return Ok(());
            }
            Self::ExpectTopics {
                participant,
                speaker,
                topics,
            } => {
                let preview = session.preview_interaction(*participant, *speaker)?;
                let actual: std::collections::BTreeSet<_> =
                    preview.topics().map(|r| r.rule.clone()).collect();
                require(
                    actual == topics.iter().cloned().collect(),
                    "unexpected NPC topics",
                )?;
                return Ok(());
            }
            Self::ExpectQuest { quest, status } => {
                require(
                    session.state().quest(*quest).status == *status,
                    "unexpected quest status",
                )?;
                return Ok(());
            }
            Self::UseItem { actor, definition } => Command::UseItem {
                actor: *actor,
                item: entry(session, session.state().carried(*actor)?.id, *definition)?,
            },
            Self::Equip { actor, definition } => Command::Equip {
                actor: *actor,
                item: entry(session, session.state().carried(*actor)?.id, *definition)?,
            },
            Self::Unequip { actor, slot } => Command::Unequip {
                actor: *actor,
                slot: slot.clone(),
            },
            Self::Transfer {
                source,
                destination,
                item,
            } => {
                require(item.quantity > 0, "transfer quantity must be positive")?;
                let mut remaining = item.quantity;
                while remaining > 0 {
                    let id = entry(session, *source, item.definition)?;
                    let quantity = session
                        .state()
                        .inventory(*source)?
                        .entry(id)?
                        .quantity
                        .min(remaining);
                    session.apply(Command::Transfer {
                        source: *source,
                        destination: *destination,
                        item: id,
                        quantity,
                    })?;
                    remaining -= quantity;
                }
                return Ok(());
            }
            Self::Trade {
                participants,
                purchases,
                sales,
            } => {
                let offer = TradeOffer {
                    purchases: trade_lines(session, participants.merchant_inventory, purchases)?,
                    sales: trade_lines(session, participants.customer_inventory, sales)?,
                };
                Command::Trade(session.quote_trade(*participants, offer)?)
            }
            Self::StartDialogue {
                bindings,
                dialogue,
                participant,
                speaker,
            } => Command::StartDialogue {
                bindings: bindings.clone(),
                dialogue: *dialogue,
                participant: *participant,
                speaker: *speaker,
            },
            Self::Choose {
                dialogue,
                participant,
                speaker,
                choice,
            } => Command::Choose {
                expected: session
                    .conversation_view(gameplay::ConversationKey {
                        dialogue: *dialogue,
                        participant: *participant,
                        speaker: *speaker,
                    })?
                    .token,
                dialogue: *dialogue,
                participant: *participant,
                speaker: *speaker,
                choice: choice.clone(),
            },
            Self::AdvanceTime { millis } => Command::AdvanceTime { millis: *millis },
        };
        session.apply(command)?;
        Ok(())
    }
}
