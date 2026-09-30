//! Conversation execution and locale-independent read models.
use crate::dialogue::{
    ArgumentSource, Conversation, Dialogue, HistoryEvent, Node, NodeKind, Position, Repeat,
    RunStatus, Token,
};
use crate::session::run_action;
use crate::tx::Tx;
use crate::{Result, *};
use game_types::*;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineView {
    pub id: Key,
    pub speaker: ActorId,
    pub text: BoundText,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceView {
    pub id: Key,
    pub text: BoundText,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationView {
    pub key: ConversationKey,
    pub token: Token,
    pub status: RunStatus,
    /// The line being shown, or `None` while the player is choosing or the run has ended.
    pub line: Option<LineView>,
    pub choices: Vec<ChoiceView>,
}
impl GameContent {
    pub fn dialogue_contract(&self, id: DialogueId) -> Result<&dialogue::DialogueContract> {
        self.game
            .dialogue_contracts
            .get(&id)
            .ok_or_else(|| Invalid("unknown dialogue contract".into()).into())
    }
    pub(crate) fn validate_dialogue_text(&self, graph: &Dialogue) -> Result<()> {
        graph.validate_messages(&|m| {
            self.text
                .get(&m.resource)
                .and_then(|c| c.messages.get(&m.key))
                .ok_or_else(|| Invalid("unknown dialogue message contract".into()))
        })?;
        for (_, args) in graph.messages() {
            for source in args.values() {
                if let ArgumentSource::Stat { stat, .. } = source {
                    self.game.rules.stat(stat)?;
                }
            }
        }
        Ok(())
    }
    fn bind_text(&self, state: &SessionState, c: &Conversation, node: &Node) -> Result<BoundText> {
        let mut arguments = BTreeMap::new();
        for (name, source) in &node.arguments {
            let value = match source {
                ArgumentSource::ActorName(role) => {
                    let actor = state.actor(c.bindings[role])?;
                    BoundArgument::Text(match &actor.name_override {
                        Some(name) => name.clone(),
                        None => self.template(actor.template)?.name.clone(),
                    })
                }
                ArgumentSource::Stat { role, stat } => {
                    BoundArgument::Number(state.stat(self, c.bindings[role], stat)?)
                }
                ArgumentSource::Text(text) => BoundArgument::Text(text.clone()),
                ArgumentSource::Number(value) => BoundArgument::Number(*value),
                ArgumentSource::Select(value) => BoundArgument::Select(value.clone()),
            };
            arguments.insert(name.clone(), value);
        }
        Ok(BoundText {
            text: node.text.clone(),
            arguments,
        })
    }
    pub fn conversation_view(
        &self,
        state: &SessionState,
        key: ConversationKey,
    ) -> Result<ConversationView> {
        let c = state.conversation(key)?;
        let graph = self.dialogue(key.dialogue)?;
        let mut view = ConversationView {
            key,
            token: c.token,
            status: c.status,
            line: None,
            choices: Vec::new(),
        };
        if c.status != RunStatus::Active {
            return Ok(view);
        }
        match &c.position {
            Position::Line(id) => {
                let node = graph.node(id)?;
                view.line = Some(LineView {
                    id: node.id.clone(),
                    speaker: c.bindings[&node.speaker],
                    text: self.bind_text(state, c, node)?,
                });
            }
            Position::Choices(after) => {
                for node in choices(self, state, c, &graph, after.as_ref())? {
                    view.choices.push(ChoiceView {
                        id: node.id.clone(),
                        text: self.bind_text(state, c, node)?,
                    });
                }
            }
        }
        Ok(view)
    }
}
fn history_key(content: &GameContent, key: ConversationKey) -> Result<dialogue::HistoryKey> {
    Ok(content
        .dialogue_contract(key.dialogue)?
        .history_key(key.participant, key.speaker))
}
fn record(
    content: &GameContent,
    state: &mut Tx,
    key: ConversationKey,
    event: HistoryEvent,
) -> Result<()> {
    let time = state.time;
    state
        .history_mut(history_key(content, key)?)
        .record(&event, time)?;
    Ok(())
}
/// A node can be reached when its speaker is taking part, its repeat policy allows it and its
/// condition holds.
fn eligible(
    content: &GameContent,
    state: &SessionState,
    c: &Conversation,
    node: &Node,
) -> Result<bool> {
    if !c.bindings.contains_key(&node.speaker) {
        return Ok(false);
    }
    let repeats = match node.repeat {
        Repeat::Always => true,
        Repeat::OncePerRun => !c.visited.contains(&node.id),
        Repeat::OnceEver => {
            state
                .history(history_key(content, ConversationKey::of(c))?)
                .count(&HistoryEvent::Node(node.id.clone()))
                == 0
        }
    };
    if !repeats {
        return Ok(false);
    }
    let Some(condition) = &node.condition else {
        return Ok(true);
    };
    let others: BTreeSet<ActorId> = c.bindings.values().copied().collect();
    Ok(content
        .evaluate_among(condition, state, c.participant, c.speaker, &others)?
        .matched)
}
/// The eligible choices under `after`, in authored order.
fn choices<'a>(
    content: &GameContent,
    state: &SessionState,
    c: &Conversation,
    graph: &'a Dialogue,
    after: Option<&Key>,
) -> Result<Vec<&'a Node>> {
    let mut result = Vec::new();
    for id in graph.children(after)? {
        let node = graph.node(id)?;
        if node.kind == NodeKind::Choice && eligible(content, state, c, node)? {
            result.push(node);
        }
    }
    Ok(result)
}
/// Moves the run past `after`: the first eligible child decides whether a line plays, the
/// player chooses, or the conversation is complete.
fn proceed(
    content: &GameContent,
    state: &mut Tx,
    key: ConversationKey,
    after: Option<Key>,
) -> Result<()> {
    let graph = content.dialogue(key.dialogue)?;
    let c = state.conversation(key)?;
    let mut next = None;
    for id in graph.children(after.as_ref())? {
        let node = graph.node(id)?;
        if eligible(content, state, c, node)? {
            next = Some(match node.kind {
                NodeKind::Line => Position::Line(node.id.clone()),
                NodeKind::Choice => Position::Choices(after.clone()),
            });
            break;
        }
    }
    let c = state.conversation_mut(key)?;
    match next {
        Some(position) => c.position = position,
        None => {
            c.status = RunStatus::Completed;
            record(content, state, key, HistoryEvent::Completed)?;
        }
    }
    Ok(())
}
/// Runs a node the player acknowledged or picked, then moves on from it.
fn take(
    content: &GameContent,
    state: &mut Tx,
    key: ConversationKey,
    node: &Node,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let others: BTreeSet<ActorId> = state
        .conversation(key)?
        .bindings
        .values()
        .copied()
        .collect();
    for action in &node.actions {
        let (participant, speaker) = (key.participant, key.speaker);
        run_action(
            content,
            state,
            participant,
            speaker,
            &others,
            action,
            events,
        )?;
    }
    let c = state.conversation_mut(key)?;
    c.bump()?;
    c.visited.insert(node.id.clone());
    record(content, state, key, HistoryEvent::Node(node.id.clone()))?;
    proceed(content, state, key, Some(node.id.clone()))
}
pub(crate) fn start(
    content: &GameContent,
    state: &mut Tx,
    key: ConversationKey,
    bindings: &BTreeMap<Key, ActorId>,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let run = match state.conversations.get(&key) {
        Some(previous) => {
            Rejection::AlreadyTalking.unless(previous.status != RunStatus::Active)?;
            previous
                .token
                .run
                .checked_add(1)
                .ok_or_else(|| Invalid("dialogue run overflow".into()))?
        }
        None => 1,
    };
    let contract = content.dialogue_contract(key.dialogue)?;
    require(
        contract
            .repeat
            .eligible(&state.history(history_key(content, key)?), state.time),
        "dialogue repeat policy blocks start",
    )?;
    let next = content.dialogue(key.dialogue)?.start(
        key.participant,
        key.speaker,
        bindings,
        &state.party.members,
        run,
    )?;
    for id in next.bindings.values() {
        state.actor(*id)?;
    }
    record(content, state, key, HistoryEvent::Started)?;
    state.put_conversation(next);
    proceed(content, state, key, None)?;
    events.push(GameEvent::DialogueStarted {
        key,
        mode: contract.mode,
    });
    Ok(())
}
pub(crate) fn present(
    content: &GameContent,
    state: &mut Tx,
    key: ConversationKey,
    expected: Token,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let c = state.conversation(key)?;
    c.check(expected)
        .map_err(|_| Rejection::ConversationMoved)?;
    let Position::Line(id) = &c.position else {
        return Err(Invalid("no pending dialogue line".into()).into());
    };
    let graph = content.dialogue(key.dialogue)?;
    let node = graph.node(id)?;
    take(content, state, key, node, events)?;
    events.push(GameEvent::LinePresented {
        key,
        line: node.id.clone(),
    });
    Ok(())
}
pub(crate) fn choose(
    content: &GameContent,
    state: &mut Tx,
    key: ConversationKey,
    choice: &Key,
    expected: Token,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let c = state.conversation(key)?;
    c.check(expected)
        .map_err(|_| Rejection::ConversationMoved)?;
    let Position::Choices(after) = &c.position else {
        return Err(Invalid("dialogue lines must be acknowledged before choosing".into()).into());
    };
    let graph = content.dialogue(key.dialogue)?;
    let node = choices(content, state, c, &graph, after.as_ref())?
        .into_iter()
        .find(|n| &n.id == choice)
        .ok_or_else(|| Rejection::ChoiceUnavailable(choice.clone()))?;
    take(content, state, key, node, events)?;
    events.push(GameEvent::ChoiceAccepted {
        choice: choice.clone(),
    });
    Ok(())
}
pub(crate) fn interrupt(
    content: &GameContent,
    state: &mut Tx,
    key: ConversationKey,
    expected: Token,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let c = state.conversation_mut(key)?;
    c.check(expected)
        .map_err(|_| Rejection::ConversationMoved)?;
    c.bump()?;
    c.status = RunStatus::Interrupted;
    record(content, state, key, HistoryEvent::Interrupted)?;
    events.push(GameEvent::DialogueInterrupted { key });
    Ok(())
}
