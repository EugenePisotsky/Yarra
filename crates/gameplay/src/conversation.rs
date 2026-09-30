//! Conversation execution and locale-independent read models.
use crate::session::{conditions_met, run_action};
use crate::{Result, *};
use dialogue::{ArgumentSource, ChoiceRepeat, HistoryEvent, RunStatus, Token};
use game_types::*;
use std::collections::BTreeMap;
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
    pub node: Key,
    pub line: Option<LineView>,
    pub choices: Vec<ChoiceView>,
}
impl GameContent {
    pub fn dialogue_contract(&self, id: DialogueId) -> Result<&dialogue::DialogueContract> {
        self.game
            .dialogue_contracts
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| Invalid("unknown dialogue contract".into()).into())
    }
    pub fn claim(&self, id: ClaimId) -> Result<&dialogue::ClaimDefinition> {
        self.game
            .claims
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| Invalid("unknown claim definition".into()).into())
    }
    pub(crate) fn validate_dialogue_text(&self, graph: &dialogue::Dialogue) -> Result<()> {
        graph.validate_messages(&|m| {
            self.text
                .iter()
                .find(|c| c.id == m.resource)
                .and_then(|c| c.messages.get(&m.key))
                .ok_or_else(|| Invalid("unknown dialogue message contract".into()))
        })?;
        for (_, args) in graph.messages() {
            for source in args.values() {
                if let ArgumentSource::Attribute { attribute, .. } = source {
                    self.game.rules.attribute(attribute)?;
                }
            }
        }
        Ok(())
    }
    fn bind_text(
        &self,
        state: &SessionState,
        c: &dialogue::Conversation,
        text: &TextRef,
        args: &dialogue::ArgumentSources,
    ) -> Result<BoundText> {
        let mut arguments = BTreeMap::new();
        for (name, source) in args {
            let value = match source {
                ArgumentSource::ActorName(role) => {
                    let actor = state.actor(c.bindings[role])?;
                    let template = self
                        .game
                        .actors
                        .iter()
                        .find(|t| t.id == actor.template)
                        .ok_or_else(|| Invalid("missing name template".into()))?;
                    BoundArgument::Text(
                        actor
                            .name_override
                            .clone()
                            .unwrap_or_else(|| template.name.clone()),
                    )
                }
                ArgumentSource::Attribute { role, attribute } => {
                    BoundArgument::Number(state.derived(self, c.bindings[role])?[attribute])
                }
                ArgumentSource::Text(text) => BoundArgument::Text(text.clone()),
                ArgumentSource::Number(value) => BoundArgument::Number(*value),
                ArgumentSource::Select(value) => BoundArgument::Select(value.clone()),
            };
            arguments.insert(name.clone(), value);
        }
        Ok(BoundText {
            text: text.clone(),
            arguments,
        })
    }
    pub fn conversation_view(
        &self,
        state: &SessionState,
        key: ConversationKey,
    ) -> Result<ConversationView> {
        let c = conversation(state, key)?;
        let graph = self.dialogue(key.dialogue)?;
        let node = graph.node(&c.node)?;
        let line = if c.status == RunStatus::Active {
            node.lines
                .get(c.line)
                .map(|line| {
                    Ok::<_, GameplayError>(LineView {
                        id: line.id.clone(),
                        speaker: c.bindings[&line.speaker],
                        text: self.bind_text(state, c, &line.text, &line.arguments)?,
                    })
                })
                .transpose()?
        } else {
            None
        };
        let mut choices = Vec::new();
        if c.status == RunStatus::Active && c.line == node.lines.len() {
            for choice in &node.choices {
                if choice_available(self, state, c, choice)? {
                    choices.push(ChoiceView {
                        id: choice.id.clone(),
                        text: self.bind_text(state, c, &choice.text, &choice.arguments)?,
                    });
                }
            }
        }
        Ok(ConversationView {
            key,
            token: c.token,
            status: c.status,
            node: c.node.clone(),
            line,
            choices,
        })
    }
}
pub(crate) fn conversation(
    state: &SessionState,
    key: ConversationKey,
) -> Result<&dialogue::Conversation> {
    state
        .conversations
        .iter()
        .find(|c| ConversationKey::of(c) == key)
        .ok_or_else(|| Invalid("conversation has not started".into()).into())
}
fn index(state: &SessionState, key: ConversationKey) -> Result<usize> {
    state
        .conversations
        .iter()
        .position(|c| ConversationKey::of(c) == key)
        .ok_or_else(|| Invalid("conversation has not started".into()).into())
}
fn record(
    content: &GameContent,
    state: &mut SessionState,
    key: ConversationKey,
    event: HistoryEvent,
) -> Result<()> {
    let h = content
        .dialogue_contract(key.dialogue)?
        .history_key(key.participant, key.speaker);
    let time = state.time;
    state
        .histories
        .iter_mut()
        .find(|v| v.key == h)
        .ok_or_else(|| Invalid("history not resolved".into()))?
        .record(&event, time)?;
    Ok(())
}
fn choice_available(
    content: &GameContent,
    state: &SessionState,
    c: &dialogue::Conversation,
    choice: &dialogue::Choice,
) -> Result<bool> {
    let history = state.history(
        content
            .dialogue_contract(c.dialogue)?
            .history_key(c.participant, c.speaker),
    )?;
    let repeat = match choice.repeat {
        ChoiceRepeat::Always => true,
        ChoiceRepeat::OncePerRun => !c.accepted.contains(&choice.id),
        ChoiceRepeat::OnceEver => history.count(&HistoryEvent::Choice(choice.id.clone())) == 0,
    };
    Ok(repeat
        && conditions_met(
            content,
            state,
            c.participant,
            c.speaker,
            c.dialogue,
            &choice.conditions,
        )?)
}
pub(crate) fn start(
    content: &GameContent,
    state: &mut SessionState,
    key: ConversationKey,
    bindings: &BTreeMap<Key, ActorId>,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let previous = state
        .conversations
        .iter()
        .position(|c| ConversationKey::of(c) == key);
    let run = if let Some(i) = previous {
        require(
            state.conversations[i].status != RunStatus::Active,
            "conversation already active",
        )?;
        state.conversations[i]
            .token
            .run
            .checked_add(1)
            .ok_or_else(|| Invalid("dialogue run overflow".into()))?
    } else {
        1
    };
    let contract = content.dialogue_contract(key.dialogue)?;
    require(
        contract.repeat.eligible(
            state.history(contract.history_key(key.participant, key.speaker))?,
            state.time,
        ),
        "dialogue repeat policy blocks start",
    )?;
    let next =
        content
            .dialogue(key.dialogue)?
            .start(key.participant, key.speaker, bindings, run)?;
    for id in next.bindings.values() {
        state.actor(*id)?;
    }
    record(content, state, key, HistoryEvent::Started)?;
    match previous {
        Some(i) => state.conversations[i] = next,
        None => state.conversations.push(next),
    };
    events.push(GameEvent::DialogueStarted);
    Ok(())
}
pub(crate) fn present(
    content: &GameContent,
    state: &mut SessionState,
    key: ConversationKey,
    expected: Token,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let i = index(state, key)?;
    let c = &state.conversations[i];
    c.check(expected)?;
    let node = content.dialogue(key.dialogue)?.node(&c.node)?;
    let line = node
        .lines
        .get(c.line)
        .ok_or_else(|| Invalid("no pending dialogue line".into()))?;
    let id = line.id.clone();
    state.conversations[i].bump()?;
    state.conversations[i].line += 1;
    record(content, state, key, HistoryEvent::Line(id.clone()))?;
    if state.conversations[i].line == node.lines.len() && node.choices.is_empty() {
        state.conversations[i].status = RunStatus::Completed;
        record(content, state, key, HistoryEvent::Completed)?;
    }
    events.push(GameEvent::LinePresented { key, line: id });
    Ok(())
}
pub(crate) fn choose(
    content: &GameContent,
    state: &mut SessionState,
    key: ConversationKey,
    choice: &Key,
    expected: Token,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let i = index(state, key)?;
    let c = &state.conversations[i];
    c.check(expected)?;
    let graph = content.dialogue(key.dialogue)?;
    let selected = c.choice(graph, choice)?;
    require(
        choice_available(content, state, c, selected)?,
        "dialogue conditions or repeat policy are not met",
    )?;
    for action in &selected.actions {
        run_action(
            content,
            state,
            key.participant,
            key.speaker,
            &content.game.actions[&BindingId::new(key.dialogue, action.clone())],
            events,
        )?;
    }
    state.conversations[i].advance(graph, choice)?;
    record(content, state, key, HistoryEvent::Choice(choice.clone()))?;
    if state.conversations[i].status == RunStatus::Completed {
        record(content, state, key, HistoryEvent::Completed)?;
    }
    events.push(GameEvent::ChoiceAccepted {
        choice: choice.clone(),
    });
    Ok(())
}
pub(crate) fn interrupt(
    content: &GameContent,
    state: &mut SessionState,
    key: ConversationKey,
    expected: Token,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let i = index(state, key)?;
    state.conversations[i].check(expected)?;
    state.conversations[i].bump()?;
    state.conversations[i].status = RunStatus::Interrupted;
    record(content, state, key, HistoryEvent::Interrupted)?;
    events.push(GameEvent::DialogueInterrupted { key });
    Ok(())
}
