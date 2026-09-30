//! The whole mutable playthrough, held in memory. Records that were never changed are absent
//! and read as their defaults, so a save only contains what differs from the authored world.
use crate::actors::{Actor, Relationship, RelationshipKey};
use crate::dialogue::{ClaimKey, Conversation, History, HistoryKey, Interaction, InteractionKey};
use crate::inventory::{Inventory, Wallet};
use crate::rules::{Attributes, RandomState};
use crate::{
    ConversationKey, GameContent, LocationState, ObjectKind, ObjectState, Result, TriggerState,
    quests,
};
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A record stored in a map under an identity it also carries.
pub trait Keyed {
    type Key: Ord + Clone;
    fn key(&self) -> Self::Key;
}
macro_rules! keyed {
    ($($value:ty => $key:ty, |$v:ident| $expr:expr;)*) => {$(
        impl Keyed for $value {
            type Key = $key;
            fn key(&self) -> $key { let $v = self; $expr }
        }
    )*};
}
keyed! {
    Actor => ActorId, |v| v.id;
    Inventory => InventoryId, |v| v.id;
    Wallet => WalletId, |v| v.id;
    quests::Progress => QuestId, |v| v.quest;
    Relationship => RelationshipKey, |v| v.key;
    Interaction => InteractionKey, |v| v.key;
    History => HistoryKey, |v| v.key;
    Conversation => ConversationKey, |v| ConversationKey::of(v);
    ObjectState => ObjectId, |v| v.id;
    LocationState => ActorId, |v| v.actor;
    TriggerState => TriggerId, |v| v.id;
}
/// Saved as a plain list of records; duplicate identities are rejected on load.
pub(crate) mod keyed {
    use super::Keyed;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
    use std::collections::BTreeMap;
    pub fn serialize<S: Serializer, K, V: Serialize>(
        map: &BTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(map.values())
    }
    pub fn deserialize<'de, D: Deserializer<'de>, V: Keyed + Deserialize<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<V::Key, V>, D::Error> {
        let mut map = BTreeMap::new();
        for value in Vec::<V>::deserialize(deserializer)? {
            if map.insert(value.key(), value).is_some() {
                return Err(D::Error::custom("duplicate record identity"));
            }
        }
        Ok(map)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionState {
    pub playthrough: PlaythroughId,
    pub generation: u64,
    pub time: GameTime,
    pub random: RandomState,
    pub narrative_random: RandomState,
    #[serde(with = "keyed")]
    pub actors: BTreeMap<ActorId, Actor>,
    /// Inventory and wallet owners that are not actors, e.g. a party or a chest.
    pub owners: BTreeSet<OwnerRef>,
    #[serde(with = "keyed")]
    pub inventories: BTreeMap<InventoryId, Inventory>,
    /// Each actor's carried inventory. Maintained by `add_inventory`, checked by `validate`.
    pub carried: BTreeMap<ActorId, InventoryId>,
    #[serde(with = "keyed")]
    pub wallets: BTreeMap<WalletId, Wallet>,
    #[serde(with = "keyed")]
    pub quests: BTreeMap<QuestId, quests::Progress>,
    #[serde(with = "keyed")]
    pub relationships: BTreeMap<RelationshipKey, Relationship>,
    #[serde(with = "keyed")]
    pub interactions: BTreeMap<InteractionKey, Interaction>,
    #[serde(with = "keyed")]
    pub histories: BTreeMap<HistoryKey, History>,
    pub claims: BTreeSet<ClaimKey>,
    #[serde(with = "keyed")]
    pub conversations: BTreeMap<ConversationKey, Conversation>,
    pub facts: BTreeSet<Key>,
    /// Characters travelling together. They take part in every conversation any of them has.
    pub party: BTreeSet<ActorId>,
    pub world: crate::WorldState,
}
pub const CARRIED: &str = "carried";

impl SessionState {
    pub fn empty(seed: u64) -> Self {
        Self {
            playthrough: PlaythroughId::new(),
            generation: 1,
            time: GameTime::default(),
            random: RandomState(seed),
            narrative_random: RandomState(seed ^ 0xd1b54a32d192ed03),
            actors: BTreeMap::new(),
            owners: BTreeSet::new(),
            inventories: BTreeMap::new(),
            carried: BTreeMap::new(),
            wallets: BTreeMap::new(),
            quests: BTreeMap::new(),
            relationships: BTreeMap::new(),
            interactions: BTreeMap::new(),
            histories: BTreeMap::new(),
            claims: BTreeSet::new(),
            conversations: BTreeMap::new(),
            facts: BTreeSet::new(),
            party: BTreeSet::new(),
            world: Default::default(),
        }
    }
    pub fn add_actor(&mut self, actor: Actor) {
        self.actors.insert(actor.id, actor);
    }
    pub fn add_inventory(&mut self, inventory: Inventory) {
        if inventory.owner.kind == "actor" && inventory.role == CARRIED {
            self.carried
                .insert(ActorId(inventory.owner.id.0), inventory.id);
        }
        self.inventories.insert(inventory.id, inventory);
    }
    pub fn add_wallet(&mut self, wallet: Wallet) {
        self.wallets.insert(wallet.id, wallet);
    }
    pub fn actor(&self, id: ActorId) -> Result<&Actor> {
        self.actors
            .get(&id)
            .ok_or_else(|| Invalid("unknown actor".into()).into())
    }
    pub fn inventory(&self, id: InventoryId) -> Result<&Inventory> {
        self.inventories
            .get(&id)
            .ok_or_else(|| Invalid("unknown inventory".into()).into())
    }
    pub fn wallet(&self, id: WalletId) -> Result<&Wallet> {
        self.wallets
            .get(&id)
            .ok_or_else(|| Invalid("unknown wallet".into()).into())
    }
    pub fn carried(&self, actor: ActorId) -> Result<&Inventory> {
        self.carried
            .get(&actor)
            .and_then(|id| self.inventories.get(id))
            .ok_or_else(|| Invalid("actor has no carried inventory".into()).into())
    }
    pub fn conversation(&self, key: ConversationKey) -> Result<&Conversation> {
        self.conversations
            .get(&key)
            .ok_or_else(|| Invalid("conversation has not started".into()).into())
    }
    /// An unstarted quest has no record.
    pub fn quest(&self, id: QuestId) -> quests::Progress {
        self.quests
            .get(&id)
            .cloned()
            .unwrap_or_else(|| quests::Progress::new(id))
    }
    /// Actors who never interacted are neutral.
    pub fn relationship(&self, key: RelationshipKey) -> Relationship {
        self.relationships
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Relationship::neutral(key))
    }
    pub fn selection(&self, key: InteractionKey) -> Option<&crate::dialogue::Selection> {
        self.interactions.get(&key)?.current.as_ref()
    }
    pub fn history(&self, key: HistoryKey) -> History {
        self.histories
            .get(&key)
            .cloned()
            .unwrap_or_else(|| History::empty(key))
    }
    pub fn claimed(&self, key: ClaimKey) -> bool {
        self.claims.contains(&key)
    }
    /// An untouched object is in its authored state, whether or not its region is loaded.
    pub fn object(&self, content: &GameContent, id: ObjectId) -> Result<ObjectState> {
        match self.world.objects.get(&id) {
            Some(state) => Ok(state.clone()),
            None => Ok(ObjectState::initial(content.object(id)?)),
        }
    }
    /// Before the first reported observation, occupancy follows the actor's saved position.
    pub fn location(&self, content: &GameContent, actor: ActorId) -> Result<LocationState> {
        match self.world.locations.get(&actor) {
            Some(state) => Ok(state.clone()),
            None => Ok(LocationState {
                areas: content.areas_at(&self.actor(actor)?.position)?,
                ..LocationState::initial(actor)
            }),
        }
    }
    pub fn trigger(&self, id: TriggerId) -> TriggerState {
        self.world
            .triggers
            .get(&id)
            .cloned()
            .unwrap_or_else(|| TriggerState::initial(id))
    }
    pub fn derived(&self, content: &GameContent, actor: ActorId) -> Result<Attributes> {
        let actor = self.actor(actor)?;
        let carried = self.carried(actor.id)?;
        let mut modifiers = Vec::new();
        let mut ids = BTreeSet::new();
        for (slot, item) in &actor.equipment {
            require(ids.insert(*item), "item equipped more than once")?;
            let definition = content.items.item(carried.entry(*item)?.definition)?;
            require(
                definition.mechanics.slot.as_ref() == Some(slot),
                "incompatible equipment slot",
            )?;
            modifiers.extend(&definition.mechanics.modifiers);
        }
        modifiers.extend(
            actor
                .effects
                .iter()
                .filter(|e| e.expires_at > self.time)
                .map(|e| &e.modifier),
        );
        Ok(content.game.rules.derive(&actor.base, modifiers)?)
    }
    fn valid_owner(&self, owner: &OwnerRef) -> bool {
        if owner.kind == "actor" {
            self.actors.contains_key(&ActorId(owner.id.0))
        } else {
            self.owners.contains(owner)
        }
    }
    pub(crate) fn check_actor(&self, content: &GameContent, actor: &Actor) -> Result<()> {
        content.template(actor.template)?;
        actor.validate(&content.game.rules)?;
        let stats = self.derived(content, actor.id)?;
        require(
            actor.health <= stats[&content.game.rules.health_attribute] as u32,
            "health exceeds derived maximum",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_inventory(&self, content: &GameContent, inv: &Inventory) -> Result<()> {
        inv.validate(&content.items)?;
        require(
            self.valid_owner(&inv.owner),
            "invalid inventory identity/owner/role",
        )?;
        if inv.owner.kind == "actor" && inv.role == CARRIED {
            require(
                self.carried.get(&ActorId(inv.owner.id.0)) == Some(&inv.id),
                "carried inventory index is stale",
            )?;
        }
        Ok(())
    }
    pub(crate) fn check_wallet(&self, wallet: &Wallet) -> Result<()> {
        wallet.validate()?;
        require(
            self.valid_owner(&wallet.owner),
            "invalid wallet identity/owner",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_history(&self, content: &GameContent, h: &History) -> Result<()> {
        h.validate(content.dialogue_contract(h.key.dialogue)?, self.time)?;
        require(
            h.key
                .scope
                .actors()
                .iter()
                .all(|id| self.actors.contains_key(id)),
            "invalid history actor",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_claim(&self, content: &GameContent, key: ClaimKey) -> Result<()> {
        key.scope.validate()?;
        require(
            content.claim(key.claim)?.scope.accepts(key.scope),
            "invalid claim identity/scope",
        )?;
        require(
            key.scope
                .actors()
                .iter()
                .all(|id| self.actors.contains_key(id)),
            "invalid claim actor",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_relationship(&self, r: &Relationship) -> Result<()> {
        r.validate()?;
        require(
            self.actors.contains_key(&r.key.from) && self.actors.contains_key(&r.key.to),
            "invalid relationship actors/identity",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_interaction(&self, content: &GameContent, i: &Interaction) -> Result<()> {
        require(
            i.key.participant != i.key.speaker
                && self.actors.contains_key(&i.key.participant)
                && self.actors.contains_key(&i.key.speaker),
            "invalid interaction actors/identity",
        )?;
        if let Some(selection) = &i.current {
            let profile = content.profile(selection.profile)?;
            require(
                profile.rules.iter().any(|r| {
                    r.id == selection.rule
                        && r.variants.iter().any(|v| v.dialogue == selection.dialogue)
                }),
                "invalid saved interaction selection",
            )?;
            require(
                self.conversations.contains_key(&ConversationKey {
                    dialogue: selection.dialogue,
                    participant: i.key.participant,
                    speaker: i.key.speaker,
                }),
                "selected conversation is missing",
            )?;
        }
        Ok(())
    }
    /// A graph that is not loaded is checked against its always-present contract.
    pub(crate) fn check_conversation(&self, content: &GameContent, c: &Conversation) -> Result<()> {
        require(
            self.actors.contains_key(&c.participant) && self.actors.contains_key(&c.speaker),
            "invalid conversation participants/identity",
        )?;
        match content.loaded_dialogue(c.dialogue) {
            Some(graph) => c.validate(graph)?,
            None => c.validate_contract(content.dialogue_contract(c.dialogue)?)?,
        }
        require(
            c.bindings.values().all(|id| self.actors.contains_key(id)),
            "unknown bound actor",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_object(&self, content: &GameContent, o: &ObjectState) -> Result<()> {
        let definition = content.object(o.id)?;
        if let ObjectKind::Container { inventory } = definition.kind
            && let Some(bag) = self.inventories.get(&inventory)
        {
            require(
                bag.owner == OwnerRef::new("object", OwnerId(o.id.0))? && bag.role == "contents",
                "container inventory ownership mismatch",
            )?;
        }
        require(
            !(o.open && (o.locked || o.destroyed)),
            "invalid object state",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_location(&self, content: &GameContent, l: &LocationState) -> Result<()> {
        self.actor(l.actor)?;
        require(
            l.areas.len() <= crate::MAX_AREA_OVERLAP
                && l.observation <= i64::MAX as u64
                && (l.observation == 0) == l.last_observed.is_none(),
            "invalid location state",
        )?;
        for id in &l.areas {
            content.area(*id)?;
        }
        Ok(())
    }
    /// Checks every record and the invariants that span records. Used when a playthrough is
    /// created or loaded; commands only re-check the records they changed.
    pub fn validate(&self, content: &GameContent) -> Result<()> {
        require(
            self.generation > 0 && self.generation <= i64::MAX as u64,
            "invalid session generation",
        )?;
        for owner in &self.owners {
            owner.validate()?;
            require(owner.kind != "actor", "invalid external owner")?;
        }
        let mut roles = BTreeSet::new();
        let mut entries = BTreeSet::new();
        for (id, inv) in &self.inventories {
            require(*id == inv.id, "inventory identity mismatch")?;
            self.check_inventory(content, inv)?;
            require(
                roles.insert((&inv.owner, &inv.role)),
                "invalid inventory identity/owner/role",
            )?;
            for item in &inv.entries {
                require(entries.insert(item.id), "duplicate global item identity")?;
            }
        }
        for (actor, id) in &self.carried {
            let inv = self.inventory(*id)?;
            require(
                inv.owner == OwnerRef::actor(*actor) && inv.role == CARRIED,
                "carried inventory index is stale",
            )?;
        }
        for (id, actor) in &self.actors {
            require(*id == actor.id, "actor identity mismatch")?;
            self.check_actor(content, actor)?;
        }
        for wallet in self.wallets.values() {
            self.check_wallet(wallet)?;
        }
        for h in self.histories.values() {
            self.check_history(content, h)?;
        }
        for key in &self.claims {
            self.check_claim(content, *key)?;
        }
        for q in self.quests.values() {
            q.validate(content.quest(q.quest)?)?;
        }
        for r in self.relationships.values() {
            self.check_relationship(r)?;
        }
        for i in self.interactions.values() {
            self.check_interaction(content, i)?;
        }
        for c in self.conversations.values() {
            self.check_conversation(content, c)?;
        }
        require(
            self.facts.is_subset(&content.game.facts),
            "unknown saved fact",
        )?;
        require(
            self.party.len() <= 16 && self.party.iter().all(|id| self.actors.contains_key(id)),
            "invalid party",
        )?;
        self.world.validate(content, self)
    }
}
