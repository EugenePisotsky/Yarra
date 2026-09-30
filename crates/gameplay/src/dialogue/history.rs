use super::DialogueContract;
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeSelector {
    Playthrough,
    Player,
    Speaker,
    Interaction,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Scope {
    Playthrough,
    Actor(ActorId),
    Interaction { player: ActorId, speaker: ActorId },
}
impl ScopeSelector {
    pub fn resolve(self, player: ActorId, speaker: ActorId) -> Scope {
        match self {
            Self::Playthrough => Scope::Playthrough,
            Self::Player => Scope::Actor(player),
            Self::Speaker => Scope::Actor(speaker),
            Self::Interaction => Scope::Interaction { player, speaker },
        }
    }
    pub fn accepts(self, scope: Scope) -> bool {
        matches!(
            (self, scope),
            (Self::Playthrough, Scope::Playthrough)
                | (Self::Player | Self::Speaker, Scope::Actor(_))
                | (Self::Interaction, Scope::Interaction { .. })
        )
    }
}
impl Scope {
    pub fn validate(self) -> Result<()> {
        if let Self::Interaction { player, speaker } = self {
            require(
                player != speaker,
                "interaction scope requires distinct actors",
            )?;
        }
        Ok(())
    }

    pub fn actors(self) -> Vec<ActorId> {
        match self {
            Self::Playthrough => vec![],
            Self::Actor(id) => vec![id],
            Self::Interaction { player, speaker } => vec![player, speaker],
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatPolicy {
    Always,
    OnceCompleted,
    Cooldown { millis: u64 },
}
impl RepeatPolicy {
    pub fn validate(self) -> Result<()> {
        if let Self::Cooldown { millis } = self {
            require(
                millis > 0 && millis <= i64::MAX as u64,
                "invalid dialogue cooldown",
            )?;
        }
        Ok(())
    }
    pub fn eligible(self, history: &History, now: GameTime) -> bool {
        match self {
            Self::Always => true,
            Self::OnceCompleted => history.completed == 0,
            Self::Cooldown { millis } => history
                .last_started
                .is_none_or(|t| now.0.saturating_sub(t.0) >= millis),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HistoryKey {
    pub dialogue: DialogueId,
    pub scope: Scope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryEvent {
    Started,
    Completed,
    Interrupted,
    /// A line acknowledged or a choice picked.
    Node(Key),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    pub key: HistoryKey,
    pub started: u64,
    pub completed: u64,
    pub interrupted: u64,
    pub last_started: Option<GameTime>,
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub nodes: BTreeMap<Key, u64>,
}
impl History {
    pub fn empty(key: HistoryKey) -> Self {
        Self {
            key,
            started: 0,
            completed: 0,
            interrupted: 0,
            last_started: None,
            nodes: BTreeMap::new(),
        }
    }
    pub fn count(&self, event: &HistoryEvent) -> u64 {
        match event {
            HistoryEvent::Started => self.started,
            HistoryEvent::Completed => self.completed,
            HistoryEvent::Interrupted => self.interrupted,
            HistoryEvent::Node(id) => self.nodes.get(id).copied().unwrap_or(0),
        }
    }
    pub fn record(&mut self, event: &HistoryEvent, time: GameTime) -> Result<()> {
        let count = self
            .count(event)
            .checked_add(1)
            .ok_or_else(|| Invalid("dialogue history overflow".into()))?;
        match event {
            HistoryEvent::Started => self.started = count,
            HistoryEvent::Completed => self.completed = count,
            HistoryEvent::Interrupted => self.interrupted = count,
            HistoryEvent::Node(id) => {
                require(
                    self.nodes.contains_key(id) || self.nodes.len() < 4096,
                    "dialogue history budget exceeded",
                )?;
                self.nodes.insert(id.clone(), count);
            }
        }
        if matches!(event, HistoryEvent::Started) {
            self.last_started = Some(time);
        }
        Ok(())
    }
    pub fn validate(&self, contract: &DialogueContract, now: GameTime) -> Result<()> {
        self.key.scope.validate()?;
        require(
            self.key.dialogue == contract.id && contract.history_scope.accepts(self.key.scope),
            "invalid history scope",
        )?;
        require(
            self.completed <= self.started && self.interrupted <= self.started - self.completed,
            "invalid history counts",
        )?;
        require(
            (self.started == 0) == self.last_started.is_none()
                && self.last_started.is_none_or(|t| t <= now),
            "invalid history time",
        )?;
        for (key, count) in &self.nodes {
            require(
                *count > 0 && contract.nodes.contains(key),
                "invalid node history",
            )?;
        }
        require(
            self.started > 0 || self.nodes.is_empty(),
            "unstarted history has events",
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimDefinition {
    pub id: ClaimId,
    pub scope: ScopeSelector,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClaimKey {
    pub claim: ClaimId,
    pub scope: Scope,
}
impl ClaimDefinition {
    pub fn key(&self, player: ActorId, speaker: ActorId) -> ClaimKey {
        ClaimKey {
            claim: self.id,
            scope: self.scope.resolve(player, speaker),
        }
    }
}
