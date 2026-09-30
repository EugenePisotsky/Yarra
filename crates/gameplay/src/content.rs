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
    SkillExperience {
        skill: Key,
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
    AwardExperience {
        skill: Key,
        amount: u64,
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
    /// A failed roll is an accepted outcome, with its own effects. Invalid effects
    /// reject the entire command, restoring random state along with domain state.
    SkillCheck {
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
        require(self.text.len() <= 2048, "too many text resources")?;
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
        let mut variables = BTreeSet::new();
        for variable in &self.game.variables {
            variable.initial.validate()?;
            require(variables.insert(variable.id), "duplicate variable")?;
        }
        let mut modules = BTreeSet::new();
        for module in &self.game.scripts {
            require(
                modules.insert(&module.name) && module.source.len() <= 1024 * 1024,
                "duplicate or oversized script module",
            )?;
        }
        self.validate_world()?;
        self.items.validate()?;
        self.game.rules.validate()?;
        require(
            self.game.actors.len() <= 10000
                && self.game.dialogues.len() <= 1000
                && self.game.variables.len() <= 10000,
            "content exceeds limits",
        )?;
        for item in &self.items.items {
            self.game.rules.validate_mechanics(&item.mechanics)?;
        }
        let mut actors = BTreeSet::new();
        for actor in &self.game.actors {
            require(actors.insert(actor.id), "duplicate actor template")?;
            actor.validate(&self.game.rules)?;
        }
        let mut contracts = BTreeSet::new();
        require(
            self.game.dialogue_contracts.len() <= 1000 && self.game.claims.len() <= 10000,
            "dialogue declaration limit",
        )?;
        for contract in &self.game.dialogue_contracts {
            contract.validate()?;
            require(contracts.insert(contract.id), "duplicate dialogue contract")?;
        }
        let mut claims = BTreeSet::new();
        for claim in &self.game.claims {
            require(claims.insert(claim.id), "duplicate claim definition")?;
        }
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
                let mut budget = 1024;
                for action in &node.actions {
                    self.validate_action(action, 0, &mut budget)?;
                }
            }
        }
        let mut quests = BTreeSet::new();
        let mut profiles = BTreeSet::new();
        let mut predicates = BTreeSet::new();
        require(
            self.game.quests.len() <= 10000
                && self.game.profiles.len() <= 10000
                && self.game.predicates.len() <= 10000,
            "narrative content exceeds limits",
        )?;
        for quest in &self.game.quests {
            require(quests.insert(quest.id), "duplicate quest")?;
            quest.validate()?;
        }
        for rule in &self.game.predicates {
            require(predicates.insert(rule.id), "duplicate named predicate")?;
            self.validate_condition(&rule.condition)?;
        }
        for profile in &self.game.profiles {
            require(profiles.insert(profile.id), "duplicate interaction profile")?;
            profile.validate()?;
            for rule in &profile.rules {
                self.validate_condition(&rule.condition)?;
            }
        }
        Ok(())
    }
    pub(crate) fn validate_action(
        &self,
        action: &Action,
        depth: usize,
        budget: &mut usize,
    ) -> Result<()> {
        require(
            depth <= 8 && *budget > 0,
            "dialogue action complexity exceeded",
        )?;
        *budget -= 1;
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
                    self.validate_action(action, depth + 1, budget)?;
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
            Action::AwardExperience { skill, amount } => {
                self.game.rules.skill(skill)?;
                require(*amount > 0, "zero XP reward")?;
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
            Action::SkillCheck {
                skill,
                difficulty,
                success,
                failure,
            } => {
                self.game.rules.skill(skill)?;
                require(*difficulty > 0, "zero check difficulty")?;
                for action in success.iter().chain(failure) {
                    self.validate_action(action, depth + 1, budget)?;
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
        self.game
            .variables
            .iter()
            .find(|v| v.id == id)
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
    pub fn template(&self, id: ActorTemplateId) -> Result<&ActorTemplate> {
        self.game
            .actors
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| Invalid("unknown actor template".into()).into())
    }
    /// Stable fingerprint of definitions, rules and world binding; excludes .ftl resources.
    pub fn fingerprint(&self) -> Result<[u8; 32]> {
        self.validate()?;
        self.validate_selection_links()?;
        let mut canonical = self.clone();
        canonical.game.world.objects.sort_by_key(|v| v.id);
        canonical.game.world.areas.sort_by_key(|v| v.id);
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
        canonical.game.scripts.sort_by(|a, b| a.name.cmp(&b.name));
        let bytes = serde_json::to_vec(&canonical).map_err(|e| Invalid(e.to_string()))?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }
    pub fn text_keys(&self) -> Vec<&MessageRef> {
        let mut refs: Vec<&TextRef> = self.items.categories.iter().map(|c| &c.name).collect();
        for item in &self.items.items {
            refs.extend([&item.name, &item.description]);
        }
        refs.extend(self.game.world.objects.iter().map(|o| &o.name));
        refs.extend(self.game.actors.iter().map(|a| &a.name));
        refs.extend(self.game.rules.attributes.iter().map(|a| &a.name));
        refs.extend(self.game.rules.skills.iter().map(|s| &s.name));
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
