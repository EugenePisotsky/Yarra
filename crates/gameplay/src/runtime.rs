//! Content port and shared identities. Mutable state is held in memory; see `state`.
use crate::Result;
use crate::*;
use game_types::*;
use rules::RandomState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Dialogue graphs kept loaded at once. Older graphs are reloaded on demand.
pub const MAX_LOADED_DIALOGUES: usize = 32;
pub const MAX_EVENTS_PER_COMMAND: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentIdentity {
    pub manifest: ContentManifest,
    pub fingerprint: [u8; 32],
}
/// Summary of a playthrough's position, returned by commands and stored beside saves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHeader {
    pub playthrough: PlaythroughId,
    pub generation: u64,
    pub time: GameTime,
    pub random: RandomState,
    pub narrative_random: RandomState,
}
impl SessionHeader {
    pub fn capture(state: &SessionState) -> Self {
        Self {
            playthrough: state.playthrough,
            generation: state.generation,
            time: state.time,
            random: state.random,
            narrative_random: state.narrative_random,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConversationKey {
    pub dialogue: DialogueId,
    pub participant: ActorId,
    pub speaker: ActorId,
}
impl ConversationKey {
    pub fn of(c: &dialogue::Conversation) -> Self {
        Self {
            dialogue: c.dialogue,
            participant: c.participant,
            speaker: c.speaker,
        }
    }
}
/// One conversation graph with the conditions and actions local to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialoguePack {
    pub graph: dialogue::Dialogue,
    pub conditions: BTreeMap<BindingId, Condition>,
    pub actions: BTreeMap<BindingId, Action>,
}
/// Published content. `core` is read once per session; dialogue graphs are read when a
/// conversation needs them, so opening a game never loads every conversation.
pub trait ContentSource {
    fn identity(&self) -> ContentIdentity;
    /// Every definition except dialogue graphs, their local bindings and text contracts.
    fn core(&mut self) -> Result<GameContent>;
    fn dialogue(&mut self, id: DialogueId) -> Result<DialoguePack>;
}
