use crate::Result;
use crate::dialogue::{RunStatus, Token};
use crate::inventory::{TradeOffer, TradeParticipants, TradeQuote};
use crate::resolution::{resolve, resolve_with};
use crate::rules::{ActiveEffect, Effect};
use crate::*;
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
    RewardClaimed {
        key: dialogue::ClaimKey,
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
    DialogueStarted,
    ChoiceAccepted {
        choice: Key,
    },
    SkillChecked {
        roll: u32,
        passed: bool,
    },
    TimeAdvanced,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub header: SessionHeader,
    pub events: Vec<GameEvent>,
}
/// One authoritative session. Storage transactions publish state/RNG/events together.
/// Application adapters separately authorize player control, distance and access.
pub struct GameSession<S: StateStore, C: ContentSource> {
    store: S,
    content: C,
}
impl<S: StateStore, C: ContentSource> GameSession<S, C> {
    pub fn new(store: S, content: C) -> Result<Self> {
        require(
            store.identity() == &content.identity(),
            "session content identity mismatch",
        )?;
        store.header()?.validate()?;
        Ok(Self { store, content })
    }
    pub fn store(&self) -> &S {
        &self.store
    }
    pub fn content_source(&self) -> &C {
        &self.content
    }
    pub fn header(&self) -> Result<SessionHeader> {
        self.store.header()
    }
    pub fn identity(&self) -> &ContentIdentity {
        self.store.identity()
    }
    /// A bounded read model from a single storage snapshot. No random state is consumed.
    pub fn query(&mut self, request: StateRequest) -> Result<SessionState> {
        let mut tx = self.store.begin()?;
        let (batch, _) = resolve(&mut tx, &mut self.content, &request)?;
        Ok(batch.state)
    }
    pub fn derived(&mut self, actor: ActorId) -> Result<rules::Attributes> {
        let mut tx = self.store.begin()?;
        let (batch, content) = resolve(
            &mut tx,
            &mut self.content,
            &StateRequest {
                actors: [actor].into(),
                ..Default::default()
            },
        )?;
        batch.state.derived(&content, actor)
    }
    pub fn quote_trade(
        &mut self,
        participants: TradeParticipants,
        offer: TradeOffer,
    ) -> Result<TradeQuote> {
        let mut tx = self.store.begin()?;
        let (batch, content) = resolve(&mut tx, &mut self.content, &trade_request(participants))?;
        let state = &batch.state;
        Ok(inventory::quote_trade(
            &content.items,
            state.inventory(participants.merchant_inventory)?,
            state.inventory(participants.customer_inventory)?,
            state.wallet(participants.merchant_wallet)?,
            state.wallet(participants.customer_wallet)?,
            &offer,
        )?)
    }
    pub fn available_choices(
        &mut self,
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
    ) -> Result<Vec<Key>> {
        Ok(self
            .conversation_view(ConversationKey {
                dialogue,
                participant,
                speaker,
            })?
            .choices
            .into_iter()
            .map(|c| c.id)
            .collect())
    }
    pub fn conversation_view(&mut self, key: ConversationKey) -> Result<ConversationView> {
        let mut tx = self.store.begin()?;
        let (batch, content) = resolve(
            &mut tx,
            &mut self.content,
            &conversation_request(key.dialogue, key.participant, key.speaker),
        )?;
        content.conversation_view(&batch.state, key)
    }
    pub fn preview_interaction(
        &mut self,
        participant: ActorId,
        speaker: ActorId,
    ) -> Result<InteractionPreview> {
        let mut tx = self.store.begin()?;
        let (batch, content, profile) =
            interaction_context(&mut tx, &mut self.content, participant, speaker)?;
        content.preview(profile, &batch.state, participant, speaker)
    }
    /// Adapter polling is indexed and survives loading a checkpoint. Starting is idempotent.
    pub fn next_movement(&mut self, after: Option<TriggerId>) -> Result<Option<MoveRequest>> {
        let mut tx = self.store.begin()?;
        let Some(id) = tx.next_movement(after, None)? else {
            return Ok(None);
        };
        let (batch, _) = resolve(
            &mut tx,
            &mut self.content,
            &StateRequest {
                triggers: [id].into(),
                ..Default::default()
            },
        )?;
        Ok(batch.state.world.trigger(id)?.movement.clone())
    }
    pub fn world_work_pending(&mut self) -> Result<bool> {
        let mut tx = self.store.begin()?;
        let time = tx.header().time;
        Ok(tx.next_event()?.is_some() || tx.next_movement(None, Some(time))?.is_some())
    }
    /// Ordinary container access enforces durable lock/open/destruction state.
    pub fn container_contents(&mut self, object: ObjectId) -> Result<inventory::Inventory> {
        let mut tx = self.store.begin()?;
        let (batch, content) = resolve(
            &mut tx,
            &mut self.content,
            &StateRequest {
                objects: [object].into(),
                ..Default::default()
            },
        )?;
        let o = batch.state.world.object(object)?;
        require(
            !o.locked && o.open && !o.destroyed,
            "container is not accessible",
        )?;
        let ObjectKind::Container { inventory } = content.object(object)?.kind else {
            return Err(Invalid("object is not a container".into()).into());
        };
        // Keep lock queries and unloaded-object mutations independent of inventory contents.
        let (contents, _) = resolve(
            &mut tx,
            &mut self.content,
            &StateRequest {
                objects: [object].into(),
                inventories: [inventory].into(),
                ..Default::default()
            },
        )?;
        Ok(contents.state.inventory(inventory)?.clone())
    }
    pub fn apply(&mut self, command: Command) -> Result<CommandOutcome> {
        let mut tx = self.store.begin()?;
        let mut header = tx.header().clone();
        let mut events = Vec::new();
        if let Command::World(command) = command {
            crate::world_runtime::apply(
                &mut tx,
                &mut self.content,
                command,
                &mut header,
                &mut events,
            )?;
        } else if let Command::AdvanceTime { millis } = command {
            header.time = header.time.advance(millis)?;
            header.validate()?;
            let mut processed = 0;
            while let Some(actor) = tx.next_expiring_actor(header.time)? {
                require(
                    processed < MAX_EXPIRATIONS_PER_COMMAND,
                    "expiration work budget exceeded",
                )?;
                let request = StateRequest {
                    actors: [actor].into(),
                    ..Default::default()
                };
                let (batch, content) = resolve(&mut tx, &mut self.content, &request)?;
                let mut next = batch.state.clone();
                apply(
                    &content,
                    &mut next,
                    Command::AdvanceTime { millis },
                    &mut Vec::new(),
                )?;
                next.validate(&content)?;
                crate::world_runtime::stage(&mut tx, &mut self.content, &batch, &next)?;
                processed += 1;
            }
            events.push(GameEvent::TimeAdvanced);
        } else if let Command::Talk {
            participant,
            speaker,
            topic,
            bindings,
        } = command
        {
            let (batch, content, profile) =
                interaction_context(&mut tx, &mut self.content, participant, speaker)?;
            let key = dialogue::InteractionKey {
                participant,
                speaker,
            };
            let active = batch
                .state
                .interaction(key)?
                .current
                .as_ref()
                .filter(|selection| {
                    batch.state.conversations.iter().any(|c| {
                        c.dialogue == selection.dialogue
                            && c.participant == participant
                            && c.speaker == speaker
                            && c.status == RunStatus::Active
                    })
                });
            if let Some(active) = active {
                let current = crate::conversation::conversation(
                    &batch.state,
                    ConversationKey {
                        dialogue: active.dialogue,
                        participant,
                        speaker,
                    },
                )?;
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
                events.push(GameEvent::DialogueResumed {
                    dialogue: active.dialogue,
                });
            } else {
                let preview = content.preview(profile, &batch.state, participant, speaker)?;
                let rule = preview.selected(topic.as_ref())?;
                let mut random = batch.state.narrative_random;
                let total: u32 = rule.variants.iter().map(|v| v.weight).sum();
                let mut roll = if rule.variants.len() == 1 {
                    0
                } else {
                    random.below(total)?
                };
                let selected = rule
                    .variants
                    .iter()
                    .find(|v| {
                        if roll < v.weight {
                            true
                        } else {
                            roll -= v.weight;
                            false
                        }
                    })
                    .ok_or_else(|| Invalid("empty variant selection".into()))?;
                let selection = dialogue::Selection {
                    profile,
                    rule: rule.rule.clone(),
                    dialogue: selected.dialogue,
                };
                // Load only the chosen graph. All state reads use the same transaction; failures
                // before commit preserve narrative RNG, active selection and gameplay effects.
                let mut request = conversation_request(selection.dialogue, participant, speaker);
                request.interactions.insert(key);
                require(bindings.len() <= 14, "too many role bindings")?;
                request.actors.extend(bindings.values());
                let (batch, content) = resolve_with(
                    &mut tx,
                    &mut self.content,
                    &request,
                    &ContentRequest {
                        profiles: [profile].into(),
                        ..Default::default()
                    },
                )?;
                let mut next = batch.state.clone();
                apply(
                    &content,
                    &mut next,
                    Command::StartDialogue {
                        dialogue: selection.dialogue,
                        participant,
                        speaker,
                        bindings,
                    },
                    &mut events,
                )?;
                next.interactions
                    .iter_mut()
                    .find(|i| i.key == key)
                    .unwrap()
                    .current = Some(selection.clone());
                next.narrative_random = random;
                next.validate(&content)?;
                header.narrative_random = random;
                crate::world_runtime::stage(&mut tx, &mut self.content, &batch, &next)?;
                events.push(GameEvent::InteractionSelected { key, selection });
            }
        } else {
            let request = command_request(&command);
            let (batch, content) = resolve(&mut tx, &mut self.content, &request)?;
            let mut next = batch.state.clone();
            apply(&content, &mut next, command, &mut events)?;
            assign_item_ids(&batch.state, &mut next, &mut events);
            next.validate(&content)?;
            header.random = next.random;
            header.narrative_random = next.narrative_random;
            crate::world_runtime::stage(&mut tx, &mut self.content, &batch, &next)?;
        }
        header.generation = header
            .generation
            .checked_add(1)
            .ok_or_else(|| Invalid("session generation overflow".into()))?;
        header.validate()?;
        tx.commit(&header)?;
        Ok(CommandOutcome { header, events })
    }
}
fn trade_request(p: TradeParticipants) -> StateRequest {
    StateRequest {
        inventories: [p.merchant_inventory, p.customer_inventory].into(),
        wallets: [p.merchant_wallet, p.customer_wallet].into(),
        ..Default::default()
    }
}
fn conversation_request(
    dialogue: DialogueId,
    participant: ActorId,
    speaker: ActorId,
) -> StateRequest {
    StateRequest {
        conversations: [ConversationKey {
            dialogue,
            participant,
            speaker,
        }]
        .into(),
        ..Default::default()
    }
}
fn command_request(command: &Command) -> StateRequest {
    match command {
        Command::World(_) => StateRequest::default(),
        Command::AdvanceLine { key, .. } | Command::InterruptDialogue { key, .. } => {
            conversation_request(key.dialogue, key.participant, key.speaker)
        }
        Command::Talk {
            participant,
            speaker,
            ..
        } => StateRequest {
            interactions: [dialogue::InteractionKey {
                participant: *participant,
                speaker: *speaker,
            }]
            .into(),
            ..Default::default()
        },
        Command::Quest { quest, .. } => StateRequest {
            quests: [*quest].into(),
            ..Default::default()
        },
        Command::AdjustRelationship { key, .. } => StateRequest {
            relationships: [*key].into(),
            ..Default::default()
        },
        Command::UseItem { actor, .. }
        | Command::Equip { actor, .. }
        | Command::Unequip { actor, .. } => StateRequest {
            actors: [*actor].into(),
            ..Default::default()
        },
        Command::Transfer {
            source,
            destination,
            ..
        } => StateRequest {
            inventories: [*source, *destination].into(),
            ..Default::default()
        },
        Command::Trade(q) => trade_request(q.participants()),
        Command::StartDialogue {
            dialogue,
            participant,
            speaker,
            bindings,
        } => {
            let mut request = conversation_request(*dialogue, *participant, *speaker);
            request.actors.extend(bindings.values());
            request
        }
        Command::Choose {
            dialogue,
            participant,
            speaker,
            ..
        } => conversation_request(*dialogue, *participant, *speaker),
        Command::AdvanceTime { .. } => StateRequest::default(),
    }
}
fn apply(
    content: &GameContent,
    state: &mut SessionState,
    command: Command,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match command {
        Command::World(_) | Command::Talk { .. } => {
            return Err(Invalid("Talk requires session selection".into()).into());
        }
        Command::Quest { quest, transition } => {
            change_quest(content, state, quest, &transition, events)?
        }
        Command::AdjustRelationship { key, amount } => {
            change_relationship(state, key, amount, events)?
        }
        Command::UseItem { actor, item } => {
            let bag = state.carried(actor)?;
            let inventory = bag.id;
            let definition = content.items.item(bag.entry(item)?.definition)?;
            require(
                !definition.mechanics.on_use.is_empty(),
                "item cannot be used",
            )?;
            require(
                !state.actor(actor)?.equipment.values().any(|id| *id == item),
                "unequip before consuming an item",
            )?;
            for effect in &definition.mechanics.on_use {
                match effect {
                    Effect::Heal(amount) => {
                        let maximum = state.derived(content, actor)?
                            [&content.game.rules.health_attribute]
                            as u32;
                        let actor = state.actor_mut(actor)?;
                        actor.health = actor.health.saturating_add(*amount).min(maximum);
                    }
                    Effect::Buff {
                        modifier,
                        duration_ms,
                    } => {
                        let expires_at = state.time.advance(*duration_ms)?;
                        state.actor_mut(actor)?.effects.push(ActiveEffect {
                            modifier: modifier.clone(),
                            expires_at,
                        });
                    }
                }
            }
            state
                .inventory_mut(inventory)?
                .remove(&content.items, item, 1)?;
            state.clamp_health(content)?;
            events.push(GameEvent::ItemUsed { actor, item });
        }
        Command::Equip { actor, item } => {
            let entry = state.carried(actor)?.entry(item)?;
            let slot = content
                .items
                .item(entry.definition)?
                .mechanics
                .slot
                .clone()
                .ok_or_else(|| Invalid("item cannot be equipped".into()))?;
            state.actor_mut(actor)?.equipment.insert(slot, item);
            state.clamp_health(content)?;
            events.push(GameEvent::EquipmentChanged { actor });
        }
        Command::Unequip { actor, slot } => {
            require(
                state.actor_mut(actor)?.equipment.remove(&slot).is_some(),
                "slot is empty",
            )?;
            state.clamp_health(content)?;
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
            remove_missing_equipment(content, state, events)?;
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
            for wallet in &mut state.wallets {
                if wallet.id == merchant_wallet.id {
                    *wallet = merchant_wallet.clone();
                } else if wallet.id == customer_wallet.id {
                    *wallet = customer_wallet.clone();
                }
            }
            remove_missing_equipment(content, state, events)?;
            events.push(GameEvent::TradeCompleted);
        }
        Command::StartDialogue {
            dialogue,
            participant,
            speaker,
            bindings,
        } => crate::conversation::start(
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
        } => crate::conversation::choose(
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
            crate::conversation::present(content, state, key, expected, events)?
        }
        Command::InterruptDialogue { key, expected } => {
            crate::conversation::interrupt(content, state, key, expected, events)?
        }
        Command::AdvanceTime { millis } => {
            let target = state.time.advance(millis)?;
            // Preserve intermediate health caps even when one step crosses several expirations.
            let deadlines: std::collections::BTreeSet<_> = state
                .actors
                .iter()
                .flat_map(|a| a.effects.iter())
                .map(|e| e.expires_at)
                .filter(|t| *t > state.time && *t <= target)
                .collect();
            for time in deadlines.into_iter().chain(std::iter::once(target)) {
                state.time = time;
                for actor in &mut state.actors {
                    actor.effects.retain(|e| e.expires_at > time);
                }
                state.clamp_health(content)?;
            }
            events.push(GameEvent::TimeAdvanced);
        }
    }
    Ok(())
}
fn remove_missing_equipment(
    content: &GameContent,
    state: &mut SessionState,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    for index in 0..state.actors.len() {
        let actor = state.actors[index].id;
        let owned: std::collections::BTreeSet<_> =
            state.carried(actor)?.entries.iter().map(|e| e.id).collect();
        let equipment = &mut state.actors[index].equipment;
        let previous = equipment.len();
        equipment.retain(|_, id| owned.contains(id));
        if previous != equipment.len() {
            events.push(GameEvent::EquipmentChanged { actor });
        }
    }
    state.clamp_health(content)
}
pub(crate) fn conditions_met(
    content: &GameContent,
    state: &SessionState,
    actor: ActorId,
    speaker: ActorId,
    dialogue: DialogueId,
    conditions: &[Key],
) -> Result<bool> {
    for id in conditions {
        if !content
            .evaluate(
                &content.game.conditions[&BindingId::new(dialogue, id.clone())],
                state,
                actor,
                speaker,
            )?
            .matched
        {
            return Ok(false);
        }
    }
    Ok(true)
}
pub(crate) fn run_action(
    content: &GameContent,
    state: &mut SessionState,
    actor: ActorId,
    speaker: ActorId,
    action: &Action,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match action {
        Action::SetLocked { object, locked } => {
            let o = state
                .world
                .objects
                .iter_mut()
                .find(|o| o.id == *object)
                .ok_or_else(|| Invalid("object state not resolved".into()))?;
            require(!o.destroyed, "object destroyed")?;
            o.locked = *locked;
            if *locked {
                o.open = false;
            }
            events.push(GameEvent::World(WorldEvent::ObjectChanged(*object)));
        }
        Action::Claim { claim, actions } => {
            let key = content.claim(*claim)?.key(actor, speaker);
            if state.claim(key)?.claimed {
                return Ok(());
            }
            state
                .claims
                .iter_mut()
                .find(|c| c.key == key)
                .unwrap()
                .claimed = true;
            for action in actions {
                run_action(content, state, actor, speaker, action, events)?;
            }
            events.push(GameEvent::RewardClaimed { key });
        }
        Action::Quest { quest, transition } => {
            change_quest(content, state, *quest, transition, events)?
        }
        Action::Relationship { from, to, amount } => change_relationship(
            state,
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
            let id = state.carried(actor)?.id;
            state
                .inventory_mut(id)?
                .grant(&content.items, *definition, *quantity)?;
        }
        Action::ConsumeItem {
            definition,
            quantity,
        } => {
            let id = state.carried(actor)?.id;
            let mut remaining = *quantity;
            let entries = state.inventory(id)?.entries.clone();
            for entry in entries.iter().filter(|e| e.definition == *definition) {
                let amount = remaining.min(entry.quantity);
                if amount == 0 {
                    break;
                }
                state
                    .inventory_mut(id)?
                    .remove(&content.items, entry.id, amount)?;
                remaining -= amount;
            }
            require(remaining == 0, "not enough items for dialogue action")?;
            remove_missing_equipment(content, state, events)?;
        }
        Action::AwardExperience { skill, amount } => {
            state
                .actor_mut(actor)?
                .award_experience(&content.game.rules, skill, *amount)?
        }
        Action::SetFact { key, value } => {
            if *value {
                state.facts.insert(key.clone());
            } else {
                state.facts.remove(key);
            }
        }
        Action::SkillCheck {
            skill,
            difficulty,
            success,
            failure,
        } => {
            let bonus = state.actor(actor)?.skills.get(skill).copied().unwrap_or(0) / 100;
            let roll = state.random.roll_d20();
            let passed = bonus.saturating_add(u64::from(roll)) >= u64::from(*difficulty);
            events.push(GameEvent::SkillChecked { roll, passed });
            for action in if passed { success } else { failure } {
                run_action(content, state, actor, speaker, action, events)?;
            }
        }
    }
    Ok(())
}

/// Runtime-created identities are derived from the accepted command, independently of narrative RNG.
pub(crate) fn assign_item_ids(
    before: &SessionState,
    after: &mut SessionState,
    events: &mut [GameEvent],
) {
    use std::collections::{BTreeMap, BTreeSet};
    let existing: BTreeSet<_> = before
        .inventories
        .iter()
        .flat_map(|i| i.entries.iter().map(|e| e.id))
        .collect();
    let mut replacements = BTreeMap::new();
    let mut ordinal = 0u64;
    after.inventories.sort_by_key(|i| i.id);
    for entry in after.inventories.iter_mut().flat_map(|i| &mut i.entries) {
        if !existing.contains(&entry.id) {
            let mut hash = blake3::Hasher::new();
            hash.update(b"yarra-item-v1");
            hash.update(&before.playthrough.0);
            hash.update(&before.generation.to_le_bytes());
            hash.update(&ordinal.to_le_bytes());
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&hash.finalize().as_bytes()[..16]);
            let id = ItemId(bytes);
            replacements.insert(entry.id, id);
            entry.id = id;
            ordinal += 1;
        }
    }
    for event in events {
        if let GameEvent::ItemsTransferred { entries } = event {
            for id in entries {
                if let Some(new) = replacements.get(id) {
                    *id = *new;
                }
            }
        }
    }
}

fn change_quest(
    content: &GameContent,
    state: &mut SessionState,
    id: QuestId,
    transition: &quests::Transition,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let progress = state
        .quests
        .iter_mut()
        .find(|q| q.quest == id)
        .ok_or_else(|| Invalid("quest state not resolved".into()))?;
    progress.apply(content.quest(id)?, transition)?;
    events.push(GameEvent::QuestChanged {
        quest: id,
        status: progress.status,
    });
    Ok(())
}
fn change_relationship(
    state: &mut SessionState,
    key: actors::RelationshipKey,
    amount: i16,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let relationship = state
        .relationships
        .iter_mut()
        .find(|r| r.key == key)
        .ok_or_else(|| Invalid("relationship state not resolved".into()))?;
    relationship.adjust(amount)?;
    events.push(GameEvent::RelationshipChanged {
        key,
        attitude: relationship.attitude,
    });
    Ok(())
}
fn interaction_context<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    source: &mut C,
    participant: ActorId,
    speaker: ActorId,
) -> Result<(WorkingSet, GameContent, InteractionProfileId)> {
    require(participant != speaker, "interaction needs two actors")?;
    let request = StateRequest {
        interactions: [dialogue::InteractionKey {
            participant,
            speaker,
        }]
        .into(),
        ..Default::default()
    };
    let (batch, content) = resolve(tx, source, &request)?;
    let template = batch.state.actor(speaker)?.template;
    let profile = content
        .game
        .actors
        .iter()
        .find(|t| t.id == template)
        .and_then(|t| t.interaction)
        .ok_or_else(|| Invalid("NPC has no interaction profile".into()))?;
    let (batch, content) = resolve_with(
        tx,
        source,
        &request,
        &ContentRequest {
            profiles: [profile].into(),
            ..Default::default()
        },
    )?;
    Ok((batch, content, profile))
}
