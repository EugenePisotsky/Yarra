use crate::Result;
use crate::dialogue::{RunStatus, Token};
use crate::inventory::{TradeOffer, TradeParticipants, TradeQuote};
use crate::rules::{ActiveEffect, Effect};
use crate::tx::Tx;
use crate::*;
use game_types::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    /// A character joins or leaves the group travelling with the player.
    Party {
        actor: ActorId,
        member: bool,
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
    DialogueStarted {
        key: ConversationKey,
        mode: dialogue::Mode,
    },
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
/// One authoritative playthrough: the whole mutable state in memory, the always-loaded
/// definitions, and dialogue graphs loaded when a conversation needs them.
/// Application adapters separately authorize player control, distance and access.
pub struct GameSession<C: ContentSource> {
    source: C,
    identity: ContentIdentity,
    content: GameContent,
    triggers: TriggerIndex,
    /// Loaded graphs, least recently used first.
    loaded: VecDeque<DialogueId>,
    state: SessionState,
}
/// What `Talk` will do, decided from a read-only look at the state before anything changes.
enum Talk {
    Resume(DialogueId),
    Start {
        selection: dialogue::Selection,
        random: rules::RandomState,
    },
}
impl<C: ContentSource> GameSession<C> {
    /// Starts or restores a playthrough. The state is checked in full against the content.
    pub fn new(mut source: C, state: SessionState) -> Result<Self> {
        let identity = source.identity();
        let content = source.core()?;
        require(
            content.manifest == identity.manifest && content.game.dialogues.is_empty(),
            "session content identity mismatch",
        )?;
        let triggers = TriggerIndex::build(&content);
        let mut session = Self {
            source,
            identity,
            content,
            triggers,
            loaded: VecDeque::new(),
            state,
        };
        let active: Vec<_> = session
            .state
            .conversations
            .values()
            .filter(|c| c.status == RunStatus::Active)
            .map(|c| c.dialogue)
            .take(MAX_LOADED_DIALOGUES)
            .collect();
        for id in active {
            session.ensure_dialogue(id)?;
        }
        session.state.validate(&session.content)?;
        Ok(session)
    }
    pub fn state(&self) -> &SessionState {
        &self.state
    }
    /// Always-loaded definitions plus the dialogue graphs currently in memory.
    pub fn content(&self) -> &GameContent {
        &self.content
    }
    pub fn content_source(&self) -> &C {
        &self.source
    }
    pub fn header(&self) -> SessionHeader {
        SessionHeader::capture(&self.state)
    }
    pub fn identity(&self) -> &ContentIdentity {
        &self.identity
    }
    fn ensure_dialogue(&mut self, id: DialogueId) -> Result<()> {
        if let Some(position) = self.loaded.iter().position(|d| *d == id) {
            self.loaded.remove(position);
            self.loaded.push_back(id);
            return Ok(());
        }
        let contract = self.content.dialogue_contract(id)?;
        let graph = self.source.dialogue(id)?;
        graph.validate()?;
        require(
            graph.id == id && &graph.contract() == contract,
            "dialogue contract mismatch",
        )?;
        if self.loaded.len() == MAX_LOADED_DIALOGUES
            && let Some(old) = self.loaded.pop_front()
        {
            self.content.game.dialogues.retain(|d| d.id != old);
        }
        self.content.game.dialogues.push(graph);
        self.loaded.push_back(id);
        Ok(())
    }
    pub fn derived(&self, actor: ActorId) -> Result<rules::Attributes> {
        self.state.derived(&self.content, actor)
    }
    pub fn quote_trade(
        &self,
        participants: TradeParticipants,
        offer: TradeOffer,
    ) -> Result<TradeQuote> {
        let state = &self.state;
        Ok(inventory::quote_trade(
            &self.content.items,
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
        self.state.conversation(key)?;
        self.ensure_dialogue(key.dialogue)?;
        self.content.conversation_view(&self.state, key)
    }
    /// No random state is consumed and no graph is loaded.
    pub fn preview_interaction(
        &self,
        participant: ActorId,
        speaker: ActorId,
    ) -> Result<InteractionPreview> {
        let profile = self.profile(participant, speaker)?;
        self.content
            .preview(profile, &self.state, participant, speaker)
    }
    fn profile(&self, participant: ActorId, speaker: ActorId) -> Result<InteractionProfileId> {
        require(participant != speaker, "interaction needs two actors")?;
        self.state.actor(participant)?;
        self.content
            .template(self.state.actor(speaker)?.template)?
            .interaction
            .ok_or_else(|| Invalid("NPC has no interaction profile".into()).into())
    }
    pub fn world_work_pending(&self) -> bool {
        !self.state.world.pending.is_empty()
            || crate::world_runtime::timed_out(&self.state).is_some()
    }
    /// Ordinary container access enforces durable lock/open/destruction state.
    pub fn container_contents(&self, object: ObjectId) -> Result<&inventory::Inventory> {
        let o = self.state.object(&self.content, object)?;
        require(
            !o.locked && o.open && !o.destroyed,
            "container is not accessible",
        )?;
        let ObjectKind::Container { inventory } = self.content.object(object)?.kind else {
            return Err(Invalid("object is not a container".into()).into());
        };
        self.state.inventory(inventory)
    }
    fn plan_talk(
        &self,
        participant: ActorId,
        speaker: ActorId,
        topic: Option<&Key>,
    ) -> Result<Talk> {
        let profile = self.profile(participant, speaker)?;
        let state = &self.state;
        let key = dialogue::InteractionKey {
            participant,
            speaker,
        };
        if let Some(active) = state.selection(key).filter(|selection| {
            state
                .conversations
                .get(&ConversationKey {
                    dialogue: selection.dialogue,
                    participant,
                    speaker,
                })
                .is_some_and(|c| c.status == RunStatus::Active)
        }) {
            return Ok(Talk::Resume(active.dialogue));
        }
        let preview = self.content.preview(profile, state, participant, speaker)?;
        let rule = preview.selected(topic)?;
        let mut random = state.narrative_random;
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
        Ok(Talk::Start {
            selection: dialogue::Selection {
                profile,
                rule: rule.rule.clone(),
                dialogue: selected.dialogue,
            },
            random,
        })
    }
    /// Accepts the command as a whole or leaves state, clock and random streams untouched.
    pub fn apply(&mut self, command: Command) -> Result<CommandOutcome> {
        // Reads and graph loading happen first; nothing below performs I/O.
        let mut unloadable = None;
        let talk = match &command {
            // A conversation queued by an action starts now; its graph must be in memory.
            Command::World(WorldCommand::ProcessNext)
                if crate::world_runtime::timed_out(&self.state).is_none() =>
            {
                if let Some(Pending::Start { dialogue, .. }) = self.state.world.pending.front() {
                    unloadable = self
                        .ensure_dialogue(*dialogue)
                        .err()
                        .map(|error| error.to_string());
                }
                None
            }
            Command::Talk {
                participant,
                speaker,
                topic,
                ..
            } => {
                let plan = self.plan_talk(*participant, *speaker, topic.as_ref())?;
                if let Talk::Start { selection, .. } = &plan {
                    self.ensure_dialogue(selection.dialogue)?;
                }
                Some(plan)
            }
            Command::StartDialogue { dialogue, .. } | Command::Choose { dialogue, .. } => {
                self.ensure_dialogue(*dialogue)?;
                None
            }
            Command::AdvanceLine { key, .. } | Command::InterruptDialogue { key, .. } => {
                self.ensure_dialogue(key.dialogue)?;
                None
            }
            _ => None,
        };
        let content = &self.content;
        let triggers = &self.triggers;
        let mut tx = Tx::begin(&mut self.state);
        let mut events = Vec::new();
        let result = (|| {
            match command {
                Command::World(command) => crate::world_runtime::apply(
                    content,
                    triggers,
                    &mut tx,
                    command,
                    unloadable,
                    &mut events,
                )?,
                Command::Talk {
                    participant,
                    speaker,
                    topic,
                    bindings,
                } => {
                    let plan = talk.expect("planned above");
                    start_talk(
                        content,
                        &mut tx,
                        participant,
                        speaker,
                        topic,
                        bindings,
                        plan,
                        &mut events,
                    )?
                }
                command => apply(content, &mut tx, command, &mut events)?,
            }
            finish(content, triggers, &mut tx, &mut events)
        })();
        match result {
            Ok(()) => Ok(CommandOutcome {
                header: SessionHeader::capture(&self.state),
                events,
            }),
            Err(error) => {
                tx.rollback();
                Err(error)
            }
        }
    }
    /// Release the state, e.g. to hand a finished tool run to another session.
    pub fn into_state(self) -> SessionState {
        self.state
    }
}
#[allow(clippy::too_many_arguments)]
fn start_talk(
    content: &GameContent,
    tx: &mut Tx,
    participant: ActorId,
    speaker: ActorId,
    topic: Option<Key>,
    bindings: BTreeMap<Key, ActorId>,
    plan: Talk,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match plan {
        Talk::Resume(dialogue) => {
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
        }
        Talk::Start { selection, random } => {
            require(bindings.len() <= 14, "too many role bindings")?;
            let key = dialogue::InteractionKey {
                participant,
                speaker,
            };
            crate::conversation::start(
                content,
                tx,
                ConversationKey {
                    dialogue: selection.dialogue,
                    participant,
                    speaker,
                },
                &bindings,
                events,
            )?;
            tx.interaction_mut(key).current = Some(selection.clone());
            tx.set_narrative_random(random);
            events.push(GameEvent::InteractionSelected { key, selection });
        }
    }
    Ok(())
}
/// Completes an accepted command: stable identities for new items, checks of what changed,
/// notifications for subscribed triggers, and the next generation.
fn finish(
    content: &GameContent,
    triggers: &TriggerIndex,
    tx: &mut Tx,
    events: &mut [GameEvent],
) -> Result<()> {
    assign_item_ids(tx, events)?;
    check_changes(content, tx)?;
    let signals: Vec<_> = crate::world_runtime::signals(tx)
        .into_iter()
        .filter(|s| triggers.subscribed(s))
        .collect();
    require(
        signals.len() <= MAX_EVENTS_PER_COMMAND,
        "command event budget exceeded",
    )?;
    require(
        tx.world.pending.len() + signals.len() <= MAX_PENDING_EVENTS,
        "pending event queue full; process work before accepting more",
    )?;
    for signal in signals {
        tx.pending_mut().push_back(Pending::Signal(signal));
    }
    tx.bump_generation()
}
/// Re-checks only the records this command wrote, plus actors whose equipment or health
/// depends on a changed inventory.
fn check_changes(content: &GameContent, tx: &Tx) -> Result<()> {
    let state: &SessionState = tx;
    let before = &tx.before;
    let mut actors: BTreeSet<ActorId> = before.actors.keys().copied().collect();
    for id in before.inventories.keys() {
        if let Some(inv) = state.inventories.get(id) {
            state.check_inventory(content, inv)?;
            if inv.owner.kind == "actor" {
                actors.insert(ActorId(inv.owner.id.0));
            }
        }
    }
    for id in actors {
        if let Some(actor) = state.actors.get(&id) {
            state.check_actor(content, actor)?;
        }
    }
    for id in before.wallets.keys() {
        state.check_wallet(state.wallet(*id)?)?;
    }
    for id in before.quests.keys() {
        state.quest(*id).validate(content.quest(*id)?)?;
    }
    for key in before.relationships.keys() {
        state.check_relationship(&state.relationship(*key))?;
    }
    for key in before.histories.keys() {
        state.check_history(content, &state.history(*key))?;
    }
    for key in &before.claims {
        state.check_claim(content, *key)?;
    }
    for key in before.conversations.keys() {
        state.check_conversation(content, state.conversation(*key)?)?;
    }
    for key in before.interactions.keys() {
        if let Some(i) = state.interactions.get(key) {
            state.check_interaction(content, i)?;
        }
    }
    for id in before.objects.keys() {
        state.check_object(content, &state.object(content, *id)?)?;
    }
    for actor in before.locations.keys() {
        if let Some(location) = state.world.locations.get(actor) {
            state.check_location(content, location)?;
        }
    }
    for id in before.triggers.keys() {
        state.check_trigger(content, &state.trigger(*id))?;
    }
    for actor in before.movements.keys() {
        if let Some(movement) = state.world.movements.get(actor) {
            state.check_movement(content, movement)?;
        }
    }
    for key in before.variables.keys() {
        state.check_variable(content, *key)?;
    }
    Ok(())
}
fn apply(
    content: &GameContent,
    state: &mut Tx,
    command: Command,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match command {
        Command::World(_) | Command::Talk { .. } => {
            return Err(Invalid("command requires session selection".into()).into());
        }
        Command::Quest { quest, transition } => {
            change_quest(content, state, quest, &transition, events)?
        }
        Command::AdjustRelationship { key, amount } => {
            state.actor(key.from)?;
            state.actor(key.to)?;
            change_relationship(state, key, amount, events)?
        }
        Command::Party { actor, member } => {
            state.actor(actor)?;
            state.set_party(actor, member);
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
            clamp_health(content, state, actor)?;
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
            clamp_health(content, state, actor)?;
            events.push(GameEvent::EquipmentChanged { actor });
        }
        Command::Unequip { actor, slot } => {
            require(
                state.actor_mut(actor)?.equipment.remove(&slot).is_some(),
                "slot is empty",
            )?;
            clamp_health(content, state, actor)?;
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
                sync_equipment(content, state, id, events)?;
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
                sync_equipment(content, state, id, events)?;
            }
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
            require(
                target.0 <= i64::MAX as u64,
                "logical time exceeds storage range",
            )?;
            let expiring: Vec<ActorId> = state
                .actors
                .values()
                .filter(|a| a.effects.iter().any(|e| e.expires_at <= target))
                .map(|a| a.id)
                .collect();
            // Preserve intermediate health caps even when one step crosses several expirations.
            for id in expiring {
                let deadlines: BTreeSet<_> = state
                    .actor(id)?
                    .effects
                    .iter()
                    .map(|e| e.expires_at)
                    .filter(|t| *t <= target)
                    .collect();
                for time in deadlines {
                    state.actor_mut(id)?.effects.retain(|e| e.expires_at > time);
                    clamp_health(content, state, id)?;
                }
            }
            state.set_time(target);
            events.push(GameEvent::TimeAdvanced);
        }
    }
    Ok(())
}
fn clamp_health(content: &GameContent, state: &mut Tx, actor: ActorId) -> Result<()> {
    let max = state.derived(content, actor)?[&content.game.rules.health_attribute] as u32;
    if state.actor(actor)?.health > max {
        state.actor_mut(actor)?.health = max;
    }
    Ok(())
}
/// After items leave a carried inventory, drop equipment that is no longer owned.
fn sync_equipment(
    content: &GameContent,
    state: &mut Tx,
    inventory: InventoryId,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let bag = state.inventory(inventory)?;
    if bag.owner.kind != "actor" || bag.role != CARRIED {
        return Ok(());
    }
    let actor = ActorId(bag.owner.id.0);
    let owned: BTreeSet<_> = bag.entries.iter().map(|e| e.id).collect();
    if state
        .actor(actor)?
        .equipment
        .values()
        .any(|id| !owned.contains(id))
    {
        state
            .actor_mut(actor)?
            .equipment
            .retain(|_, id| owned.contains(id));
        events.push(GameEvent::EquipmentChanged { actor });
    }
    clamp_health(content, state, actor)
}
pub(crate) fn run_action(
    content: &GameContent,
    state: &mut Tx,
    actor: ActorId,
    speaker: ActorId,
    action: &Action,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match action {
        Action::Script(name) => content.scripts.engine(name)?.action(
            name,
            &mut ActScope {
                content,
                tx: state,
                player: actor,
                speaker,
                events,
            },
        )?,
        Action::SetLocked { object, locked } => {
            let o = state.object_mut(content, *object)?;
            require(!o.destroyed, "object destroyed")?;
            o.locked = *locked;
            if *locked {
                o.open = false;
            }
            events.push(GameEvent::World(WorldEvent::ObjectChanged(*object)));
        }
        Action::Claim { claim, actions } => {
            let key = content.claim(*claim)?.key(actor, speaker);
            if state.claimed(key) {
                return Ok(());
            }
            state.claim(key);
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
            sync_equipment(content, state, id, events)?;
        }
        Action::AwardExperience { skill, amount } => {
            state
                .actor_mut(actor)?
                .award_experience(&content.game.rules, skill, *amount)?
        }
        Action::Move {
            actor: who,
            to,
            timeout_ms,
        } => crate::world_runtime::request_move(
            content,
            state,
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
            state.actor(speaker)?;
            state.pending_mut().push_back(Pending::Start {
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
            state.check_variable(content, key)?;
            state.set_variable(key, value.clone());
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
            let Value::Int(current) = state.variable(content, key)? else {
                return Err(Invalid("only whole-number variables can be added to".into()).into());
            };
            let sum = current
                .checked_add(*amount)
                .ok_or_else(|| Invalid("variable overflow".into()))?;
            state.set_variable(key, Value::Int(sum));
        }
        Action::SkillCheck {
            skill,
            difficulty,
            success,
            failure,
        } => {
            let bonus = state.actor(actor)?.skills.get(skill).copied().unwrap_or(0) / 100;
            let roll = state.random_mut().roll_d20();
            let passed = bonus.saturating_add(u64::from(roll)) >= u64::from(*difficulty);
            events.push(GameEvent::SkillChecked { roll, passed });
            for action in if passed { success } else { failure } {
                run_action(content, state, actor, speaker, action, events)?;
            }
        }
    }
    Ok(())
}

/// Items created by a command get identities derived from the playthrough and generation,
/// so replaying the same command after a load produces the same items.
fn assign_item_ids(tx: &mut Tx, events: &mut [GameEvent]) -> Result<()> {
    let existing: BTreeSet<ItemId> = tx
        .before
        .inventories
        .values()
        .flatten()
        .flat_map(|i| i.entries.iter().map(|e| e.id))
        .collect();
    let touched: Vec<InventoryId> = tx.before.inventories.keys().copied().collect();
    let (playthrough, generation) = (tx.playthrough, tx.generation);
    let mut replacements = BTreeMap::new();
    let mut ordinal = 0u64;
    for id in touched {
        if !tx.inventories.contains_key(&id) {
            continue;
        }
        for entry in &mut tx.inventory_mut(id)?.entries {
            if !existing.contains(&entry.id) {
                let mut hash = blake3::Hasher::new();
                hash.update(b"yarra-item-v1");
                hash.update(&playthrough.0);
                hash.update(&generation.to_le_bytes());
                hash.update(&ordinal.to_le_bytes());
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(&hash.finalize().as_bytes()[..16]);
                replacements.insert(entry.id, ItemId(bytes));
                entry.id = ItemId(bytes);
                ordinal += 1;
            }
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
    Ok(())
}

fn change_quest(
    content: &GameContent,
    state: &mut Tx,
    id: QuestId,
    transition: &quests::Transition,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let definition = content.quest(id)?;
    let progress = state.quest_mut(id);
    progress.apply(definition, transition)?;
    events.push(GameEvent::QuestChanged {
        quest: id,
        status: progress.status,
    });
    Ok(())
}
fn change_relationship(
    state: &mut Tx,
    key: actors::RelationshipKey,
    amount: i16,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let relationship = state.relationship_mut(key);
    relationship.adjust(amount)?;
    events.push(GameEvent::RelationshipChanged {
        key,
        attitude: relationship.attitude,
    });
    Ok(())
}
