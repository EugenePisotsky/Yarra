//! Storage/content ports. Implementations own I/O; domain operations receive resolved records.
use crate::Result;
use crate::*;
use game_types::*;
use rules::RandomState;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_WORKING_RECORDS: usize = 64;
pub const MAX_WORKING_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const MAX_EXPIRATIONS_PER_COMMAND: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentIdentity {
    pub manifest: ContentManifest,
    pub fingerprint: [u8; 32],
}
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
    pub fn empty_state(&self) -> SessionState {
        SessionState {
            world: Default::default(),
            playthrough: self.playthrough,
            generation: self.generation,
            time: self.time,
            random: self.random,
            narrative_random: self.narrative_random,
            histories: vec![],
            claims: vec![],
            quests: vec![],
            relationships: vec![],
            interactions: vec![],
            actors: vec![],
            owners: vec![],
            inventories: vec![],
            wallets: vec![],
            conversations: vec![],
            facts: BTreeSet::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        require(
            self.generation > 0 && self.generation < i64::MAX as u64,
            "invalid session generation",
        )?;
        require(
            self.time.0 <= i64::MAX as u64,
            "logical time exceeds storage range",
        )?;
        Ok(())
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateRequest {
    pub objects: BTreeSet<ObjectId>,
    pub locations: BTreeSet<ActorId>,
    pub triggers: BTreeSet<TriggerId>,
    pub histories: BTreeSet<dialogue::HistoryKey>,
    pub claims: BTreeSet<dialogue::ClaimKey>,
    pub quests: BTreeSet<QuestId>,
    pub relationships: BTreeSet<actors::RelationshipKey>,
    pub interactions: BTreeSet<dialogue::InteractionKey>,
    pub actors: BTreeSet<ActorId>,
    pub inventories: BTreeSet<InventoryId>,
    pub wallets: BTreeSet<WalletId>,
    pub conversations: BTreeSet<ConversationKey>,
    pub facts: BTreeSet<Key>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContentRequest {
    pub objects: BTreeSet<ObjectId>,
    pub areas: BTreeSet<AreaId>,
    pub triggers: BTreeSet<TriggerId>,
    pub dialogue_contracts: BTreeSet<DialogueId>,
    pub claims: BTreeSet<ClaimId>,
    pub quests: BTreeSet<QuestId>,
    pub profiles: BTreeSet<InteractionProfileId>,
    pub predicates: BTreeSet<PredicateId>,
    pub items: BTreeSet<ItemDefinitionId>,
    pub actors: BTreeSet<ActorTemplateId>,
    pub dialogues: BTreeSet<DialogueId>,
    pub facts: BTreeSet<Key>,
}
impl ContentRequest {
    pub fn for_state(state: &SessionState, request: &StateRequest) -> Self {
        Self {
            objects: request.objects.clone(),
            areas: state
                .world
                .locations
                .iter()
                .flat_map(|l| l.areas.iter().copied())
                .collect(),
            triggers: request.triggers.clone(),
            dialogue_contracts: request.histories.iter().map(|h| h.dialogue).collect(),
            claims: request.claims.iter().map(|c| c.claim).collect(),
            quests: request.quests.clone(),
            profiles: state
                .interactions
                .iter()
                .filter_map(|i| i.current.as_ref().map(|c| c.profile))
                .collect(),
            predicates: BTreeSet::new(),
            items: state
                .inventories
                .iter()
                .flat_map(|i| i.entries.iter().map(|e| e.definition))
                .collect(),
            actors: state.actors.iter().map(|a| a.template).collect(),
            dialogues: request
                .conversations
                .iter()
                .map(|c| c.dialogue)
                .chain(state.conversations.iter().map(|c| c.dialogue))
                .collect(),
            facts: request.facts.clone(),
        }
    }
}
/// Return only the requested assets and their bounded dependency closure, including rules.
/// The returned content uses the publication's catalog ID/revision, never a subset fingerprint.
pub trait ContentSource {
    fn identity(&self) -> ContentIdentity;
    fn resolve(&mut self, request: &ContentRequest) -> Result<GameContent>;
    fn areas_at(&mut self, position: &actors::Position) -> Result<BTreeSet<AreaId>>;
    fn next_trigger(
        &mut self,
        signal: &WorldSignal,
        after: Option<TriggerId>,
    ) -> Result<Option<TriggerId>>;
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RecordKey {
    Object(ObjectId),
    Location(ActorId),
    Trigger(TriggerId),
    History(dialogue::HistoryKey),
    Claim(dialogue::ClaimKey),
    Quest(QuestId),
    Relationship(actors::RelationshipKey),
    Interaction(dialogue::InteractionKey),
    Actor(ActorId),
    Inventory(InventoryId),
    Wallet(WalletId),
    Conversation(ConversationKey),
}
/// Transaction-local records and optimistic revisions. No whole-playthrough resident cache.
pub struct WorkingSet {
    pub state: SessionState,
    pub versions: BTreeMap<RecordKey, u64>,
}
pub trait StateTransaction {
    fn header(&self) -> &SessionHeader;
    /// Also resolves inventory/wallet owners, actors' carried bags and conversation participants.
    fn load(&mut self, request: &StateRequest) -> Result<WorkingSet>;
    fn facts(&mut self, keys: &BTreeSet<Key>) -> Result<BTreeSet<Key>>;
    /// Indexed lookup. Staging an expiration removes it from subsequent results in this transaction.
    fn next_expiring_actor(&mut self, through: GameTime) -> Result<Option<ActorId>>;
    fn enqueue(&mut self, signals: &BTreeSet<WorldSignal>) -> Result<()>;
    fn next_event(&mut self) -> Result<Option<PendingEvent>>;
    fn advance_event(&mut self, event: &PendingEvent, after: Option<TriggerId>) -> Result<()>;
    fn movement_owner(&mut self, actor: ActorId) -> Result<Option<TriggerId>>;
    fn next_movement(
        &mut self,
        after: Option<TriggerId>,
        through: Option<GameTime>,
    ) -> Result<Option<TriggerId>>;
    fn stage(&mut self, before: &WorkingSet, after: &SessionState) -> Result<()>;
    fn commit(self, header: &SessionHeader) -> Result<()>;
}
pub trait StateStore {
    type Transaction<'a>: StateTransaction
    where
        Self: 'a;
    fn identity(&self) -> &ContentIdentity;
    fn header(&self) -> Result<SessionHeader>;
    fn begin(&mut self) -> Result<Self::Transaction<'_>>;
}
