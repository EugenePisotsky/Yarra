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
    Claimed {
        claim: ClaimId,
        value: bool,
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
    Claim {
        claim: ClaimId,
        actions: Vec<Action>,
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
    pub dialogue_contracts: Vec<dialogue::DialogueContract>,
    pub claims: Vec<dialogue::ClaimDefinition>,
    pub quests: Vec<quests::Quest>,
    pub profiles: Vec<InteractionProfile>,
    pub predicates: Vec<NamedPredicate>,
    pub rules: Rules,
    pub actors: Vec<ActorTemplate>,
    pub dialogues: Vec<Dialogue>,
    pub variables: Vec<VariableDefinition>,
    #[serde(default)]
    pub scripts: Vec<ScriptModule>,
    #[serde(default)]
    pub loot: Vec<crate::inventory::LootTable>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameContent {
    pub text: Vec<TextContract>,
    pub manifest: ContentManifest,
    pub items: ItemCatalog,
    pub game: GameDefinitions,
    /// The engine running `game.scripts`, installed by whoever loaded the content.
    #[serde(skip)]
    pub scripts: Scripts,
}
/// Finds a definition in a list kept in order of identity.
pub(crate) fn find<T, K: Ord + Copy>(list: &[T], id: K, of: impl Fn(&T) -> K) -> Option<&T> {
    let index = list.binary_search_by_key(&id, of).ok()?;
    Some(&list[index])
}
/// Each identity once and in order, which is what `find` relies on.
pub(crate) fn ordered<T, K: Ord>(list: &[T], of: impl Fn(&T) -> K, what: &str) -> Result<()> {
    require(
        list.windows(2).all(|pair| of(&pair[0]) < of(&pair[1])),
        &format!("{what} must be listed once each in order of identity; see GameContent::sort"),
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
        let mut text_ids = BTreeSet::new();
        for contract in &self.text {
            contract.validate()?;
            require(text_ids.insert(contract.id), "duplicate text resource")?;
        }
        let contracts: BTreeMap<_, _> = self.text.iter().map(|c| (c.id, c)).collect();
        fn visit_text(
            id: TextResourceId,
            contracts: &BTreeMap<TextResourceId, &TextContract>,
            active: &mut BTreeSet<TextResourceId>,
            visited: &mut BTreeSet<TextResourceId>,
        ) -> Result<()> {
            require(!active.contains(&id), "text import cycle")?;
            if visited.contains(&id) {
                return Ok(());
            }
            require(active.len() < 256, "text import depth exceeds limit")?;
            let contract = contracts
                .get(&id)
                .ok_or_else(|| Invalid("unknown text import".into()))?;
            active.insert(id);
            for import in &contract.imports {
                visit_text(*import, contracts, active, visited)?;
            }
            active.remove(&id);
            visited.insert(id);
            Ok(())
        }
        let mut visited = BTreeSet::new();
        for id in contracts.keys() {
            visit_text(*id, &contracts, &mut BTreeSet::new(), &mut visited)?;
        }
        ordered(&self.game.variables, |v| v.id, "variables")?;
        for variable in &self.game.variables {
            variable.initial.validate()?;
        }
        let mut modules = BTreeSet::new();
        for module in &self.game.scripts {
            require(modules.insert(&module.name), "duplicate script module")?;
        }
        self.validate_world()?;
        self.items.validate()?;
        self.game.rules.validate()?;
        for name in [&self.game.rules.derive, &self.game.rules.check] {
            self.scripts.engine(name)?;
        }
        for ability in &self.game.rules.abilities {
            self.scripts.engine(&ability.resolve)?;
        }
        for item in &self.items.items {
            self.game.rules.validate_mechanics(&item.mechanics)?;
        }
        ordered(&self.game.loot, |v| v.id, "loot tables")?;
        for table in &self.game.loot {
            table.validate(&self.items)?;
        }
        ordered(&self.game.actors, |v| v.id, "actor templates")?;
        for actor in &self.game.actors {
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
        for object in &self.game.world.objects {
            if let crate::ObjectKind::Container {
                loot: Some(loot), ..
            } = object.kind
            {
                self.loot(loot)?;
            }
        }
        ordered(
            &self.game.dialogue_contracts,
            |v| v.id,
            "dialogue contracts",
        )?;
        for contract in &self.game.dialogue_contracts {
            contract.validate()?;
        }
        ordered(&self.game.claims, |v| v.id, "claims")?;
        let mut graphs = BTreeSet::new();
        for graph in &self.game.dialogues {
            require(graphs.insert(graph.id), "duplicate dialogue")?;
            graph.validate()?;
            require(
                self.dialogue_contract(graph.id)? == &graph.contract(),
                "dialogue contract mismatch",
            )?;
            self.validate_dialogue_text(graph)?;
            for node in &graph.nodes {
                if let Some(condition) = &node.condition {
                    self.validate_condition(condition)?;
                }
                for action in &node.actions {
                    self.validate_action(action, 0)?;
                }
            }
        }
        ordered(&self.game.quests, |v| v.id, "quests")?;
        ordered(&self.game.predicates, |v| v.id, "named predicates")?;
        ordered(&self.game.profiles, |v| v.id, "interaction profiles")?;
        for quest in &self.game.quests {
            quest.validate()?;
        }
        for rule in &self.game.predicates {
            self.validate_condition(&rule.condition)?;
        }
        for profile in &self.game.profiles {
            profile.validate()?;
            for rule in &profile.rules {
                self.validate_condition(&rule.condition)?;
            }
        }
        Ok(())
    }
    /// Actions nest only so deep, so running them cannot exhaust the stack.
    pub(crate) fn validate_action(&self, action: &Action, depth: usize) -> Result<()> {
        require(depth <= 8, "actions nested too deeply")?;
        match action {
            Action::Script(name) => {
                self.scripts.engine(name)?;
            }
            Action::SetLocked { object, .. } => {
                self.object(*object)?;
            }
            Action::Claim { claim, actions } => {
                self.claim(*claim)?;
                require(!actions.is_empty(), "empty claimed action group")?;
                for action in actions {
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
    /// At runtime only recently used graphs are present; sessions load them on demand.
    pub fn loaded_dialogue(&self, id: DialogueId) -> Option<&Dialogue> {
        self.game.dialogues.iter().find(|d| d.id == id)
    }
    pub fn dialogue(&self, id: DialogueId) -> Result<&Dialogue> {
        self.loaded_dialogue(id)
            .ok_or_else(|| Invalid("unknown dialogue".into()).into())
    }
    pub fn variable(&self, id: VariableId) -> Result<&VariableDefinition> {
        find(&self.game.variables, id, |v| v.id)
            .ok_or_else(|| Invalid(format!("unknown variable {id}")).into())
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
    /// Puts every list of definitions in order of identity. Lookups bisect, so content is
    /// sorted once when it is loaded or built and checked to be so by `validate`.
    pub fn sort(&mut self) {
        self.items.sort();
        self.text.sort_by_key(|v| v.id);
        let game = &mut self.game;
        game.world.objects.sort_by_key(|v| v.id);
        game.world.triggers.sort_by_key(|v| v.id);
        game.dialogue_contracts.sort_by_key(|v| v.id);
        game.claims.sort_by_key(|v| v.id);
        game.quests.sort_by_key(|v| v.id);
        game.profiles.sort_by_key(|v| v.id);
        game.predicates.sort_by_key(|v| v.id);
        game.actors.sort_by_key(|v| v.id);
        game.variables.sort_by_key(|v| v.id);
        game.loot.sort_by_key(|v| v.id);
        game.scripts.sort_by(|a, b| a.name.cmp(&b.name));
    }
    pub fn loot(&self, id: LootId) -> Result<&crate::inventory::LootTable> {
        find(&self.game.loot, id, |v| v.id)
            .ok_or_else(|| Invalid(format!("unknown loot table {id}")).into())
    }
    pub fn template(&self, id: ActorTemplateId) -> Result<&ActorTemplate> {
        find(&self.game.actors, id, |v| v.id)
            .ok_or_else(|| Invalid("unknown actor template".into()).into())
    }
    /// Stable fingerprint of definitions, rules and world binding; excludes .ftl resources.
    pub fn fingerprint(&self) -> Result<[u8; 32]> {
        self.validate()?;
        self.validate_selection_links()?;
        let mut canonical = self.clone();
        canonical.game.world.objects.sort_by_key(|v| v.id);
        canonical.game.world.triggers.sort_by_key(|v| v.id);
        canonical.text.sort_by_key(|t| t.id);
        canonical.items.categories.sort_by_key(|v| v.id);
        canonical.items.items.sort_by_key(|v| v.id);
        canonical.game.dialogue_contracts.sort_by_key(|v| v.id);
        canonical.game.claims.sort_by_key(|v| v.id);
        canonical.game.quests.sort_by_key(|v| v.id);
        canonical.game.profiles.sort_by_key(|v| v.id);
        canonical.game.predicates.sort_by_key(|v| v.id);
        canonical.game.actors.sort_by_key(|v| v.id);
        canonical.game.dialogues.sort_by_key(|v| v.id);
        canonical.game.variables.sort_by_key(|v| v.id);
        canonical.game.loot.sort_by_key(|v| v.id);
        canonical.game.scripts.sort_by(|a, b| a.name.cmp(&b.name));
        // Identities as bytes: a name shows only once something has parsed it, and the
        // fingerprint must not depend on that.
        let bytes =
            names::raw(|| serde_json::to_vec(&canonical)).map_err(|e| Invalid(e.to_string()))?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }
    pub fn text_keys(&self) -> Vec<&MessageRef> {
        let mut refs: Vec<&TextRef> = self.items.categories.iter().map(|c| &c.name).collect();
        for item in &self.items.items {
            refs.extend([&item.name, &item.description]);
        }
        refs.extend(self.game.world.objects.iter().map(|o| &o.name));
        refs.extend(self.game.actors.iter().map(|a| &a.name));
        refs.extend(self.game.rules.names());
        for quest in &self.game.quests {
            refs.push(&quest.title);
            refs.extend(quest.objectives.iter().map(|o| &o.title));
        }
        for profile in &self.game.profiles {
            refs.extend(profile.rules.iter().filter_map(|r| r.topic.as_ref()));
        }
        for graph in &self.game.dialogues {
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
