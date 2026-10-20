//! What a player or the engine can ask of a playthrough, carried out as one transaction, and
//! the events that report what happened.
use crate::{
    ConversationKey, DialogueVariant, GameContent, Rejection, Result, SessionHeader, WorldCommand,
    WorldEvent, actions, actors, character, combat, conversation,
    dialogue::{self, RunStatus, Token},
    inventory::{self, TradeQuote},
    quests,
    rules::{RandomState, Use},
    tx::Tx,
    world_runtime,
};

use game_types::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub enum Command {
    World(WorldCommand),
    AdvanceLine {
        key: ConversationKey,
        expected: Token,
    },
    InterruptDialogue {
        key: ConversationKey,
        expected: Token,
    },
    Talk {
        participant: ActorId,
        speaker: ActorId,
        topic: Option<Key>,
        bindings: BTreeMap<Key, ActorId>,
    },
    Quest {
        quest: QuestId,
        transition: quests::Transition,
    },
    AdjustRelationship {
        key: actors::RelationshipKey,
        amount: i16,
    },
    /// A character joins or leaves the party. One who joins gains the levels the party's
    /// experience has already earned.
    Party {
        actor: ActorId,
        member: bool,
    },
    /// The player steers this party member from now on.
    Control {
        actor: ActorId,
    },
    /// Someone looks into a character's inventory: to trade, to loot, to pick a pocket.
    /// An inventory nothing needed before is made now from the character's template.
    OpenInventory {
        actor: ActorId,
    },
    /// A party member puts one attribute point into a primary stat.
    SpendAttributePoint {
        actor: ActorId,
        stat: Key,
    },
    /// A character lines up something to do, after what is lined up already or, with
    /// `clear`, instead of it. What it is in the middle of goes on.
    Intend {
        actor: ActorId,
        intent: actors::Intent,
        clear: bool,
    },
    /// A character drops what it is doing and everything lined up.
    Interrupt {
        actor: ActorId,
    },
    UseItem {
        actor: ActorId,
        item: ItemId,
    },
    Equip {
        actor: ActorId,
        item: ItemId,
    },
    Unequip {
        actor: ActorId,
        slot: Key,
    },
    Transfer {
        source: InventoryId,
        destination: InventoryId,
        item: ItemId,
        quantity: u32,
    },
    Trade(TradeQuote),
    StartDialogue {
        bindings: BTreeMap<Key, ActorId>,
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
    },
    Choose {
        expected: Token,
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
        choice: Key,
    },
    AdvanceTime {
        millis: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    World(WorldEvent),
    LinePresented {
        key: ConversationKey,
        line: Key,
    },
    DialogueInterrupted {
        key: ConversationKey,
    },
    QuestChanged {
        quest: QuestId,
        status: quests::Status,
    },
    RelationshipChanged {
        key: actors::RelationshipKey,
        attitude: i16,
    },
    InteractionSelected {
        key: dialogue::InteractionKey,
        selection: dialogue::Selection,
    },
    DialogueResumed {
        dialogue: DialogueId,
    },
    ItemUsed {
        actor: ActorId,
        item: ItemId,
    },
    EquipmentChanged {
        actor: ActorId,
    },
    ItemsTransferred {
        entries: Vec<ItemId>,
    },
    TradeCompleted,
    DialogueStarted {
        key: ConversationKey,
        mode: dialogue::Mode,
    },
    ChoiceAccepted {
        choice: Key,
    },
    /// A check by the rules' formula, with what it rolled.
    Checked {
        actor: ActorId,
        skill: Key,
        passed: bool,
        rolls: Vec<u32>,
    },
    ResourceChanged {
        actor: ActorId,
        resource: Key,
        change: i32,
        now: i32,
    },
    Died {
        actor: ActorId,
    },
    EffectApplied {
        actor: ActorId,
        effect: Key,
    },
    EffectEnded {
        actor: ActorId,
        effect: Key,
    },
    ExperienceAwarded {
        amount: u64,
        total: u64,
    },
    LeveledUp {
        actor: ActorId,
        level: u32,
    },
    StatRaised {
        actor: ActorId,
        stat: Key,
    },
    SkillLearned {
        actor: ActorId,
        skill: Key,
        rank: u8,
    },
    Paid {
        amount: u64,
    },
    /// A character began something; it takes effect at `completes_at`.
    AbilityBegun {
        actor: ActorId,
        ability: Key,
        target: Option<ActorId>,
        completes_at: GameTime,
    },
    AbilityResolved {
        actor: ActorId,
        ability: Key,
        target: Option<ActorId>,
    },
    /// Something lined up could not be begun, or what was begun came to nothing.
    IntentDropped {
        actor: ActorId,
        ability: Key,
        reason: Rejection,
    },
    /// The ability's script failed; nothing it did took effect.
    AbilityFailed {
        actor: ActorId,
        ability: Key,
        reason: String,
    },
    Interrupted {
        actor: ActorId,
    },
    TimeAdvanced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub header: SessionHeader,
    pub events: Vec<GameEvent>,
}

/// Carries out one command within its transaction.
pub(crate) fn apply(
    content: &GameContent,
    state: &mut Tx,
    command: Command,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match command {
        Command::World(command) => world_runtime::apply(content, state, command, events)?,
        Command::Talk {
            participant,
            speaker,
            topic,
            bindings,
        } => talk(
            content,
            state,
            participant,
            speaker,
            topic,
            &bindings,
            events,
        )?,
        Command::Quest { quest, transition } => {
            actions::change_quest(content, state, quest, &transition, events)?
        }
        Command::AdjustRelationship { key, amount } => {
            state.actor(key.from)?;
            state.actor(key.to)?;
            actions::change_relationship(state, key, amount, events)?
        }
        Command::Party { actor, member } => {
            state.actor(actor)?;
            if member {
                if !state.party.members.contains(&actor) {
                    if state.party.members.len() >= content.game.rules.party_size {
                        return Err(Rejection::PartyFull.into());
                    }

                    state.party_mut().members.insert(actor);
                    character::catch_up(content, state, actor, events)?;
                }
            } else {
                Rejection::Controlled(actor).unless(state.party.controlled != Some(actor))?;
                state.party_mut().members.remove(&actor);
            }
        }
        Command::Control { actor } => {
            if !state.party.members.contains(&actor) {
                return Err(Rejection::NotInParty(actor).into());
            }
            state.party_mut().controlled = Some(actor);
        }
        Command::OpenInventory { actor } => {
            character::carried(content, state, actor)?;
        }
        Command::SpendAttributePoint { actor, stat } => {
            character::spend_attribute_point(content, state, actor, &stat, events)?
        }
        Command::Intend {
            actor,
            intent,
            clear,
        } => combat::intend(content, state, actor, intent, clear, events)?,
        Command::Interrupt { actor } => combat::interrupt(state, actor, events)?,
        Command::UseItem { actor, item } => {
            character::require_alive(content, state.actor(actor)?)?;
            character::carried(content, state, actor)?;
            let bag = state.carried(actor)?;
            let inventory = bag.id;
            let definition = content.items.item(bag.entry(item)?.definition)?;
            Rejection::NotUsable.unless(!definition.mechanics.on_use.is_empty())?;
            let worn = state.actor(actor)?.equipment.values().any(|id| *id == item);
            Rejection::Equipped.unless(!worn)?;

            for effect in &definition.mechanics.on_use {
                match effect {
                    Use::Restore { resource, amount } => character::change_resource(
                        content,
                        state,
                        actor,
                        resource,
                        i64::from(*amount),
                        events,
                    )?,
                    Use::Apply {
                        effect,
                        duration_ms,
                    } => character::apply_effect(
                        content,
                        state,
                        actor,
                        effect,
                        *duration_ms,
                        events,
                    )?,
                }
            }

            state
                .inventory_mut(inventory)?
                .remove(&content.items, item, 1)?;
            events.push(GameEvent::ItemUsed { actor, item });
        }
        Command::Equip { actor, item } => {
            character::carried(content, state, actor)?;
            let entry = state.carried(actor)?.entry(item)?;
            let slot = content
                .items
                .item(entry.definition)?
                .mechanics
                .slot
                .clone()
                .ok_or(Rejection::NotEquippable)?;
            state.actor_mut(actor)?.equipment.insert(slot, item);
            character::refresh(content, state, actor)?;
            events.push(GameEvent::EquipmentChanged { actor });
        }
        Command::Unequip { actor, slot } => {
            let removed = state.actor_mut(actor)?.equipment.remove(&slot);
            Rejection::SlotEmpty(slot).unless(removed.is_some())?;
            character::refresh(content, state, actor)?;
            events.push(GameEvent::EquipmentChanged { actor });
        }
        Command::Transfer {
            source,
            destination,
            item,
            quantity,
        } => {
            let mut from = state.inventory(source)?.clone();
            let mut to = state.inventory(destination)?.clone();
            let entries = inventory::transfer(&content.items, &mut from, &mut to, item, quantity)?;
            *state.inventory_mut(source)? = from;
            *state.inventory_mut(destination)? = to;

            for id in [source, destination] {
                character::sync_equipment(content, state, id, events)?;
            }

            events.push(GameEvent::ItemsTransferred { entries });
        }
        Command::Trade(quote) => {
            let p = quote.participants();
            let mut merchant = state.inventory(p.merchant_inventory)?.clone();
            let mut customer = state.inventory(p.customer_inventory)?.clone();
            let mut merchant_wallet = state.wallet(p.merchant_wallet)?.clone();
            let mut customer_wallet = state.wallet(p.customer_wallet)?.clone();
            inventory::commit_trade(
                &content.items,
                &mut merchant,
                &mut customer,
                &mut merchant_wallet,
                &mut customer_wallet,
                &quote,
            )?;
            *state.inventory_mut(p.merchant_inventory)? = merchant;
            *state.inventory_mut(p.customer_inventory)? = customer;
            *state.wallet_mut(p.merchant_wallet)? = merchant_wallet;
            *state.wallet_mut(p.customer_wallet)? = customer_wallet;

            for id in [p.merchant_inventory, p.customer_inventory] {
                character::sync_equipment(content, state, id, events)?;
            }

            events.push(GameEvent::TradeCompleted);
        }
        Command::StartDialogue {
            dialogue,
            participant,
            speaker,
            bindings,
        } => conversation::start(
            content,
            state,
            ConversationKey {
                dialogue,
                participant,
                speaker,
            },
            &bindings,
            events,
        )?,
        Command::Choose {
            dialogue,
            participant,
            speaker,
            choice,
            expected,
        } => conversation::choose(
            content,
            state,
            ConversationKey {
                dialogue,
                participant,
                speaker,
            },
            &choice,
            expected,
            events,
        )?,
        Command::AdvanceLine { key, expected } => {
            conversation::present(content, state, key, expected, events)?
        }
        Command::InterruptDialogue { key, expected } => {
            conversation::interrupt(content, state, key, expected, events)?
        }
        // A conversation on screen holds the world still.
        Command::AdvanceTime { millis } => {
            if state.floor.blocking.is_empty() {
                character::advance_time(content, state, millis, events)?
            }
        }
    }
    Ok(())
}

/// Talking to someone resumes the conversation under way between the two, or starts the one
/// the speaker's interaction profile selects, its variant rolled from the narrative stream.
fn talk(
    content: &GameContent,
    tx: &mut Tx,
    participant: ActorId,
    speaker: ActorId,
    topic: Option<Key>,
    bindings: &BTreeMap<Key, ActorId>,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let profile = content.interaction(tx, participant, speaker)?;
    let key = dialogue::InteractionKey {
        participant,
        speaker,
    };
    let under_way = tx.selection(key).map(|s| s.dialogue).filter(|&dialogue| {
        let conversation = tx.conversations.get(&ConversationKey {
            dialogue,
            participant,
            speaker,
        });
        conversation.is_some_and(|c| c.status == RunStatus::Active)
    });

    if let Some(dialogue) = under_way {
        let current = tx.conversation(ConversationKey {
            dialogue,
            participant,
            speaker,
        })?;
        require(
            bindings
                .iter()
                .all(|(r, id)| current.bindings.get(r) == Some(id)),
            "cannot rebind active conversation",
        )?;
        require(
            topic.is_none(),
            "finish the active conversation before choosing a topic",
        )?;
        events.push(GameEvent::DialogueResumed { dialogue });
        return Ok(());
    }

    let preview = content.preview(profile, tx, participant, speaker)?;
    let rule = preview.selected(topic.as_ref())?;
    let mut random = tx.narrative_random;
    let selection = dialogue::Selection {
        profile,
        rule: rule.rule.clone(),
        dialogue: pick(&rule.variants, &mut random)?,
    };
    let conversation = ConversationKey {
        dialogue: selection.dialogue,
        participant,
        speaker,
    };
    conversation::start(content, tx, conversation, bindings, events)?;
    tx.interaction_mut(key).current = Some(selection.clone());
    tx.set_narrative_random(random);
    events.push(GameEvent::InteractionSelected { key, selection });
    Ok(())
}

/// One of the variants, by weight. A single one costs no roll.
fn pick(variants: &[DialogueVariant], random: &mut RandomState) -> Result<DialogueId> {
    if let [only] = variants {
        return Ok(only.dialogue);
    }

    let total = variants.iter().map(|v| v.weight).sum();
    let mut roll = random.below(total)?;

    for variant in variants {
        if roll < variant.weight {
            return Ok(variant.dialogue);
        }

        roll -= variant.weight;
    }

    Err(Invalid("empty variant selection".into()).into())
}
