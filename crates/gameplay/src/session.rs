use crate::{
    Command, CommandOutcome, ContentIdentity, ContentSource, ConversationKey, ConversationView,
    GameContent, GameEvent, Graphs, InteractionPreview, MAX_LOADED_DIALOGUES, ObjectKind, Pending,
    Rejection, Result, SessionHeader, SessionState, TriggerIndex, WorldEvent, character,
    dialogue::RunStatus,
    inventory,
    inventory::{TradeOffer, TradeParticipants, TradeQuote},
    tx::Tx,
};

use game_types::*;
use std::collections::BTreeSet;

/// Pieces of queued work one command carries out at most; see `GameSession::settle`.
const SETTLE_STEPS: usize = 256;

/// One authoritative playthrough: the whole mutable state in memory, the always-loaded
/// definitions, and dialogue graphs the content reads when a conversation needs them.
/// Application adapters separately authorize player control, distance and access.
pub struct GameSession {
    identity: ContentIdentity,
    content: GameContent,
    triggers: TriggerIndex,
    state: SessionState,
}

impl GameSession {
    /// Starts or restores a playthrough. The state is brought in line with the content (see
    /// `adopt`), then checked against it in full.
    pub fn new(mut source: impl ContentSource + 'static, mut state: SessionState) -> Result<Self> {
        let identity = source.identity();
        let mut content = source.core()?;
        require(
            content.manifest == identity.manifest,
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
        let profile = self
            .content
            .interaction(&self.state, participant, speaker)?;
        self.content
            .preview(profile, &self.state, participant, speaker)
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
        Rejection::Destroyed(object).when(o.destroyed)?;
        Rejection::Locked(object).when(o.locked)?;
        Rejection::Closed(object).when(!o.open)?;
        let ObjectKind::Container { inventory, .. } = self.content.object(object)?.kind else {
            return Err(Invalid("object is not a container".into()).into());
        };
        self.state.inventory(inventory)
    }

    /// Accepts the command as a whole or leaves state, clock and random streams untouched.
    /// Then carries out the work it gave rise to (triggers listening for what it changed, the
    /// conversations they start, walks whose time ran out), each piece on its own, so the
    /// outcome is the settled state.
    pub fn apply(&mut self, command: Command) -> Result<CommandOutcome> {
        let mut events = self.transact(|content, _, tx, events| {
            crate::commands::apply(content, tx, command, events)
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
    state.rebuild_indexes();
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
