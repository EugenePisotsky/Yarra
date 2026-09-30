//! Authored conversations, active runs and scoped history; no game services.
//!
//! A conversation is a flat set of nodes. Each node is spoken by a role and lists its
//! children in order. After a node, the first eligible child decides what happens: a line
//! plays; a choice means every eligible choice among the children is offered to the player.
//! A reaction from whoever happens to be present is therefore just an earlier child that is
//! only eligible when its speaker is there.
mod history;
use crate::{Action, Condition};
use game_types::*;
pub use history::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PLAYER: &str = "player";
pub const SPEAKER: &str = "speaker";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgumentSource {
    ActorName(Key),
    /// A stat of whoever fills the role, or how much of a resource they have.
    Stat {
        role: Key,
        stat: Key,
    },
    Text(TextRef),
    Number(i32),
    Select(String),
}
pub type ArgumentSources = BTreeMap<String, ArgumentSource>;
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Always,
    OncePerRun,
    OnceEver,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// Spoken by its role and acknowledged by the player.
    Line,
    /// Offered to the player together with its eligible sibling choices.
    Choice,
}
/// How a conversation is presented. The run itself works the same either way.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Takes the player's attention: lines wait to be acknowledged and choices are offered.
    #[default]
    Blocking,
    /// Plays alongside the game, e.g. companions talking while walking. Lines only.
    Ambient,
}
/// How a role gets its actor when a conversation starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// Must be supplied. `player` and `speaker` are always required.
    Required,
    /// May be supplied; its nodes are skipped otherwise.
    Optional,
    /// A named character, taking part only when present.
    Actor(ActorId),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: Key,
    pub kind: NodeKind,
    pub speaker: Key,
    pub text: TextRef,
    #[serde(default, deserialize_with = "game_types::deserialize_unique_map")]
    pub arguments: ArgumentSources,
    #[serde(default)]
    pub repeat: Repeat,
    #[serde(default)]
    pub condition: Option<Condition>,
    /// Run when the line is acknowledged or the choice is picked.
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Tried in order. With none eligible the conversation is complete.
    #[serde(default)]
    pub children: Vec<Key>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dialogue {
    pub id: DialogueId,
    #[serde(deserialize_with = "game_types::deserialize_key_map")]
    pub roles: BTreeMap<Key, Role>,
    pub history_scope: ScopeSelector,
    pub repeat: RepeatPolicy,
    #[serde(default)]
    pub mode: Mode,
    /// Entry nodes, tried in order like any node's children.
    pub start: Vec<Key>,
    pub nodes: Vec<Node>,
}
/// The part of a conversation that is always loaded: enough to select it, to check saved
/// history and to validate references, without the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueContract {
    pub id: DialogueId,
    #[serde(deserialize_with = "game_types::deserialize_key_map")]
    pub roles: BTreeMap<Key, Role>,
    pub history_scope: ScopeSelector,
    pub repeat: RepeatPolicy,
    pub mode: Mode,
    pub nodes: BTreeSet<Key>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Active,
    Completed,
    Interrupted,
}
/// Optimistic cursor carried by UI commands: retries cannot consume another line/choice/run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub run: u64,
    pub step: u64,
}
/// Where a run is waiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Position {
    /// This line is being shown and waits to be acknowledged.
    Line(Key),
    /// The player picks among the choices under this node (`None`: the entry nodes).
    Choices(Option<Key>),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    pub dialogue: DialogueId,
    pub participant: ActorId,
    pub speaker: ActorId,
    /// Roles that have an actor in this run.
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub bindings: BTreeMap<Key, ActorId>,
    pub token: Token,
    pub position: Position,
    pub status: RunStatus,
    /// Nodes acknowledged or picked during this run.
    pub visited: BTreeSet<Key>,
}
fn key(name: &str) -> Key {
    Key::new(name).expect("static role name")
}
impl Dialogue {
    pub fn messages(&self) -> impl Iterator<Item = (&TextRef, &ArgumentSources)> {
        self.nodes.iter().map(|n| (&n.text, &n.arguments))
    }
    pub fn validate_messages<'a>(
        &self,
        lookup: &impl Fn(&MessageRef) -> Result<&'a MessageContract>,
    ) -> Result<()> {
        for (text, args) in self.messages() {
            match text {
                TextRef::Literal(_) => require(
                    args.is_empty(),
                    "literal dialogue text cannot have arguments",
                )?,
                TextRef::Message(m) => {
                    let contract = lookup(m)?;
                    require(
                        args.len() == contract.arguments.len(),
                        "dialogue argument contract mismatch",
                    )?;
                    for (name, source) in args {
                        let valid = match (source, contract.arguments.get(name)) {
                            (
                                ArgumentSource::ActorName(_) | ArgumentSource::Text(_),
                                Some(ArgumentType::Text),
                            ) => true,
                            (
                                ArgumentSource::Stat { .. } | ArgumentSource::Number(_),
                                Some(ArgumentType::Number),
                            ) => true,
                            (ArgumentSource::Select(value), Some(ArgumentType::Select(values))) => {
                                values.contains(value)
                            }
                            _ => false,
                        };
                        require(valid, "dialogue argument type mismatch")?;
                    }
                }
            }
            for source in args.values() {
                if let ArgumentSource::Text(TextRef::Message(m)) = source {
                    require(
                        lookup(m)?.arguments.is_empty(),
                        "nested argument text must be static",
                    )?;
                }
            }
        }
        Ok(())
    }
    pub fn node(&self, id: &Key) -> Result<&Node> {
        self.nodes
            .iter()
            .find(|n| &n.id == id)
            .ok_or_else(|| Invalid("unknown dialogue node".into()))
    }
    /// The nodes that can follow `after`, or the entry nodes.
    pub fn children(&self, after: Option<&Key>) -> Result<&[Key]> {
        Ok(match after {
            Some(id) => &self.node(id)?.children,
            None => &self.start,
        })
    }
    pub fn contract(&self) -> DialogueContract {
        DialogueContract {
            id: self.id,
            roles: self.roles.clone(),
            history_scope: self.history_scope,
            repeat: self.repeat,
            mode: self.mode,
            nodes: self.nodes.iter().map(|n| n.id.clone()).collect(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.contract().validate()?;
        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            require(ids.insert(&node.id), "duplicate dialogue node")?;
            require(
                self.mode == Mode::Blocking || node.kind == NodeKind::Line,
                "an ambient conversation cannot offer choices",
            )?;
            require(
                self.roles.contains_key(&node.speaker),
                "node speaker is not a declared role",
            )?;
            node.text.validate()?;
            self.validate_arguments(&node.arguments)?;
        }
        require(!self.start.is_empty(), "a dialogue has entry nodes")?;
        for id in self
            .start
            .iter()
            .chain(self.nodes.iter().flat_map(|n| &n.children))
        {
            require(ids.contains(id), "unknown dialogue node")?;
        }
        Ok(())
    }
    fn validate_arguments(&self, args: &ArgumentSources) -> Result<()> {
        for (name, value) in args {
            TextKey::new(name.clone())?;
            match value {
                ArgumentSource::ActorName(role) | ArgumentSource::Stat { role, .. } => {
                    // Text can only name roles that are certain to have an actor.
                    require(
                        self.roles.get(role) == Some(&Role::Required),
                        "argument needs a required role",
                    )?
                }
                ArgumentSource::Text(text) => text.validate()?,
                ArgumentSource::Select(value) => require(
                    !value.is_empty() && value.len() <= 96,
                    "invalid selector value",
                )?,
                _ => {}
            }
        }
        Ok(())
    }
    /// Binds roles for a new run. `supplied` fills required and optional roles; named
    /// characters join when they are among `present`.
    pub fn start(
        &self,
        participant: ActorId,
        speaker: ActorId,
        supplied: &BTreeMap<Key, ActorId>,
        present: &BTreeSet<ActorId>,
        run: u64,
    ) -> Result<Conversation> {
        let mut bindings = BTreeMap::from([(key(PLAYER), participant), (key(SPEAKER), speaker)]);
        for (role, actor) in supplied {
            require(
                !bindings.contains_key(role)
                    && matches!(self.roles.get(role), Some(Role::Required | Role::Optional)),
                "role cannot be supplied",
            )?;
            bindings.insert(role.clone(), *actor);
        }
        for (role, binding) in &self.roles {
            match binding {
                Role::Required => require(bindings.contains_key(role), "required role is unbound")?,
                Role::Actor(actor) if present.contains(actor) => {
                    bindings.insert(role.clone(), *actor);
                }
                _ => {}
            }
        }
        let conversation = Conversation {
            dialogue: self.id,
            participant,
            speaker,
            bindings,
            token: Token { run, step: 0 },
            // Replaced by the first resolved position before the run is stored.
            position: Position::Choices(None),
            status: RunStatus::Active,
            visited: BTreeSet::new(),
        };
        conversation.validate(self)?;
        Ok(conversation)
    }
}
impl DialogueContract {
    pub fn validate(&self) -> Result<()> {
        require(
            self.roles.get(&key(PLAYER)) == Some(&Role::Required)
                && self.roles.get(&key(SPEAKER)) == Some(&Role::Required),
            "dialogue requires player/speaker roles",
        )?;
        require(!self.nodes.is_empty(), "a dialogue has nodes")?;
        self.repeat.validate()
    }
    pub fn history_key(&self, player: ActorId, speaker: ActorId) -> HistoryKey {
        HistoryKey {
            dialogue: self.id,
            scope: self.history_scope.resolve(player, speaker),
        }
    }
}
impl Conversation {
    /// Checks that need only the always-loaded contract, not the graph.
    pub fn validate_contract(&self, contract: &DialogueContract) -> Result<()> {
        require(
            self.dialogue == contract.id && self.participant != self.speaker,
            "invalid conversation identity",
        )?;
        require(self.token.run > 0, "invalid conversation cursor")?;
        require(
            self.bindings.get(&key(PLAYER)) == Some(&self.participant)
                && self.bindings.get(&key(SPEAKER)) == Some(&self.speaker),
            "invalid conversation role bindings",
        )?;
        for (role, binding) in &contract.roles {
            let bound = self.bindings.get(role);
            let valid = match binding {
                Role::Required => bound.is_some(),
                Role::Optional => true,
                Role::Actor(actor) => bound.is_none_or(|b| b == actor),
            };
            require(valid, "invalid conversation role bindings")?;
        }
        require(
            self.bindings.keys().all(|r| contract.roles.contains_key(r)),
            "invalid conversation role bindings",
        )?;
        let known = |id: &Key| contract.nodes.contains(id);
        require(
            self.visited.iter().all(known)
                && match &self.position {
                    Position::Line(id) | Position::Choices(Some(id)) => known(id),
                    Position::Choices(None) => true,
                },
            "unknown dialogue node in saved run",
        )
    }
    pub fn validate(&self, graph: &Dialogue) -> Result<()> {
        self.validate_contract(&graph.contract())?;
        if let Position::Line(id) = &self.position {
            require(
                graph.node(id)?.kind == NodeKind::Line,
                "saved line position is not a line",
            )?;
        }
        Ok(())
    }
    pub fn check(&self, token: Token) -> Result<()> {
        require(
            self.status == RunStatus::Active && self.token == token,
            "stale or inactive conversation cursor",
        )
    }
    pub fn bump(&mut self) -> Result<()> {
        self.token.step = self
            .token
            .step
            .checked_add(1)
            .ok_or_else(|| Invalid("dialogue step overflow".into()))?;
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InteractionKey {
    pub participant: ActorId,
    pub speaker: ActorId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub profile: InteractionProfileId,
    pub rule: Key,
    pub dialogue: DialogueId,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interaction {
    pub key: InteractionKey,
    pub current: Option<Selection>,
}
