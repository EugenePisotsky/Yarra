//! Records kept in maps under an identity they also carry, and saved as plain lists: the
//! playthrough's state and the content's definitions alike.
use crate::actors::{Actor, ActorTemplate, Relationship, RelationshipKey};
use crate::dialogue::{
    Conversation, Dialogue, DialogueContract, History, HistoryKey, Interaction, InteractionKey,
};
use crate::inventory::{Category, Inventory, ItemDefinition, LootTable, Wallet};
use crate::*;
use game_types::*;
use std::collections::BTreeMap;

/// A record stored in a map under an identity it also carries.
pub trait Keyed {
    type Key: Ord + Clone;
    fn key(&self) -> Self::Key;
}
/// A map of records by their identity.
pub trait KeyedMap<V: Keyed> {
    /// Adds the record, replacing one with the same identity.
    fn add(&mut self, value: V) -> Option<V>;
}
impl<V: Keyed> KeyedMap<V> for BTreeMap<V::Key, V> {
    fn add(&mut self, value: V) -> Option<V> {
        self.insert(value.key(), value)
    }
}
/// Records filed by their identities; a later one replaces an earlier with the same.
pub fn keyed_map<V: Keyed>(records: impl IntoIterator<Item = V>) -> BTreeMap<V::Key, V> {
    records.into_iter().map(|v| (v.key(), v)).collect()
}
macro_rules! keyed {
    ($($value:ty => $key:ty, |$v:ident| $expr:expr;)*) => {$(
        impl Keyed for $value {
            type Key = $key;
            fn key(&self) -> $key { let $v = self; $expr }
        }
    )*};
}
// The playthrough's records.
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
    Movement => ActorId, |v| v.actor;
    TriggerState => TriggerId, |v| v.id;
}
// The content's definitions.
keyed! {
    TextContract => TextResourceId, |v| v.id;
    Category => CategoryId, |v| v.id;
    ItemDefinition => ItemDefinitionId, |v| v.id;
    LootTable => LootId, |v| v.id;
    ObjectDefinition => ObjectId, |v| v.id;
    TriggerDefinition => TriggerId, |v| v.id;
    DialogueContract => DialogueId, |v| v.id;
    Dialogue => DialogueId, |v| v.id;
    quests::Quest => QuestId, |v| v.id;
    InteractionProfile => InteractionProfileId, |v| v.id;
    NamedPredicate => PredicateId, |v| v.id;
    ActorTemplate => ActorTemplateId, |v| v.id;
    VariableDefinition => VariableId, |v| v.id;
    ScriptModule => Key, |v| v.name.clone();
}
/// Saved as a plain list of records; duplicate identities are rejected on load.
pub(crate) mod list {
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
