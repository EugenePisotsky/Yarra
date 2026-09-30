//! The whole mutable playthrough, held in memory. Records that were never changed are absent
//! and read as their defaults, so a save only contains what differs from the authored world.
use crate::actors::{Actor, Relationship, RelationshipKey};
use crate::dialogue::{ClaimKey, Conversation, History, HistoryKey, Interaction, InteractionKey};
use crate::inventory::{Inventory, Wallet};
use crate::rules::{Modifier, RandomState, Stats};
use crate::{
    ConversationKey, GameContent, LocationState, ObjectKind, ObjectState, Result, TriggerState,
    Value, quests,
};
use crate::{VariableKey, VariableScope};
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
    crate::Movement => ActorId, |v| v.actor;
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

/// A map whose keys are not plain text, saved as a list of key/value pairs.
mod pairs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
    use std::collections::BTreeMap;
    pub fn serialize<S: Serializer, K: Serialize, V: Serialize>(
        map: &BTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(map.iter())
    }
    pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        let mut map = BTreeMap::new();
        for (key, value) in Vec::<(K, V)>::deserialize(deserializer)? {
            if map.insert(key, value).is_some() {
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
    /// Variables that were set; the rest still have their initial value.
    #[serde(with = "pairs")]
    pub variables: BTreeMap<VariableKey, Value>,
    pub party: Party,
    /// Characters with an effect that will tick or end: the only ones passing time looks at.
    pub timed: BTreeSet<ActorId>,
    pub world: crate::WorldState,
}
pub const CARRIED: &str = "carried";

/// Where a change to a stat comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModifierSource {
    /// Something worn or wielded.
    Item(ItemDefinitionId),
    /// A status effect, by its name in the rules.
    Effect(Key),
}
/// The characters travelling together. They take part in every conversation any of them
/// has, and share their experience and their gold.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Party {
    pub members: BTreeSet<ActorId>,
    /// The member the player steers.
    pub controlled: Option<ActorId>,
    /// Every member is at least the level this has earned.
    pub experience: u64,
    /// The shared purse.
    pub wallet: Option<WalletId>,
}

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
            variables: BTreeMap::new(),
            party: Party::default(),
            timed: BTreeSet::new(),
            world: Default::default(),
        }
    }
    pub fn add_actor(&mut self, actor: Actor) {
        self.actors.insert(actor.id, actor);
    }
    /// Adds a character as its template describes it, with its stats worked out and its
    /// resources full.
    pub fn spawn(
        &mut self,
        content: &GameContent,
        template: ActorTemplateId,
        id: ActorId,
    ) -> Result<&mut Actor> {
        let rules = &content.game.rules;
        let mut actor = Actor::from_template(content.template(template)?, rules)?;
        actor.id = id;
        actor.stats = self.sheet(content, &actor)?;
        for (resource, maximum) in rules.resources() {
            let cap = actor.stats[maximum].max(0);
            actor.resources.insert(resource.clone(), cap);
        }
        require(!self.actors.contains_key(&id), "actor spawned twice")?;
        Ok(self.actors.entry(id).or_insert(actor))
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
    /// The areas the actor was last reported inside; none until the engine has reported.
    pub fn areas(&self, actor: ActorId) -> &BTreeSet<AreaId> {
        static NONE: BTreeSet<AreaId> = BTreeSet::new();
        self.world.locations.get(&actor).map_or(&NONE, |l| &l.areas)
    }
    /// A variable nobody has set still has its initial value, for every actor.
    pub fn variable(&self, content: &GameContent, key: impl Into<VariableKey>) -> Result<Value> {
        let key = key.into();
        self.check_variable(content, key)?;
        Ok(match self.variables.get(&key) {
            Some(value) => value.clone(),
            None => content.variable(key.variable)?.initial.clone(),
        })
    }
    pub(crate) fn check_variable(&self, content: &GameContent, key: VariableKey) -> Result<()> {
        let definition = content.variable(key.variable)?;
        require(
            (definition.scope == VariableScope::Actor) == key.actor.is_some(),
            "variable scope mismatch",
        )?;
        if let Some(actor) = key.actor {
            self.actor(actor)?;
        }
        require(
            self.variables
                .get(&key)
                .is_none_or(|value| value.same_type(&definition.initial)),
            "saved variable has the wrong type",
        )
        .map_err(Into::into)
    }
    pub fn trigger(&self, id: TriggerId) -> TriggerState {
        self.world
            .triggers
            .get(&id)
            .cloned()
            .unwrap_or_else(|| TriggerState::initial(id))
    }
    /// A stat after equipment and effects, or how much of a resource the actor has now.
    pub fn stat(&self, content: &GameContent, actor: ActorId, stat: &Key) -> Result<i32> {
        content.game.rules.stat(stat)?;
        let actor = self.actor(actor)?;
        actor
            .resources
            .get(stat)
            .or_else(|| actor.stats.get(stat))
            .copied()
            .ok_or_else(|| Invalid(format!("{} has no {}", actor.id, stat.as_str())).into())
    }
    /// What is in the party's shared purse.
    pub fn gold(&self) -> Result<u64> {
        Ok(match self.party.wallet {
            Some(wallet) => self.wallet(wallet)?.balance.units(),
            None => 0,
        })
    }
    /// What a character's equipment and effects do to its stats, each with where it comes
    /// from.
    fn modifiers<'a>(
        &self,
        content: &'a GameContent,
        actor: &Actor,
    ) -> Result<Vec<(ModifierSource, &'a Modifier)>> {
        let mut modifiers = Vec::new();
        let wear = |item: ItemDefinitionId| -> Result<&'a crate::rules::ItemMechanics> {
            Ok(&content.items.item(item)?.mechanics)
        };
        if !self.carried.contains_key(&actor.id) {
            // Nobody has looked into this character's inventory yet. What its template
            // says it wears counts all the same.
            for item in &content.template(actor.template)?.equipment {
                let source = ModifierSource::Item(*item);
                modifiers.extend(wear(*item)?.modifiers.iter().map(|m| (source.clone(), m)));
            }
        } else if !actor.equipment.is_empty() {
            let carried = self.carried(actor.id)?;
            let mut ids = BTreeSet::new();
            for (slot, item) in &actor.equipment {
                require(ids.insert(*item), "item equipped more than once")?;
                let definition = carried.entry(*item)?.definition;
                let mechanics = wear(definition)?;
                require(
                    mechanics.slot.as_ref() == Some(slot),
                    "incompatible equipment slot",
                )?;
                let source = ModifierSource::Item(definition);
                modifiers.extend(mechanics.modifiers.iter().map(|m| (source.clone(), m)));
            }
        }
        for effect in &actor.effects {
            let source = ModifierSource::Effect(effect.effect.clone());
            let definition = content.game.rules.effect(&effect.effect)?;
            modifiers.extend(definition.modifiers.iter().map(|m| (source.clone(), m)));
        }
        Ok(modifiers)
    }
    /// What changes one of a character's stats, and where each change comes from: for a
    /// tooltip that explains a number.
    pub fn modifiers_of(
        &self,
        content: &GameContent,
        actor: ActorId,
        stat: &Key,
    ) -> Result<Vec<(ModifierSource, crate::rules::Operation)>> {
        content.game.rules.stat(stat)?;
        let modifiers = self.modifiers(content, self.actor(actor)?)?;
        Ok(modifiers
            .into_iter()
            .filter(|(_, modifier)| &modifier.stat == stat)
            .map(|(source, modifier)| (source, modifier.op))
            .collect())
    }
    /// Every primary and derived stat of a character as it is built now: base values,
    /// then equipment and effects, with the derived ones from the rules script.
    pub fn sheet(&self, content: &GameContent, actor: &Actor) -> Result<Stats> {
        let rules = &content.game.rules;
        let modifiers = self.modifiers(content, actor)?;
        let modifiers = modifiers.iter().map(|(_, modifier)| *modifier);
        let mut stats = rules.effective_primaries(&actor.base, modifiers.clone())?;
        let derived = content.scripts.engine(&rules.derive)?.derive(
            &rules.derive,
            &crate::rules::Sheet {
                level: actor.level,
                class: &actor.class,
                stats: &stats,
                skills: &actor.skills,
            },
        )?;
        rules.add_derived(&mut stats, &derived, modifiers)?;
        Ok(stats)
    }
    fn valid_owner(&self, content: &GameContent, owner: &OwnerRef) -> bool {
        match owner.kind.as_str() {
            "actor" => self.actors.contains_key(&ActorId(owner.id.0)),
            // A container's contents belong to the authored object.
            "object" => content.object(ObjectId(owner.id.0)).is_ok() || self.owners.contains(owner),
            _ => self.owners.contains(owner),
        }
    }
    fn loot_random(&self, owner: [u8; 16]) -> RandomState {
        // Its own stream per owner, so what is found does not depend on what was opened
        // before it or on anything else that rolled.
        let mut hash = blake3::Hasher::new();
        hash.update(b"yarra-loot-v1");
        hash.update(&self.playthrough.0);
        hash.update(&owner);
        let bytes = hash.finalize();
        RandomState(u64::from_le_bytes(
            bytes.as_bytes()[..8].try_into().expect("eight bytes"),
        ))
    }
    /// The carried inventory a character will have once something first needs it: what its
    /// template wears, equipped, and what its loot table yields. The same whenever asked.
    pub fn unopened_inventory(
        &self,
        content: &GameContent,
        actor: &Actor,
    ) -> Result<(Inventory, BTreeMap<Key, ItemId>)> {
        let template = content.template(actor.template)?;
        let mut inventory = Inventory::new(OwnerRef::actor(actor.id), CARRIED)?;
        let mut id = blake3::Hasher::new();
        id.update(b"yarra-carried-v1");
        id.update(&actor.id.0);
        inventory
            .id
            .0
            .copy_from_slice(&id.finalize().as_bytes()[..16]);
        let mut equipment = BTreeMap::new();
        for definition in &template.equipment {
            let slot = content.items.item(*definition)?.mechanics.slot.clone();
            let slot = slot.ok_or_else(|| Invalid("template equipment has no slot".into()))?;
            let ids = inventory.grant(&content.items, *definition, 1)?;
            equipment.insert(slot, ids[0]);
        }
        if let Some(loot) = template.loot {
            let mut random = self.loot_random(actor.id.0);
            content
                .loot(loot)?
                .roll(&content.items, &mut random, &mut inventory)?;
        }
        Ok((inventory, equipment))
    }
    /// How many of an item a character carries, whether or not anything has looked into
    /// its inventory yet: what an unopened one will hold is already decided.
    pub fn item_count(
        &self,
        content: &GameContent,
        actor: ActorId,
        item: ItemDefinitionId,
    ) -> Result<u64> {
        content.items.item(item)?;
        let count = |inventory: &Inventory| {
            inventory
                .entries
                .iter()
                .filter(|e| e.definition == item)
                .map(|e| u64::from(e.quantity))
                .sum()
        };
        match self.carried.contains_key(&actor) {
            true => Ok(count(self.carried(actor)?)),
            false => Ok(count(
                &self.unopened_inventory(content, self.actor(actor)?)?.0,
            )),
        }
    }
    /// What a container holds the first time it is opened.
    pub fn unopened_container(&self, content: &GameContent, object: ObjectId) -> Result<Inventory> {
        let ObjectKind::Container {
            inventory: id,
            loot,
        } = content.object(object)?.kind
        else {
            return Err(Invalid("object is not a container".into()).into());
        };
        let mut inventory =
            Inventory::new(OwnerRef::new("object", OwnerId(object.0))?, "contents")?;
        inventory.id = id;
        if let Some(loot) = loot {
            let mut random = self.loot_random(object.0);
            content
                .loot(loot)?
                .roll(&content.items, &mut random, &mut inventory)?;
        }
        Ok(inventory)
    }
    /// `deep` works the stats out again and compares; without it only their shape and
    /// what they rest on is checked.
    pub(crate) fn check_actor(
        &self,
        content: &GameContent,
        actor: &Actor,
        deep: bool,
    ) -> Result<()> {
        content.template(actor.template)?;
        actor.validate(&content.game.rules)?;
        if deep {
            require(
                actor.stats == self.sheet(content, actor)?,
                "stats do not match what the character is built of",
            )?;
        } else {
            self.modifiers(content, actor)?;
        }
        require(
            self.timed.contains(&actor.id) == actor.next_event().is_some(),
            "list of characters with pending effects is stale",
        )?;
        if self.party.members.contains(&actor.id) {
            require(
                actor.level >= content.game.rules.level_for(self.party.experience),
                "party member below the level the party has earned",
            )?;
        }
        Ok(())
    }
    pub(crate) fn check_inventory(&self, content: &GameContent, inv: &Inventory) -> Result<()> {
        inv.validate(&content.items)?;
        require(
            self.valid_owner(content, &inv.owner),
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
    pub(crate) fn check_wallet(&self, content: &GameContent, wallet: &Wallet) -> Result<()> {
        wallet.validate()?;
        require(
            self.valid_owner(content, &wallet.owner),
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
        if let ObjectKind::Container { inventory, .. } = definition.kind
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
            l.areas.len() <= crate::MAX_AREA_OVERLAP,
            "area overlap budget exceeded",
        )?;
        for id in &l.areas {
            content.area(*id)?;
        }
        Ok(())
    }
    pub(crate) fn check_trigger(&self, content: &GameContent, t: &TriggerState) -> Result<()> {
        content.trigger(t.id)?;
        require(
            (t.fired == 0) == t.last_fired.is_none()
                && t.last_fired.is_none_or(|time| time <= self.time),
            "invalid trigger progress",
        )
        .map_err(Into::into)
    }
    pub(crate) fn check_movement(&self, content: &GameContent, m: &crate::Movement) -> Result<()> {
        self.actor(m.actor)?;
        content.area(m.to)
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
            self.check_actor(content, actor, true)?;
        }
        require(
            self.timed.iter().all(|id| self.actors.contains_key(id)),
            "unknown character with pending effects",
        )?;
        for wallet in self.wallets.values() {
            self.check_wallet(content, wallet)?;
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
        for key in self.variables.keys() {
            self.check_variable(content, *key)?;
        }
        let party = &self.party;
        require(
            party.members.len() <= content.game.rules.party_size
                && party.members.iter().all(|id| self.actors.contains_key(id))
                && party
                    .controlled
                    .is_none_or(|id| party.members.contains(&id))
                && party.experience <= i64::MAX as u64
                && party.wallet.is_none_or(|id| self.wallets.contains_key(&id)),
            "invalid party",
        )?;
        self.world.validate(content, self)
    }
}
