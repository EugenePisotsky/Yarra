//! Authored conversations, active runs, scoped history and claims; no game services.
mod history;
use game_types::*;
pub use history::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgumentSource {
    ActorName(Key),
    Attribute { role: Key, attribute: Key },
    Text(TextRef),
    Number(i32),
    Select(String),
}
pub type ArgumentSources = BTreeMap<String, ArgumentSource>;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChoiceRepeat {
    Always,
    OncePerRun,
    OnceEver,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: Key,
    pub text: TextRef,
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub arguments: ArgumentSources,
    pub repeat: ChoiceRepeat,
    pub conditions: Vec<Key>,
    pub actions: Vec<Key>,
    /// None completes this run. Reopening is governed independently by the graph policy.
    pub next: Option<Key>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub id: Key,
    pub speaker: Key,
    pub text: TextRef,
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub arguments: ArgumentSources,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: Key,
    pub lines: Vec<Line>,
    pub choices: Vec<Choice>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dialogue {
    pub id: DialogueId,
    pub roles: BTreeSet<Key>,
    pub history_scope: ScopeSelector,
    pub repeat: RepeatPolicy,
    pub start: Key,
    pub nodes: Vec<Node>,
}
/// A derived, independently indexed contract. Selection/history queries need no graph payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueContract {
    pub id: DialogueId,
    pub roles: BTreeSet<Key>,
    pub history_scope: ScopeSelector,
    pub repeat: RepeatPolicy,
    pub lines: BTreeSet<Key>,
    pub choices: BTreeSet<Key>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    pub dialogue: DialogueId,
    pub participant: ActorId,
    pub speaker: ActorId,
    #[serde(deserialize_with = "game_types::deserialize_unique_map")]
    pub bindings: BTreeMap<Key, ActorId>,
    pub token: Token,
    pub node: Key,
    /// Next unacknowledged line. Choices become available after every line is acknowledged.
    pub line: usize,
    pub status: RunStatus,
    pub accepted: BTreeSet<Key>,
}
impl Dialogue {
    pub fn messages(&self) -> impl Iterator<Item = (&TextRef, &ArgumentSources)> {
        self.nodes.iter().flat_map(|n| {
            n.lines
                .iter()
                .map(|l| (&l.text, &l.arguments))
                .chain(n.choices.iter().map(|c| (&c.text, &c.arguments)))
        })
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
                                ArgumentSource::Attribute { .. } | ArgumentSource::Number(_),
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
    pub fn contract(&self) -> DialogueContract {
        DialogueContract {
            id: self.id,
            roles: self.roles.clone(),
            history_scope: self.history_scope,
            repeat: self.repeat,
            lines: self
                .nodes
                .iter()
                .flat_map(|n| n.lines.iter().map(|l| l.id.clone()))
                .collect(),
            choices: self
                .nodes
                .iter()
                .flat_map(|n| n.choices.iter().map(|c| c.id.clone()))
                .collect(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        require(
            !self.nodes.is_empty() && self.nodes.len() <= 4096,
            "dialogue node limit",
        )?;
        self.contract().validate()?;
        let mut nodes = BTreeSet::new();
        let mut choices = BTreeSet::new();
        let mut lines = BTreeSet::new();
        for node in &self.nodes {
            require(
                nodes.insert(&node.id)
                    && !node.lines.is_empty()
                    && node.lines.len() <= 64
                    && node.choices.len() <= 128,
                "duplicate/oversized node",
            )?;
            for line in &node.lines {
                require(
                    lines.insert(&line.id) && self.roles.contains(&line.speaker),
                    "duplicate line or undeclared speaker",
                )?;
                line.text.validate()?;
                self.validate_arguments(&line.arguments)?;
            }
            for choice in &node.choices {
                require(
                    choices.insert(&choice.id)
                        && choice.conditions.len() <= 64
                        && choice.actions.len() <= 64,
                    "duplicate/oversized choice",
                )?;
                choice.text.validate()?;
                self.validate_arguments(&choice.arguments)?;
                if let Some(next) = &choice.next {
                    self.node(next)?;
                }
            }
        }
        self.node(&self.start)?;
        Ok(())
    }
    fn validate_arguments(&self, args: &ArgumentSources) -> Result<()> {
        require(args.len() <= 32, "too many message arguments")?;
        for (name, value) in args {
            TextKey::new(name.clone())?;
            match value {
                ArgumentSource::ActorName(role) | ArgumentSource::Attribute { role, .. } => {
                    require(
                        self.roles.contains(role),
                        "argument references undeclared role",
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
    pub fn start(
        &self,
        participant: ActorId,
        speaker: ActorId,
        extra: &BTreeMap<Key, ActorId>,
        run: u64,
    ) -> Result<Conversation> {
        require(
            !extra.contains_key(&Key::new("player")?) && !extra.contains_key(&Key::new("speaker")?),
            "reserved role override",
        )?;
        let mut bindings = extra.clone();
        bindings.insert(Key::new("player")?, participant);
        bindings.insert(Key::new("speaker")?, speaker);
        let conversation = Conversation {
            dialogue: self.id,
            participant,
            speaker,
            bindings,
            token: Token { run, step: 0 },
            node: self.start.clone(),
            line: 0,
            status: RunStatus::Active,
            accepted: BTreeSet::new(),
        };
        conversation.validate(self)?;
        Ok(conversation)
    }
}
impl DialogueContract {
    pub fn validate(&self) -> Result<()> {
        require(
            self.roles.len() >= 2
                && self.roles.len() <= 16
                && self.roles.contains(&Key::new("player")?)
                && self.roles.contains(&Key::new("speaker")?),
            "dialogue requires player/speaker roles, at most 16 total",
        )?;
        require(
            !self.lines.is_empty() && self.lines.len() <= 4096 && self.choices.len() <= 4096,
            "dialogue history key budget exceeded",
        )?;
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
    pub fn validate(&self, graph: &Dialogue) -> Result<()> {
        require(
            self.dialogue == graph.id && self.participant != self.speaker,
            "invalid conversation identity",
        )?;
        let node = graph.node(&self.node)?;
        require(
            self.status != RunStatus::Completed || self.line == node.lines.len(),
            "completed conversation has unread lines",
        )?;
        require(
            self.token.run > 0 && self.line <= node.lines.len(),
            "invalid conversation cursor",
        )?;
        require(
            self.bindings.keys().cloned().collect::<BTreeSet<_>>() == graph.roles
                && self.bindings.get(&Key::new("player")?) == Some(&self.participant)
                && self.bindings.get(&Key::new("speaker")?) == Some(&self.speaker),
            "invalid conversation role bindings",
        )?;
        require(
            self.accepted.is_subset(&graph.contract().choices),
            "unknown accepted choice",
        )?;
        Ok(())
    }
    pub fn check(&self, token: Token) -> Result<()> {
        require(
            self.status == RunStatus::Active && self.token == token,
            "stale or inactive conversation cursor",
        )
    }
    pub fn choice<'a>(&self, graph: &'a Dialogue, id: &Key) -> Result<&'a Choice> {
        self.validate(graph)?;
        require(
            self.status == RunStatus::Active && self.line == graph.node(&self.node)?.lines.len(),
            "dialogue lines must be acknowledged before choosing",
        )?;
        let choice = graph
            .node(&self.node)?
            .choices
            .iter()
            .find(|c| &c.id == id)
            .ok_or_else(|| Invalid("choice not at current node".into()))?;
        require(
            choice.repeat != ChoiceRepeat::OncePerRun || !self.accepted.contains(id),
            "choice already accepted this run",
        )?;
        Ok(choice)
    }
    pub fn bump(&mut self) -> Result<()> {
        self.token.step = self
            .token
            .step
            .checked_add(1)
            .ok_or_else(|| Invalid("dialogue step overflow".into()))?;
        Ok(())
    }
    pub fn advance(&mut self, graph: &Dialogue, id: &Key) -> Result<()> {
        let next = self.choice(graph, id)?.next.clone();
        self.bump()?;
        self.accepted.insert(id.clone());
        match next {
            Some(node) => {
                self.node = node;
                self.line = 0;
            }
            None => self.status = RunStatus::Completed,
        }
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
