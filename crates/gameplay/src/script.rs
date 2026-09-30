//! The boundary scripts work through. `gameplay` defines what a script may read and do;
//! an engine (Luau, in the `scripting` crate) only translates calls into these methods, so
//! scripted and built-in rules go through the same checks and the same command journal.
use crate::rules::Sheet;
use crate::tx::Tx;
use crate::{Action, GameContent, GameEvent, Result, SessionState, Value, quests};
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::rc::Rc;

/// `module.function`, e.g. `guard.ready`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ScriptName {
    pub module: Key,
    pub function: Key,
}
impl TryFrom<String> for ScriptName {
    type Error = Invalid;
    fn try_from(value: String) -> game_types::Result<Self> {
        let (module, function) = value
            .split_once('.')
            .ok_or_else(|| Invalid(format!("script reference {value} is not module.function")))?;
        Ok(Self {
            module: Key::new(module)?,
            function: Key::new(function)?,
        })
    }
}
impl From<ScriptName> for String {
    fn from(name: ScriptName) -> Self {
        name.to_string()
    }
}
impl std::fmt::Display for ScriptName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.module.as_str(), self.function.as_str())
    }
}
/// One authored script file. Its name is the module part of a `ScriptName`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptModule {
    pub name: Key,
    pub source: String,
}
pub trait ScriptEngine {
    fn exports(&self, name: &ScriptName) -> bool;
    /// An error is an error, never a quiet `false`.
    fn condition(&self, name: &ScriptName, scope: &ReadScope) -> Result<bool>;
    fn action(&self, name: &ScriptName, scope: &mut ActScope) -> Result<()>;
    /// The rules' formula for derived stats: a pure function of the character, returning
    /// a value for every derived stat by name.
    fn derive(
        &self,
        name: &ScriptName,
        character: &Sheet,
    ) -> Result<std::collections::BTreeMap<String, f64>>;
    /// The rules' formula for a check. `roll(sides)` draws from the saved random stream.
    fn check(
        &self,
        name: &ScriptName,
        character: &Sheet,
        skill: &Key,
        difficulty: u32,
        roll: &mut dyn FnMut(u32) -> Result<u32>,
    ) -> Result<bool>;
}
/// The engine running a content set's scripts. It is not part of the content's value:
/// two contents are equal when their definitions, including script sources, are equal.
#[derive(Clone, Default)]
pub struct Scripts(Option<Rc<dyn ScriptEngine>>);
impl Scripts {
    pub fn new(engine: Rc<dyn ScriptEngine>) -> Self {
        Self(Some(engine))
    }
    pub(crate) fn engine(&self, name: &ScriptName) -> Result<&dyn ScriptEngine> {
        let engine = self
            .0
            .as_deref()
            .ok_or_else(|| Invalid(format!("no script engine is installed for {name}")))?;
        require(engine.exports(name), &format!("unknown script {name}"))?;
        Ok(engine)
    }
}
impl PartialEq for Scripts {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for Scripts {}
impl std::fmt::Debug for Scripts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Scripts(installed)"
        } else {
            "Scripts(none)"
        })
    }
}

/// What any script may look at. `player` and `speaker` are the pair the rule is about.
#[derive(Clone, Copy)]
pub struct ReadScope<'a> {
    pub(crate) content: &'a GameContent,
    pub(crate) state: &'a SessionState,
    pub player: ActorId,
    pub speaker: ActorId,
    pub(crate) others: &'a BTreeSet<ActorId>,
}
impl ReadScope<'_> {
    pub fn item_count(&self, actor: ActorId, item: ItemDefinitionId) -> Result<u64> {
        self.state.item_count(self.content, actor, item)
    }
    pub fn quest_status(&self, quest: QuestId) -> Result<quests::Status> {
        self.content.quest(quest)?;
        Ok(self.state.quest(quest).status)
    }
    pub fn objective_completed(&self, quest: QuestId, objective: &Key) -> Result<bool> {
        self.content.quest(quest)?.objective(objective)?;
        Ok(self.state.quest(quest).completed.contains(objective))
    }
    pub fn relationship(&self, from: ActorId, to: ActorId) -> Result<i16> {
        self.state.actor(from)?;
        self.state.actor(to)?;
        Ok(self
            .state
            .relationship(crate::actors::RelationshipKey { from, to })
            .attitude)
    }
    pub fn present(&self, actor: ActorId) -> bool {
        actor == self.player
            || actor == self.speaker
            || self.others.contains(&actor)
            || self.state.party.members.contains(&actor)
    }
    /// `actor` names whose value, for a per-actor variable.
    pub fn variable(&self, variable: VariableId, actor: Option<ActorId>) -> Result<Value> {
        self.state
            .variable(self.content, crate::VariableKey { variable, actor })
    }
    pub fn inside_area(&self, actor: ActorId, area: AreaId) -> Result<bool> {
        self.content.area(area)?;
        self.state.actor(actor)?;
        Ok(self.state.areas(actor).contains(&area))
    }
    /// A stat after equipment and effects, or how much of a resource the actor has now.
    pub fn stat(&self, actor: ActorId, stat: &Key) -> Result<i32> {
        self.state.stat(self.content, actor, stat)
    }
    pub fn skill(&self, actor: ActorId, skill: &Key) -> Result<u8> {
        self.content.game.rules.skill(skill)?;
        Ok(self.state.actor(actor)?.skill(skill))
    }
    pub fn level(&self, actor: ActorId) -> Result<u32> {
        Ok(self.state.actor(actor)?.level)
    }
    pub fn class(&self, actor: ActorId) -> Result<&Key> {
        Ok(&self.state.actor(actor)?.class)
    }
    /// What is in the party's shared purse.
    pub fn gold(&self) -> Result<u64> {
        self.state.gold()
    }
    pub fn has_effect(&self, actor: ActorId, effect: &Key) -> Result<bool> {
        self.content.game.rules.effect(effect)?;
        Ok(self
            .state
            .actor(actor)?
            .effects
            .iter()
            .any(|e| &e.effect == effect))
    }
    /// How often a line was acknowledged or a choice picked in a conversation between the pair.
    pub fn history_count(&self, dialogue: DialogueId, node: &Key) -> Result<u64> {
        let contract = self.content.dialogue_contract(dialogue)?;
        require(contract.nodes.contains(node), "unknown history node")?;
        Ok(self
            .state
            .history(contract.history_key(self.player, self.speaker))
            .count(&crate::dialogue::HistoryEvent::Node(node.clone())))
    }
    pub fn locked(&self, object: ObjectId) -> Result<bool> {
        Ok(self.state.object(self.content, object)?.locked)
    }
    pub fn time(&self) -> GameTime {
        self.state.time
    }
}

/// No one besides the pair a rule is about.
pub(crate) static NOBODY: BTreeSet<ActorId> = BTreeSet::new();

/// What an action script may do, on top of everything it may read. Changes go through the
/// command's journal: the script sees them at once, and they are undone with the command if
/// it fails.
pub struct ActScope<'a, 'tx> {
    pub(crate) content: &'a GameContent,
    pub(crate) tx: &'a mut Tx<'tx>,
    pub player: ActorId,
    pub speaker: ActorId,
    pub(crate) others: &'a BTreeSet<ActorId>,
    pub(crate) events: &'a mut Vec<GameEvent>,
}
impl ActScope<'_, '_> {
    pub fn read(&self) -> ReadScope<'_> {
        ReadScope {
            content: self.content,
            state: self.tx,
            player: self.player,
            speaker: self.speaker,
            others: self.others,
        }
    }
    /// Runs a built-in action on behalf of `actor`, exactly as authored content would.
    pub fn apply(&mut self, actor: ActorId, action: &Action) -> Result<()> {
        self.content.validate_action(action, 0)?;
        crate::session::run_action(
            self.content,
            self.tx,
            actor,
            self.speaker,
            self.others,
            action,
            self.events,
        )
    }
    /// A skill check by the rules' own formula. Whatever it rolls comes from the saved
    /// random stream.
    pub fn check(&mut self, actor: ActorId, skill: &Key, difficulty: u32) -> Result<bool> {
        crate::character::check(self.content, self.tx, actor, skill, difficulty, self.events)
    }
    /// One of 1..=sides from the saved random stream.
    pub fn random(&mut self, sides: u32) -> Result<u32> {
        Ok(self.tx.random_mut().die(sides)?)
    }
}
