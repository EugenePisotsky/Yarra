use crate::Result;
use crate::character;
use crate::dialogue::{RunStatus, Token};
use crate::inventory::{TradeOffer, TradeParticipants, TradeQuote};
use crate::rules::Use;
use crate::tx::Tx;
use crate::*;
use game_types::*;
use std::collections::{BTreeMap, BTreeSet};

/// Pieces of queued work one command carries out at most; see `GameSession::settle`.
const SETTLE_STEPS: usize = 256;

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
/// One authoritative playthrough: the whole mutable state in memory, the always-loaded
/// definitions, and dialogue graphs the content reads when a conversation needs them.
/// Application adapters separately authorize player control, distance and access.
pub struct GameSession {
    identity: ContentIdentity,
    content: GameContent,
    triggers: TriggerIndex,
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
impl GameSession {
    /// Starts or restores a playthrough. The state is brought in line with the content (see
    /// `adopt`), then checked against it in full.
    pub fn new(mut source: impl ContentSource + 'static, mut state: SessionState) -> Result<Self> {
        let identity = source.identity();
        let mut content = source.core()?;
        require(
            content.manifest == identity.manifest && content.game.dialogues.is_empty(),
            "session content identity mismatch",
        )?;
        content.graphs = Graphs::reading(move |id| source.dialogue(id));
        adopt(&content, &mut state)?;
        // Conversations under way are checked against their graphs.
        let active = state.conversations.values();
        let active = active.filter(|c| c.status == RunStatus::Active);
        for conversation in active.take(MAX_LOADED_DIALOGUES) {
            content.dialogue(conversation.dialogue)?;
        }
        state.validate(&content)?;
        Ok(Self {
            identity,
            triggers: TriggerIndex::build(&content),
            content,
            state,
        })
    }
    pub fn state(&self) -> &SessionState {
        &self.state
    }
    pub fn content(&self) -> &GameContent {
        &self.content
    }
    pub fn header(&self) -> SessionHeader {
        SessionHeader::capture(&self.state)
    }
    pub fn identity(&self) -> &ContentIdentity {
        &self.identity
    }
    /// A stat after equipment and effects, or how much of a resource the actor has now.
    pub fn stat(&self, actor: ActorId, stat: &Key) -> Result<i32> {
        self.state.stat(&self.content, actor, stat)
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
        &self,
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
    pub fn conversation_view(&self, key: ConversationKey) -> Result<ConversationView> {
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
            .ok_or_else(|| Rejection::NothingToSay(speaker).into())
    }
    /// Whether work was left for the next command, which only content whose triggers keep
    /// setting each other off leaves.
    pub fn world_work_pending(&self) -> bool {
        !self.state.world.pending.is_empty()
            || crate::world_runtime::timed_out(&self.state).is_some()
    }
    /// Whether the engine should report where this actor is: party members, anyone asked to
    /// walk, and anyone whose comings and goings a trigger listens for.
    pub fn observed(&self, actor: ActorId) -> bool {
        self.state.party.members.contains(&actor)
            || self.state.world.movements.contains_key(&actor)
            || self.triggers.watches(actor)
    }
    /// Ordinary container access enforces durable lock/open/destruction state.
    pub fn container_contents(&self, object: ObjectId) -> Result<&inventory::Inventory> {
        let o = self.state.object(&self.content, object)?;
        Rejection::Destroyed(object).unless(!o.destroyed)?;
        Rejection::Locked(object).unless(!o.locked)?;
        Rejection::Closed(object).unless(o.open)?;
        let ObjectKind::Container { inventory, .. } = self.content.object(object)?.kind else {
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
    /// Then carries out the work it gave rise to (triggers listening for what it changed, the
    /// conversations they start, walks whose time ran out), each piece on its own, so the
    /// outcome is the settled state.
    pub fn apply(&mut self, command: Command) -> Result<CommandOutcome> {
        let talk = match &command {
            Command::Talk {
                participant,
                speaker,
                topic,
                ..
            } => Some(self.plan_talk(*participant, *speaker, topic.as_ref())?),
            _ => None,
        };
        let mut events = self.transact(|content, _, tx, events| match command {
            Command::World(command) => crate::world_runtime::apply(content, tx, command, events),
            Command::Talk {
                participant,
                speaker,
                topic,
                bindings,
            } => start_talk(
                content,
                tx,
                participant,
                speaker,
                topic,
                bindings,
                talk.expect("planned above"),
                events,
            ),
            command => apply(content, tx, command, events),
        })?;
        self.settle(&mut events);
        Ok(CommandOutcome {
            header: self.header(),
            events,
        })
    }
    /// Runs `work` as one step: all of it takes effect and the generation moves on, or none
    /// of it does.
    fn transact(
        &mut self,
        work: impl FnOnce(&GameContent, &TriggerIndex, &mut Tx, &mut Vec<GameEvent>) -> Result<()>,
    ) -> Result<Vec<GameEvent>> {
        let (content, triggers) = (&self.content, &self.triggers);
        let mut tx = Tx::begin(&mut self.state);
        let mut events = Vec::new();
        let result = work(content, triggers, &mut tx, &mut events)
            .and_then(|()| finish(content, triggers, &mut tx));
        match result {
            Ok(()) => Ok(events),
            Err(error) => {
                tx.rollback();
                Err(error)
            }
        }
    }
    /// Carries out queued work until none is left. Content whose triggers keep setting each
    /// other off is cut short after `SETTLE_STEPS`; the rest waits for the next command, so
    /// the game goes on and the events show what repeats.
    fn settle(&mut self, events: &mut Vec<GameEvent>) {
        for _ in 0..SETTLE_STEPS {
            if !self.world_work_pending() {
                return;
            }
            let step = self.transact(|content, triggers, tx, events| {
                crate::world_runtime::process_next(content, triggers, tx, events);
                Ok(())
            });
            match step {
                Ok(more) => events.extend(more),
                Err(error) => {
                    events.push(GameEvent::World(WorldEvent::WorkFailed {
                        reason: error.to_string(),
                    }));
                    let dropped = self.transact(|_, _, tx, _| {
                        crate::world_runtime::drop_next(tx);
                        Ok(())
                    });
                    if dropped.is_err() {
                        return;
                    }
                }
            }
        }
    }
    /// Release the state, e.g. to hand a finished tool run to another session.
    pub fn into_state(self) -> SessionState {
        self.state
    }
}
/// Brings a playthrough, perhaps saved with other content, in line with this content:
/// every character's stats are worked out again with the current formulas and its
/// resources kept within their caps, and party members reach the level the party's
/// experience now earns. What cannot be brought in line fails the check that follows.
fn adopt(content: &GameContent, state: &mut SessionState) -> Result<()> {
    let rules = &content.game.rules;
    let actors: Vec<ActorId> = state.actors.keys().copied().collect();
    for id in actors {
        let stats = state.sheet(content, &state.actors[&id])?;
        let actor = state.actors.get_mut(&id).expect("listed above");
        actor.set_stats(stats, rules);
    }
    let members: Vec<ActorId> = state.party.members.iter().copied().collect();
    let mut tx = Tx::begin(state);
    for member in members {
        character::catch_up(content, &mut tx, member, &mut Vec::new())?;
    }
    Ok(())
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
/// Completes an accepted command: turns for conversations that started or ended, checks of
/// what changed, notifications for subscribed triggers, and the next generation.
fn finish(content: &GameContent, triggers: &TriggerIndex, tx: &mut Tx) -> Result<()> {
    take_turns(content, tx)?;
    // The rules keep what they change consistent; looking again only finds mistakes in
    // their own code, so release builds leave it out. A loaded save is always checked.
    if cfg!(debug_assertions) {
        check_changes(content, tx)?;
    }
    let signals: Vec<_> = crate::world_runtime::signals(content, tx)
        .into_iter()
        .filter(|s| triggers.subscribed(s))
        .collect();
    for signal in signals {
        tx.pending_mut().push_back(Pending::Signal(signal));
    }
    tx.bump_generation()
}
/// A conversation that started waits for its turn behind those of its kind already under
/// way; one that ended gives its turn up.
fn take_turns(content: &GameContent, tx: &mut Tx) -> Result<()> {
    let touched: Vec<ConversationKey> = tx.before.conversations.keys().copied().collect();
    for key in touched {
        let mode = content.dialogue_contract(key.dialogue)?.mode;
        let active = tx
            .conversations
            .get(&key)
            .is_some_and(|c| c.status == RunStatus::Active);
        if active != tx.floor.queue(mode).contains(&key) {
            let queue = tx.floor_mut().queue_mut(mode);
            if active {
                queue.push_back(key);
            } else {
                queue.retain(|k| *k != key);
            }
        }
    }
    Ok(())
}
/// Re-checks only the records this command wrote, plus actors whose equipment depends on a
/// changed inventory. Stats are worked out again, and compared, only for characters whose
/// build changed.
fn check_changes(content: &GameContent, tx: &Tx) -> Result<()> {
    let state: &SessionState = tx;
    let before = &tx.before;
    let mut actors: BTreeSet<ActorId> = before.actors.keys().copied().collect();
    for id in before.inventories.keys() {
        if let Some(inv) = state.inventories.get(id) {
            state.check_inventory(content, inv)?;
            if let Some(actor) = inv.owner.as_actor() {
                actors.insert(actor);
            }
        }
    }
    for id in actors {
        if let Some(actor) = state.actors.get(&id) {
            let rebuilt = match before.actors.get(&id) {
                Some(Some(old)) => !old.same_build(actor),
                Some(None) => true,
                None => false,
            };
            state.check_actor(content, actor, rebuilt)?;
        }
    }
    for id in before.wallets.keys() {
        state.check_wallet(content, state.wallet(*id)?)?;
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
    for key in before.conversations.keys() {
        state.check_conversation(content, state.conversation(*key)?)?;
        if state.conversation(*key)?.status == RunStatus::Active {
            state.check_floor(content, *key)?;
        }
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
        } => crate::combat::intend(content, state, actor, intent, clear, events)?,
        Command::Interrupt { actor } => crate::combat::interrupt(state, actor, events)?,
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
        // A conversation on screen holds the world still.
        Command::AdvanceTime { millis } => {
            if state.floor.blocking.is_empty() {
                character::advance_time(content, state, millis, events)?
            }
        }
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
    let Some(actor) = bag.carried_by() else {
        return Ok(());
    };
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
        character::refresh(content, state, actor)?;
        events.push(GameEvent::EquipmentChanged { actor });
    }
    Ok(())
}
pub(crate) fn run_action(
    content: &GameContent,
    state: &mut Tx,
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
                tx: state,
                player: actor,
                speaker,
                others,
                events,
            },
        )?,
        Action::SetLocked { object, locked } => {
            let o = state.object_mut(content, *object)?;
            Rejection::Destroyed(*object).unless(!o.destroyed)?;
            o.locked = *locked;
            if *locked {
                o.open = false;
            }
            events.push(GameEvent::World(WorldEvent::ObjectChanged(*object)));
        }
        Action::If {
            condition,
            then,
            otherwise,
        } => {
            let holds = content.evaluate_among(condition, state, actor, speaker, others)?;
            for action in if holds.matched { then } else { otherwise } {
                run_action(content, state, actor, speaker, others, action, events)?;
            }
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
            let id = character::carried(content, state, actor)?;
            state
                .inventory_mut(id)?
                .grant(&content.items, *definition, *quantity)?;
        }
        Action::ConsumeItem {
            definition,
            quantity,
        } => {
            let id = character::carried(content, state, actor)?;
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
            Rejection::NotEnoughItems {
                definition: *definition,
                missing: remaining,
            }
            .unless(remaining == 0)?;
            sync_equipment(content, state, id, events)?;
        }
        Action::AwardExperience { amount } => {
            character::award_experience(content, state, *amount, events)?
        }
        Action::Teach { skill } => character::teach(content, state, actor, skill, events)?,
        Action::Pay { amount } => character::pay(state, *amount, events)?,
        Action::ChangeResource {
            of,
            resource,
            amount,
        } => character::change_resource(
            content,
            state,
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
            state,
            of.resolve(actor, speaker),
            effect,
            *duration_ms,
            events,
        )?,
        Action::RemoveEffect { of, effect } => {
            character::remove_effect(content, state, of.resolve(actor, speaker), effect, events)?
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
        Action::Check {
            skill,
            difficulty,
            success,
            failure,
        } => {
            let passed = character::check(content, state, actor, skill, *difficulty, events)?;
            for action in if passed { success } else { failure } {
                run_action(content, state, actor, speaker, others, action, events)?;
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
