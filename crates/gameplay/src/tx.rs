//! One command's view of the state. Every write first remembers the record it replaces, so a
//! rejected command restores the state exactly, and an accepted one knows precisely what it
//! changed. Cost follows the records a command touches, not the size of the playthrough.
use crate::actors::{Actor, Relationship, RelationshipKey};
use crate::dialogue::{ClaimKey, Conversation, History, HistoryKey, Interaction, InteractionKey};
use crate::inventory::{Inventory, Wallet};
use crate::rules::RandomState;
use crate::{Result, *};
use game_types::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy)]
struct Scalars {
    time: GameTime,
    random: RandomState,
    narrative_random: RandomState,
}
impl Scalars {
    fn of(state: &SessionState) -> Self {
        Self {
            time: state.time,
            random: state.random,
            narrative_random: state.narrative_random,
        }
    }
}
/// The single list of journaled record maps: `name / touch: Key => Value, state path`.
macro_rules! journal {
    ($($name:ident / $touch:ident: $key:ty => $value:ty, $($path:ident).+;)*) => {
        /// Values as they were before this command first touched them. `None` means absent.
        pub(crate) struct Before {
            scalars: Scalars,
            $(pub $name: BTreeMap<$key, Option<$value>>,)*
            pub claims: BTreeSet<ClaimKey>,
            pending: Option<VecDeque<Pending>>,
            /// Signals raised directly by this scope; dropped with it if it is undone.
            pub signals: Vec<WorldSignal>,
            party: Option<Party>,
            timed: Option<BTreeSet<ActorId>>,
        }
        impl Before {
            fn new(state: &SessionState) -> Self {
                Self {
                    scalars: Scalars::of(state),
                    $($name: BTreeMap::new(),)*
                    claims: BTreeSet::new(),
                    pending: None,
                    signals: Vec::new(),
                    party: None,
                    timed: None,
                }
            }
            fn restore(self, state: &mut SessionState) {
                $(for (key, value) in self.$name {
                    match value {
                        Some(value) => state.$($path).+.insert(key, value),
                        None => state.$($path).+.remove(&key),
                    };
                })*
                for key in self.claims {
                    state.claims.remove(&key);
                }
                if let Some(pending) = self.pending {
                    state.world.pending = pending;
                }
                if let Some(party) = self.party {
                    state.party = party;
                }
                if let Some(timed) = self.timed {
                    state.timed = timed;
                }
                state.time = self.scalars.time;
                state.random = self.scalars.random;
                state.narrative_random = self.scalars.narrative_random;
            }
            /// Fold a finished nested scope into the enclosing one; the earliest value wins.
            fn merge_into(self, outer: &mut Before) {
                $(for (key, value) in self.$name {
                    outer.$name.entry(key).or_insert(value);
                })*
                outer.claims.extend(self.claims);
                if outer.pending.is_none() {
                    outer.pending = self.pending;
                }
                outer.signals.extend(self.signals);
                if outer.party.is_none() {
                    outer.party = self.party;
                }
                if outer.timed.is_none() {
                    outer.timed = self.timed;
                }
            }
        }
        impl Tx<'_> {
            $(fn $touch(&mut self, key: &$key) {
                let state = &*self.state;
                self.before
                    .$name
                    .entry(key.clone())
                    .or_insert_with(|| state.$($path).+.get(key).cloned());
            })*
        }
    };
}
journal! {
    actors / touch_actor: ActorId => Actor, actors;
    inventories / touch_inventory: InventoryId => Inventory, inventories;
    carried / touch_carried: ActorId => InventoryId, carried;
    wallets / touch_wallet: WalletId => Wallet, wallets;
    quests / touch_quest: QuestId => quests::Progress, quests;
    relationships / touch_relationship: RelationshipKey => Relationship, relationships;
    interactions / touch_interaction: InteractionKey => Interaction, interactions;
    histories / touch_history: HistoryKey => History, histories;
    conversations / touch_conversation: ConversationKey => Conversation, conversations;
    objects / touch_object: ObjectId => ObjectState, world.objects;
    locations / touch_location: ActorId => LocationState, world.locations;
    triggers / touch_trigger: TriggerId => TriggerState, world.triggers;
    movements / touch_movement: ActorId => Movement, world.movements;
    variables / touch_variable: VariableKey => Value, variables;
}

pub(crate) struct Tx<'a> {
    state: &'a mut SessionState,
    pub(crate) before: Before,
}
impl std::ops::Deref for Tx<'_> {
    type Target = SessionState;
    fn deref(&self) -> &SessionState {
        self.state
    }
}
/// An enclosing scope set aside while a nested one runs; see `Tx::savepoint`.
pub(crate) struct Savepoint(Before);

impl<'a> Tx<'a> {
    pub fn begin(state: &'a mut SessionState) -> Self {
        let before = Before::new(state);
        Self { state, before }
    }
    pub fn rollback(self) {
        self.before.restore(self.state);
    }
    /// Start a nested scope that can be undone on its own, keeping earlier work of the command.
    pub fn savepoint(&mut self) -> Savepoint {
        let fresh = Before::new(self.state);
        Savepoint(std::mem::replace(&mut self.before, fresh))
    }
    pub fn release(&mut self, savepoint: Savepoint) {
        let nested = std::mem::replace(&mut self.before, savepoint.0);
        nested.merge_into(&mut self.before);
    }
    pub fn rollback_to(&mut self, savepoint: Savepoint) {
        let nested = std::mem::replace(&mut self.before, savepoint.0);
        nested.restore(self.state);
    }
    pub fn set_time(&mut self, time: GameTime) {
        self.state.time = time;
    }
    pub fn random_mut(&mut self) -> &mut RandomState {
        &mut self.state.random
    }
    pub fn set_narrative_random(&mut self, random: RandomState) {
        self.state.narrative_random = random;
    }
    pub fn bump_generation(&mut self) -> Result<()> {
        self.state.generation = self
            .state
            .generation
            .checked_add(1)
            .filter(|g| *g <= i64::MAX as u64)
            .ok_or_else(|| Invalid("session generation overflow".into()))?;
        Ok(())
    }
    pub fn actor_mut(&mut self, id: ActorId) -> Result<&mut Actor> {
        self.state.actor(id)?;
        self.touch_actor(&id);
        Ok(self.state.actors.get_mut(&id).expect("checked actor"))
    }
    pub fn inventory_mut(&mut self, id: InventoryId) -> Result<&mut Inventory> {
        self.state.inventory(id)?;
        self.touch_inventory(&id);
        Ok(self
            .state
            .inventories
            .get_mut(&id)
            .expect("checked inventory"))
    }
    /// Adds an inventory that did not exist: one opened for the first time.
    pub fn put_inventory(&mut self, inventory: Inventory) {
        let id = inventory.id;
        self.touch_inventory(&id);
        if inventory.owner.kind == "actor" && inventory.role == CARRIED {
            let actor = ActorId(inventory.owner.id.0);
            self.touch_carried(&actor);
            self.state.carried.insert(actor, id);
        }
        self.state.inventories.insert(id, inventory);
    }
    pub fn wallet_mut(&mut self, id: WalletId) -> Result<&mut Wallet> {
        self.state.wallet(id)?;
        self.touch_wallet(&id);
        Ok(self.state.wallets.get_mut(&id).expect("checked wallet"))
    }
    pub fn quest_mut(&mut self, id: QuestId) -> &mut quests::Progress {
        self.touch_quest(&id);
        self.state
            .quests
            .entry(id)
            .or_insert_with(|| quests::Progress::new(id))
    }
    pub fn relationship_mut(&mut self, key: RelationshipKey) -> &mut Relationship {
        self.touch_relationship(&key);
        self.state
            .relationships
            .entry(key)
            .or_insert_with(|| Relationship::neutral(key))
    }
    pub fn interaction_mut(&mut self, key: InteractionKey) -> &mut Interaction {
        self.touch_interaction(&key);
        self.state
            .interactions
            .entry(key)
            .or_insert(Interaction { key, current: None })
    }
    pub fn history_mut(&mut self, key: HistoryKey) -> &mut History {
        self.touch_history(&key);
        self.state
            .histories
            .entry(key)
            .or_insert_with(|| History::empty(key))
    }
    pub fn conversation_mut(&mut self, key: ConversationKey) -> Result<&mut Conversation> {
        self.state.conversation(key)?;
        self.touch_conversation(&key);
        Ok(self
            .state
            .conversations
            .get_mut(&key)
            .expect("checked conversation"))
    }
    pub fn put_conversation(&mut self, conversation: Conversation) {
        let key = ConversationKey::of(&conversation);
        self.touch_conversation(&key);
        self.state.conversations.insert(key, conversation);
    }
    pub fn object_mut(&mut self, content: &GameContent, id: ObjectId) -> Result<&mut ObjectState> {
        let current = self.state.object(content, id)?;
        self.touch_object(&id);
        Ok(self.state.world.objects.entry(id).or_insert(current))
    }
    pub fn set_areas(&mut self, actor: ActorId, areas: BTreeSet<AreaId>) {
        self.touch_location(&actor);
        self.state
            .world
            .locations
            .insert(actor, LocationState { actor, areas });
    }
    pub fn set_movement(&mut self, movement: Movement) {
        self.touch_movement(&movement.actor);
        self.state.world.movements.insert(movement.actor, movement);
    }
    pub fn remove_movement(&mut self, actor: ActorId) -> Option<Movement> {
        self.touch_movement(&actor);
        self.state.world.movements.remove(&actor)
    }
    /// Announces something the changed records alone do not show, such as an arrival.
    pub fn signal(&mut self, signal: WorldSignal) {
        self.before.signals.push(signal);
    }
    pub fn put_trigger(&mut self, progress: TriggerState) {
        self.touch_trigger(&progress.id);
        self.state.world.triggers.insert(progress.id, progress);
    }
    pub fn set_variable(&mut self, key: VariableKey, value: Value) {
        self.touch_variable(&key);
        self.state.variables.insert(key, value);
    }
    pub fn party_mut(&mut self) -> &mut Party {
        if self.before.party.is_none() {
            self.before.party = Some(self.state.party.clone());
        }
        &mut self.state.party
    }
    /// Characters with an effect that will tick or end.
    pub fn timed_mut(&mut self) -> &mut BTreeSet<ActorId> {
        if self.before.timed.is_none() {
            self.before.timed = Some(self.state.timed.clone());
        }
        &mut self.state.timed
    }
    /// Claims are never withdrawn, so undoing one only ever removes it.
    pub fn claim(&mut self, key: ClaimKey) {
        if self.state.claims.insert(key) {
            self.before.claims.insert(key);
        }
    }
    pub fn pending_mut(&mut self) -> &mut VecDeque<Pending> {
        if self.before.pending.is_none() {
            self.before.pending = Some(self.state.world.pending.clone());
        }
        &mut self.state.world.pending
    }
}
