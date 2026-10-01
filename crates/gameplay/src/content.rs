use crate::Keyed;
use crate::actors::ActorTemplate;
use crate::dialogue::Dialogue;
use crate::inventory::ItemCatalog;
use crate::rules::Rules;
use crate::{
    InteractionProfile, NamedPredicate, Participant, Result, ScriptModule, ScriptName, Scripts,
};
use crate::{dialogue, quests};
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// What a variable can hold. A variable keeps the type of its initial value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
}
impl Value {
    pub fn same_type(&self, other: &Value) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
    fn validate(&self) -> game_types::Result<()> {
        match self {
            Self::Text(text) => require(text.len() <= 1024, "variable text exceeds 1024 bytes"),
            _ => Ok(()),
        }
    }
}
/// A named value that content and scripts read and change, kept for the whole playthrough.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableDefinition {
    pub id: VariableId,
    pub initial: Value,
    #[serde(default)]
    pub scope: VariableScope,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariableScope {
    /// One value for the playthrough.
    #[default]
    Playthrough,
    /// Every actor has a value of its own, e.g. what one NPC remembers.
    Actor,
}
/// Which stored value: the variable, and whose it is when the variable is per actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VariableKey {
    pub variable: VariableId,
    pub actor: Option<ActorId>,
}
impl From<VariableId> for VariableKey {
    fn from(variable: VariableId) -> Self {
        Self {
            variable,
            actor: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Test {
    Is(Value),
    AtLeast(i64),
    AtMost(i64),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Condition {
    /// A script function returning a boolean.
    Script(ScriptName),
    /// The actor takes part in the conversation or travels with the party.
    Present(ActorId),
    InsideArea {
        area: AreaId,
    },
    History {
        dialogue: DialogueId,
        event: dialogue::HistoryEvent,
        minimum: u64,
    },
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    Named(PredicateId),
    QuestStatus {
        quest: QuestId,
        status: quests::Status,
    },
    ObjectiveCompleted {
        quest: QuestId,
        objective: Key,
    },
    Relationship {
        from: Participant,
        to: Participant,
        minimum: i16,
    },
    HasItem {
        definition: ItemDefinitionId,
        quantity: u32,
    },
    Variable {
        variable: VariableId,
        /// Whose value, for a per-actor variable.
        #[serde(default)]
        of: Option<Participant>,
        test: Test,
    },
    /// A stat of the player's after equipment and effects, or how much of a resource is
    /// left.
    Stat {
        stat: Key,
        minimum: i32,
    },
    /// The player's rank in a skill.
    Skill {
        skill: Key,
        minimum: u8,
    },
    Level {
        minimum: u32,
    },
    Class(Key),
    /// A trainer could teach the player the next rank: the class allows it and the
    /// learning points are there.
    CanLearn {
        skill: Key,
    },
    /// The party's shared purse holds at least this much.
    Gold {
        minimum: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// A script function making changes through the script API.
    Script(ScriptName),
    SetLocked {
        object: ObjectId,
        locked: bool,
    },
    /// `then` when the condition holds, otherwise `otherwise`. With a variable that the
    /// actions set, it gives a reward once however often the choice is offered.
    If {
        condition: Condition,
        then: Vec<Action>,
        #[serde(default)]
        otherwise: Vec<Action>,
    },
    Quest {
        quest: QuestId,
        transition: quests::Transition,
    },
    Relationship {
        from: Participant,
        to: Participant,
        amount: i16,
    },
    ConsumeItem {
        definition: ItemDefinitionId,
        quantity: u32,
    },
    GrantItem {
        definition: ItemDefinitionId,
        quantity: u32,
    },
    /// Experience for the whole party; members gain the levels it earns.
    AwardExperience {
        amount: u64,
    },
    /// A trainer teaches the player the next rank of a skill for learning points.
    Teach {
        skill: Key,
    },
    /// Gold leaves the party's shared purse.
    Pay {
        amount: u64,
    },
    /// Adds to a resource, or takes from it with a negative amount: healing, a trap.
    ChangeResource {
        #[serde(default)]
        of: Participant,
        resource: Key,
        amount: i32,
    },
    ApplyEffect {
        #[serde(default)]
        of: Participant,
        effect: Key,
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    RemoveEffect {
        #[serde(default)]
        of: Participant,
        effect: Key,
    },
    /// Asks the engine to walk an actor into an area. `Arrived` or `MoveFailed` follows.
    Move {
        actor: Participant,
        to: AreaId,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Starts a conversation with the player once the current command is done. Without a
    /// speaker named, it is the speaker of the rule this action belongs to.
    StartDialogue {
        dialogue: DialogueId,
        #[serde(default)]
        speaker: Option<Participant>,
    },
    Set {
        variable: VariableId,
        #[serde(default)]
        of: Option<Participant>,
        value: Value,
    },
    /// Adds to a whole-number variable; the amount may be negative.
    Add {
        variable: VariableId,
        #[serde(default)]
        of: Option<Participant>,
        amount: i64,
    },
    /// A skill check by the rules' formula. Failing is an accepted outcome with its own
    /// effects. Invalid effects reject the entire command, restoring the random state along
    /// with everything else.
    Check {
        skill: Key,
        difficulty: u32,
        success: Vec<Action>,
        failure: Vec<Action>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentManifest {
    pub id: ContentId,
    pub revision: u64,
    /// Immutable world publication identifier supplied by the composition layer.
    pub world_generation: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameDefinitions {
    pub world: crate::WorldDefinitions,
    #[serde(with = "crate::keyed::list")]
    pub dialogue_contracts: BTreeMap<DialogueId, dialogue::DialogueContract>,
    #[serde(with = "crate::keyed::list")]
    pub quests: BTreeMap<QuestId, quests::Quest>,
    #[serde(with = "crate::keyed::list")]
    pub profiles: BTreeMap<InteractionProfileId, InteractionProfile>,
    #[serde(with = "crate::keyed::list")]
    pub predicates: BTreeMap<PredicateId, NamedPredicate>,
    pub rules: Rules,
    #[serde(with = "crate::keyed::list")]
    pub actors: BTreeMap<ActorTemplateId, ActorTemplate>,
    /// Characters the content refers to by name.
    #[serde(default, with = "crate::keyed::list")]
    pub characters: BTreeMap<ActorId, crate::actors::CharacterDefinition>,
    #[serde(with = "crate::keyed::list")]
    pub dialogues: BTreeMap<DialogueId, Dialogue>,
    #[serde(with = "crate::keyed::list")]
    pub variables: BTreeMap<VariableId, VariableDefinition>,
    #[serde(default, with = "crate::keyed::list")]
    pub scripts: BTreeMap<Key, ScriptModule>,
    #[serde(default, with = "crate::keyed::list")]
    pub loot: BTreeMap<LootId, crate::inventory::LootTable>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameContent {
    #[serde(with = "crate::keyed::list")]
    pub text: BTreeMap<TextResourceId, TextContract>,
    pub manifest: ContentManifest,
    pub items: ItemCatalog,
    pub game: GameDefinitions,
    /// The engine running `game.scripts`, installed by whoever loaded the content.
    #[serde(skip)]
    pub scripts: Scripts,
    /// Where dialogue graphs not in `game.dialogues` are read from; see `dialogue`.
    #[serde(skip)]
    pub graphs: crate::Graphs,
}
impl Action {
    /// The participants this action names itself, not those of actions inside it.
    pub fn participants(&self) -> Vec<&Participant> {
        match self {
            Self::Relationship { from, to, .. } => vec![from, to],
            Self::ChangeResource { of, .. }
            | Self::ApplyEffect { of, .. }
            | Self::RemoveEffect { of, .. } => vec![of],
            Self::Move { actor, .. } => vec![actor],
            Self::StartDialogue { speaker, .. } => speaker.iter().collect(),
            Self::Set { of, .. } | Self::Add { of, .. } => of.iter().collect(),
            _ => vec![],
        }
    }
}
/// Names the record a mistake in content was found in.
pub(crate) fn within(
    record: impl std::fmt::Display,
    check: impl FnOnce() -> Result<()>,
) -> Result<()> {
    check().map_err(|error| match error {
        crate::GameplayError::Invalid(Invalid(message)) => {
            Invalid(format!("{record}: {message}")).into()
        }
        other => other,
    })
}
/// Each record is filed under its own identity.
pub(crate) fn filed<V: Keyed>(map: &BTreeMap<V::Key, V>, what: &str) -> Result<()> {
    require(
        map.iter().all(|(key, value)| *key == value.key()),
        &format!("{what} filed under another identity"),
    )
    .map_err(Into::into)
}
impl GameContent {
    pub fn validate(&self) -> Result<()> {
        require(
            self.manifest.revision > 0
                && !self.manifest.world_generation.is_empty()
                && self.manifest.world_generation.len() <= 256,
            "invalid content manifest",
        )?;
        filed(&self.text, "text resources")?;
        for contract in self.text.values() {
            contract.validate()?;
        }
        filed(&self.game.variables, "variables")?;
        for variable in self.game.variables.values() {
            variable.initial.validate()?;
        }
        filed(&self.game.scripts, "script modules")?;
        self.validate_world()?;
        self.items.validate()?;
        self.game.rules.validate()?;
        for name in [&self.game.rules.derive, &self.game.rules.check] {
            self.scripts.engine(name)?;
        }
        for ability in &self.game.rules.abilities {
            self.scripts.engine(&ability.resolve)?;
        }
        for item in self.items.items.values() {
            self.game.rules.validate_mechanics(&item.mechanics)?;
        }
        filed(&self.game.loot, "loot tables")?;
        for table in self.game.loot.values() {
            table.validate(&self.items)?;
        }
        filed(&self.game.characters, "characters")?;
        for character in self.game.characters.values() {
            within(format!("character {}", character.id), || {
                self.template(character.template)?;
                match &character.name {
                    Some(name) => Ok(name.validate()?),
                    None => Ok(()),
                }
            })?;
        }
        filed(&self.game.actors, "actor templates")?;
        for actor in self.game.actors.values() {
            actor.validate(&self.game.rules)?;
            if let Some(loot) = actor.loot {
                self.loot(loot)?;
            }
            let mut slots = BTreeSet::new();
            for item in &actor.equipment {
                let slot = self.items.item(*item)?.mechanics.slot.as_ref();
                require(
                    slot.is_some_and(|slot| slots.insert(slot)),
                    &format!(
                        "template {}: {item} is not equipment or its slot is taken",
                        actor.id
                    ),
                )?;
            }
        }
        for object in self.game.world.objects.values() {
            if let crate::ObjectKind::Container {
                loot: Some(loot), ..
            } = object.kind
            {
                self.loot(loot)?;
            }
        }
        filed(&self.game.dialogue_contracts, "dialogue contracts")?;
        for contract in self.game.dialogue_contracts.values() {
            within(format!("dialogue {}", contract.id), || {
                contract.validate()?;
                for role in contract.roles.values() {
                    if let crate::dialogue::Role::Actor(actor) = role {
                        self.character(*actor)?;
                    }
                }
                Ok(())
            })?;
        }
        filed(&self.game.dialogues, "dialogues")?;
        for graph in self.game.dialogues.values() {
            within(format!("dialogue {}", graph.id), || {
                graph.validate()?;
                require(
                    self.dialogue_contract(graph.id)? == &graph.contract(),
                    "dialogue contract mismatch",
                )?;
                self.validate_dialogue_text(graph)?;
                for node in &graph.nodes {
                    within(format!("node {}", node.id), || {
                        if let Some(condition) = &node.condition {
                            self.validate_condition(condition)?;
                        }
                        for action in &node.actions {
                            self.validate_action(action, 0)?;
                        }
                        Ok(())
                    })?;
                }
                Ok(())
            })?;
        }
        filed(&self.game.quests, "quests")?;
        filed(&self.game.predicates, "named predicates")?;
        filed(&self.game.profiles, "interaction profiles")?;
        for quest in self.game.quests.values() {
            within(format!("quest {}", quest.id), || Ok(quest.validate()?))?;
        }
        for rule in self.game.predicates.values() {
            within(format!("predicate {}", rule.id), || {
                self.validate_condition(&rule.condition)
            })?;
        }
        for profile in self.game.profiles.values() {
            within(format!("interaction profile {}", profile.id), || {
                profile.validate()?;
                for rule in &profile.rules {
                    self.validate_condition(&rule.condition)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
    /// Actions nest only so deep, so running them cannot exhaust the stack.
    pub(crate) fn validate_action(&self, action: &Action, depth: usize) -> Result<()> {
        require(depth <= 8, "actions nested too deeply")?;
        for participant in action.participants() {
            self.validate_participant(participant)?;
        }
        match action {
            Action::Script(name) => {
                self.scripts.engine(name)?;
            }
            Action::SetLocked { object, .. } => {
                self.object(*object)?;
            }
            Action::If {
                condition,
                then,
                otherwise,
            } => {
                self.validate_condition(condition)?;
                require(
                    !then.is_empty() || !otherwise.is_empty(),
                    "a conditional action does nothing either way",
                )?;
                for action in then.iter().chain(otherwise) {
                    self.validate_action(action, depth + 1)?;
                }
            }
            Action::Quest { quest, transition } => {
                let q = self.quest(*quest)?;
                if let quests::Transition::CompleteObjective(id) = transition {
                    q.objective(id)?;
                }
            }
            Action::Relationship { from, to, .. } => {
                require(from != to, "relationship needs two roles")?
            }
            Action::GrantItem {
                definition,
                quantity,
            }
            | Action::ConsumeItem {
                definition,
                quantity,
            } => {
                self.items.item(*definition)?;
                require(*quantity > 0, "zero item action")?;
            }
            Action::AwardExperience { amount } => {
                require(*amount > 0, "zero XP reward")?;
            }
            Action::Teach { skill } => {
                self.game.rules.skill(skill)?;
            }
            Action::Pay { amount } => require(*amount > 0, "paying nothing")?,
            Action::ChangeResource {
                resource, amount, ..
            } => {
                self.game.rules.resource(resource)?;
                require(*amount != 0, "changing a resource by nothing")?;
            }
            Action::ApplyEffect {
                effect,
                duration_ms,
                ..
            } => {
                self.game.rules.effect(effect)?;
                require(
                    duration_ms.is_none_or(|ms| ms > 0),
                    "effect duration must be positive",
                )?;
            }
            Action::RemoveEffect { effect, .. } => {
                self.game.rules.effect(effect)?;
            }
            Action::Move { to, timeout_ms, .. } => {
                self.area(*to)?;
                require(
                    timeout_ms.is_none_or(|ms| ms > 0 && ms <= 86_400_000),
                    "movement timeout must be 1ms..1 day",
                )?;
            }
            Action::StartDialogue { dialogue, .. } => {
                self.dialogue_contract(*dialogue)?;
            }
            Action::Set {
                variable,
                of,
                value,
            } => {
                value.validate()?;
                require(
                    self.variable_use(*variable, of)?.initial.same_type(value),
                    "value has a different type than the variable",
                )?
            }
            Action::Add { variable, of, .. } => require(
                matches!(self.variable_use(*variable, of)?.initial, Value::Int(_)),
                "only whole-number variables can be added to",
            )?,
            Action::Check {
                skill,
                difficulty,
                success,
                failure,
            } => {
                self.game.rules.skill(skill)?;
                require(*difficulty > 0, "zero check difficulty")?;
                for action in success.iter().chain(failure) {
                    self.validate_action(action, depth + 1)?;
                }
            }
        }
        Ok(())
    }
    /// A dialogue graph. Content assembled by a tool holds them all; a session's content
    /// reads one from its source the first time it is needed and keeps the most recently
    /// used. A graph that cannot be read, or that does not match its contract, is an error
    /// for whatever needed it.
    pub fn dialogue(&self, id: DialogueId) -> Result<Rc<Dialogue>> {
        if let Some(graph) = self.graphs.cached(id) {
            return Ok(graph);
        }
        let graph = match self.authored(id) {
            Some(graph) => graph.clone(),
            None => {
                let contract = self.dialogue_contract(id)?;
                let graph = self
                    .graphs
                    .read(id)
                    .ok_or_else(|| Invalid(format!("unknown dialogue {id}")))??;
                graph.validate()?;
                require(
                    graph.id == id && &graph.contract() == contract,
                    "dialogue contract mismatch",
                )?;
                graph
            }
        };
        Ok(self.graphs.keep(graph))
    }
    /// The graph if it is in memory already; nothing is read.
    pub fn loaded_dialogue(&self, id: DialogueId) -> Option<Rc<Dialogue>> {
        self.graphs
            .cached(id)
            .or_else(|| Some(self.graphs.keep(self.authored(id)?.clone())))
    }
    /// A graph the content was assembled with.
    pub(crate) fn authored(&self, id: DialogueId) -> Option<&Dialogue> {
        self.game.dialogues.get(&id)
    }
    pub fn variable(&self, id: VariableId) -> Result<&VariableDefinition> {
        let variable = self.game.variables.get(&id);
        variable.ok_or_else(|| Invalid(format!("unknown variable {id}")).into())
    }
    /// The definition, after checking that an actor is named exactly when the variable is
    /// per actor.
    pub fn variable_use(
        &self,
        id: VariableId,
        of: &Option<Participant>,
    ) -> Result<&VariableDefinition> {
        let definition = self.variable(id)?;
        require(
            (definition.scope == VariableScope::Actor) == of.is_some(),
            &format!("variable {id} is per actor exactly when `of` names one"),
        )?;
        Ok(definition)
    }
    pub fn loot(&self, id: LootId) -> Result<&crate::inventory::LootTable> {
        let table = self.game.loot.get(&id);
        table.ok_or_else(|| Invalid(format!("unknown loot table {id}")).into())
    }
    /// A declared character.
    pub fn character(&self, id: ActorId) -> Result<&crate::actors::CharacterDefinition> {
        let character = self.game.characters.get(&id);
        character.ok_or_else(|| Invalid(format!("unknown character {id}")).into())
    }
    /// What an actor is called: its own name in this playthrough, else its character's,
    /// else its template's.
    pub fn actor_name<'a>(&'a self, actor: &'a crate::actors::Actor) -> Result<&'a TextRef> {
        if let Some(name) = &actor.name_override {
            return Ok(name);
        }
        let character = self.game.characters.get(&actor.id);
        match character.and_then(|c| c.name.as_ref()) {
            Some(name) => Ok(name),
            None => Ok(&self.template(actor.template)?.name),
        }
    }
    /// A participant naming a character names a declared one.
    pub(crate) fn validate_participant(&self, participant: &Participant) -> Result<()> {
        if let Participant::Actor(id) = participant {
            self.character(*id)?;
        }
        Ok(())
    }
    pub fn template(&self, id: ActorTemplateId) -> Result<&ActorTemplate> {
        let template = self.game.actors.get(&id);
        template.ok_or_else(|| Invalid(format!("unknown actor template {id}")).into())
    }
    /// Stable fingerprint of definitions, rules and world binding; excludes .ftl resources.
    pub fn fingerprint(&self) -> Result<[u8; 32]> {
        self.validate()?;
        self.validate_selection_links()?;
        // Every definition is in a map, so it serializes in order of identity whatever order
        // it was added in. Identities as bytes: a name shows only once something has parsed
        // it, and the fingerprint must not depend on that.
        let bytes = names::raw(|| serde_json::to_vec(self)).map_err(|e| Invalid(e.to_string()))?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }
    pub fn text_keys(&self) -> Vec<&MessageRef> {
        let categories = self.items.categories.values();
        let mut refs: Vec<&TextRef> = categories.map(|c| &c.name).collect();
        for item in self.items.items.values() {
            refs.extend([&item.name, &item.description]);
        }
        refs.extend(self.game.world.objects.values().map(|o| &o.name));
        refs.extend(self.game.actors.values().map(|a| &a.name));
        refs.extend(
            self.game
                .characters
                .values()
                .filter_map(|c| c.name.as_ref()),
        );
        refs.extend(self.game.rules.names());
        for quest in self.game.quests.values() {
            refs.push(&quest.title);
            refs.extend(quest.objectives.iter().map(|o| &o.title));
        }
        for profile in self.game.profiles.values() {
            refs.extend(profile.rules.iter().filter_map(|r| r.topic.as_ref()));
        }
        for graph in self.game.dialogues.values() {
            for (text, arguments) in graph.messages() {
                refs.push(text);
                refs.extend(arguments.values().filter_map(|a| match a {
                    dialogue::ArgumentSource::Text(t) => Some(t),
                    _ => None,
                }));
            }
        }
        refs.into_iter()
            .filter_map(|r| {
                if let TextRef::Message(k) = r {
                    Some(k)
                } else {
                    None
                }
            })
            .collect()
    }
}
