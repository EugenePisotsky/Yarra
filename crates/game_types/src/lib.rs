//! Small, durable contracts shared by gameplay domains. No runtime services.
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct Invalid(pub String);
pub type Result<T> = std::result::Result<T, Invalid>;
pub fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Invalid(message.into()))
    }
}

/// Identities are 16 bytes. Authored content names things (`guard/gate`) and the name is
/// hashed into those bytes; things created at runtime get random bytes. A name therefore
/// *is* the identity: renaming something makes it a different thing.
pub mod names {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    /// FNV-1a over 128 bits. Usable in constants, so code can name content it relies on.
    pub const fn hash(name: &str) -> [u8; 16] {
        let bytes = name.as_bytes();
        let mut hash: u128 = 0x6c62272e07bb014262b821756295c58d;
        let mut i = 0;
        while i < bytes.len() {
            hash ^= bytes[i] as u128;
            hash = hash.wrapping_mul(0x0000000001000000000000000000013b);
            i += 1;
        }
        hash.to_be_bytes()
    }
    pub fn valid(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 96
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-/.".contains(&b))
    }
    fn table() -> &'static Mutex<HashMap<[u8; 16], Box<str>>> {
        static TABLE: OnceLock<Mutex<HashMap<[u8; 16], Box<str>>>> = OnceLock::new();
        TABLE.get_or_init(Default::default)
    }
    /// Remembers which name produced an identity, so messages and saved files can show it.
    /// Two different names with the same hash are reported instead of silently merging.
    pub fn remember(name: &str) -> Result<[u8; 16], String> {
        if !valid(name) {
            return Err(format!("invalid name or UUID: {name}"));
        }
        let id = hash(name);
        let mut table = table().lock().unwrap_or_else(|e| e.into_inner());
        match table.get(&id) {
            Some(known) if &**known != name => {
                Err(format!("names {known} and {name} have the same identity"))
            }
            Some(_) => Ok(id),
            None => {
                table.insert(id, name.into());
                Ok(id)
            }
        }
    }
    thread_local! {
        static RAW: Cell<bool> = const { Cell::new(false) };
    }
    /// Runs `work` with every identity written in UUID form, for output that must come out
    /// the same whichever names this process happens to have met.
    pub fn raw<T>(work: impl FnOnce() -> T) -> T {
        let before = RAW.replace(true);
        let result = work();
        RAW.set(before);
        result
    }
    /// The name behind an identity, if one is known and names are being shown.
    pub fn lookup(id: &[u8; 16]) -> Option<String> {
        if RAW.get() {
            return None;
        }
        let table = table().lock().unwrap_or_else(|e| e.into_inner());
        table.get(id).map(|name| name.to_string())
    }
}

macro_rules! ids {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(pub [u8; 16]);
        impl $name {
            /// A fresh identity for something created while playing.
            pub fn new() -> Self { Self(*uuid::Uuid::new_v4().as_bytes()) }
            /// The identity of authored content with this name.
            pub const fn named(name: &str) -> Self { Self(names::hash(name)) }
            /// The bytes in UUID form, whether or not a name is known for them.
            pub fn raw(&self) -> String { uuid::Uuid::from_bytes(self.0).to_string() }
        }
        impl Default for $name { fn default() -> Self { Self::new() } }
        impl TryFrom<String> for $name {
            type Error = Invalid;
            /// Accepts a name, or the UUID form used for identities without one.
            fn try_from(value: String) -> Result<Self> {
                if let Ok(id) = uuid::Uuid::parse_str(&value) {
                    return Ok(Self(*id.as_bytes()));
                }
                names::remember(&value).map(Self).map_err(Invalid)
            }
        }
        impl From<$name> for String { fn from(value: $name) -> Self { value.to_string() } }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match names::lookup(&self.0) {
                    Some(name) => f.write_str(&name),
                    None => uuid::Uuid::from_bytes(self.0).fmt(f),
                }
            }
        }
    )+};
}
ids!(
    CatalogId,
    CategoryId,
    ItemDefinitionId,
    InventoryId,
    ItemId,
    WalletId,
    OwnerId,
    ActorId,
    ActorTemplateId,
    DialogueId,
    QuestId,
    ClaimId,
    ObjectId,
    AreaId,
    TriggerId,
    InteractionProfileId,
    PredicateId,
    TextResourceId,
    PackageId,
    ContentId,
    PlaythroughId,
    VariableId,
    LootId
);

/// A stable semantic key, independent of translated labels.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Key(String);
impl Key {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        require(
            !value.is_empty()
                && value.len() <= 96
                && value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b)),
            "invalid semantic key",
        )?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for Key {
    type Error = Invalid;
    fn try_from(v: String) -> Result<Self> {
        Self::new(v)
    }
}
impl From<Key> for String {
    fn from(v: Key) -> Self {
        v.0
    }
}

/// Fluent message ID, not an editable item/actor key or rendered label.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TextKey(String);
impl TextKey {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        require(
            !value.is_empty()
                && value.len() <= 128
                && value.as_bytes()[0].is_ascii_alphabetic()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "invalid Fluent message ID",
        )?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for TextKey {
    type Error = Invalid;
    fn try_from(v: String) -> Result<Self> {
        Self::new(v)
    }
}
impl From<TextKey> for String {
    fn from(v: TextKey) -> Self {
        v.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextRef {
    Message(MessageRef),
    Literal(String),
}
impl TextRef {
    pub fn message(resource: TextResourceId, key: impl Into<String>) -> Result<Self> {
        Ok(Self::Message(MessageRef {
            resource,
            key: TextKey::new(key)?,
        }))
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Message(_) => Ok(()),
            Self::Literal(s) => require(s.len() <= 8192, "text exceeds 8192 bytes"),
        }
    }
}
impl From<String> for TextRef {
    fn from(v: String) -> Self {
        Self::Literal(v)
    }
}
impl From<&str> for TextRef {
    fn from(v: &str) -> Self {
        Self::Literal(v.into())
    }
}

/// An owner can be an actor, chest, party or another registered world object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OwnerRef {
    pub kind: String,
    pub id: OwnerId,
}
impl OwnerRef {
    pub fn new(kind: impl Into<String>, id: OwnerId) -> Result<Self> {
        let value = Self {
            kind: kind.into(),
            id,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn actor(id: ActorId) -> Self {
        Self {
            kind: "actor".into(),
            id: OwnerId(id.0),
        }
    }
    pub fn validate(&self) -> Result<()> {
        Key::new(self.kind.clone()).map(|_| ())
    }
}

/// Logical milliseconds, independent of wall-clock time and rendering frames.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GameTime(pub u64);
impl GameTime {
    pub fn advance(self, millis: u64) -> Result<Self> {
        self.0
            .checked_add(millis)
            .map(Self)
            .ok_or_else(|| Invalid("game time overflow".into()))
    }
}

/// Deserialize authored semantic maps without silently replacing duplicate keys.
pub fn deserialize_key_map<'de, D, V>(
    deserializer: D,
) -> std::result::Result<std::collections::BTreeMap<Key, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct UniqueMap<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for UniqueMap<V> {
        type Value = std::collections::BTreeMap<Key, V>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a map with unique semantic keys")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut entries: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut result = std::collections::BTreeMap::new();
            while let Some((key, value)) = entries.next_entry::<Key, V>()? {
                if result.contains_key(&key) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate semantic key {}",
                        key.as_str()
                    )));
                }
                result.insert(key, value);
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(UniqueMap(std::marker::PhantomData))
}

/// Stable resource scope and local Fluent key. Filesystem paths are never identities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageRef {
    pub resource: TextResourceId,
    pub key: TextKey,
}
impl MessageRef {
    pub fn as_str(&self) -> &str {
        self.key.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgumentType {
    Text,
    Number,
    Select(std::collections::BTreeSet<String>),
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageContract {
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub arguments: std::collections::BTreeMap<String, ArgumentType>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextContract {
    pub id: TextResourceId,
    pub imports: std::collections::BTreeSet<TextResourceId>,
    #[serde(deserialize_with = "deserialize_unique_map")]
    pub messages: std::collections::BTreeMap<TextKey, MessageContract>,
}
impl TextContract {
    pub fn validate(&self) -> Result<()> {
        require(
            !self.imports.contains(&self.id),
            "a text resource imports itself",
        )?;
        for contract in self.messages.values() {
            for (name, kind) in &contract.arguments {
                TextKey::new(name)?;
                if let ArgumentType::Select(values) = kind {
                    require(!values.is_empty(), "a selector has values")?;
                    for value in values {
                        TextKey::new(value)?;
                    }
                }
            }
        }
        Ok(())
    }
}
/// Reject duplicate keys instead of silently losing authoring data during deserialization.
pub fn deserialize_unique_map<'de, D, K, V>(
    deserializer: D,
) -> std::result::Result<std::collections::BTreeMap<K, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct Unique<K, V>(std::marker::PhantomData<(K, V)>);
    impl<'de, K: Deserialize<'de> + Ord, V: Deserialize<'de>> serde::de::Visitor<'de> for Unique<K, V> {
        type Value = std::collections::BTreeMap<K, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a map with unique keys")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut result = std::collections::BTreeMap::new();
            while let Some((key, value)) = map.next_entry()? {
                if result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate map key"));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Unique(std::marker::PhantomData))
}

/// Presentation-ready references and typed values. Mechanics never render a locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundArgument {
    Text(TextRef),
    Number(i32),
    Select(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundText {
    pub text: TextRef,
    pub arguments: std::collections::BTreeMap<String, BoundArgument>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_name_is_an_identity_and_is_shown_as_written() {
        const GATE: QuestId = QuestId::named("guard/gate");
        let parsed = QuestId::try_from("guard/gate".to_owned()).unwrap();
        assert_eq!(parsed, GATE);
        assert_eq!(parsed.to_string(), "guard/gate");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"guard/gate\"");
        assert_ne!(GATE, QuestId::named("guard/gates"));
        // The same name means the same bytes for every kind of identity.
        assert_eq!(ActorId::named("hero").0, OwnerId::named("hero").0);
    }
    #[test]
    fn raw_output_shows_bytes_even_for_known_names() {
        let id = QuestId::try_from("guard/raw".to_owned()).unwrap();
        let raw = names::raw(|| serde_json::to_string(&id).unwrap());
        assert_eq!(raw, format!("\"{}\"", id.raw()));
        assert_eq!(id.to_string(), "guard/raw");
    }
    #[test]
    fn identities_without_a_name_keep_their_uuid_form() {
        let text = "0a0b0c0d-0102-0304-0506-0708090a0b0c";
        let id = ItemId::try_from(text.to_owned()).unwrap();
        assert_eq!(id.to_string(), text);
        assert_eq!(id.raw(), text);
        let fresh = ItemId::new();
        assert_eq!(ItemId::try_from(fresh.to_string()).unwrap(), fresh);
        assert_eq!(QuestId::named("guard/gate").raw().len(), 36);
    }
    #[test]
    fn malformed_names_are_rejected() {
        for bad in ["", "Bad Id", "UPPER", "spaced name", &"x".repeat(97)] {
            let error = QuestId::try_from(bad.to_owned()).unwrap_err().to_string();
            assert!(error.contains("invalid name or UUID"), "{error}");
        }
    }
}
