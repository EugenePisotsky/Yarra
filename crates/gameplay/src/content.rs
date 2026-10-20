use crate::{
    Keyed, actors::ActorTemplate, dialogue::Dialogue, inventory::ItemCatalog, rules::Rules,
};

use crate::{InteractionProfile, NamedPredicate, Participant, Result, ScriptModule, Scripts};

use crate::{dialogue, quests};
use game_types::*;
use serde::{Deserialize, Serialize};

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

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

    pub(crate) fn validate(&self) -> game_types::Result<()> {
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
    pub variables: BTreeMap<VariableId, VariableDefinition>,
    #[serde(default, with = "crate::keyed::list")]
    pub scripts: BTreeMap<Key, ScriptModule>,
    #[serde(default, with = "crate::keyed::list")]
    pub loot: BTreeMap<LootId, crate::inventory::LootTable>,
}

/// What a playthrough reads up front: everything but dialogue graphs, which load when a
/// conversation needs them, and the wording, which only presentation reads.
#[derive(Debug, Clone)]
pub struct GameContent {
    pub manifest: ContentManifest,
    pub items: ItemCatalog,
    pub game: GameDefinitions,
    /// The engine running `game.scripts`, installed by whoever loaded the content.
    pub scripts: Scripts,
    /// Where dialogue graphs are read from, and those read lately; see `dialogue`.
    pub graphs: crate::Graphs,
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
    Ok(require_with(
        map.iter().all(|(key, value)| *key == value.key()),
        || format!("{what} filed under another identity"),
    )?)
}

impl GameContent {
    pub fn validate(&self) -> Result<()> {
        require(
            self.manifest.revision > 0
                && !self.manifest.world_generation.is_empty()
                && self.manifest.world_generation.len() <= 256,
            "invalid content manifest",
        )?;
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

            if let Some(profile) = actor.interaction {
                self.profile(profile)?;
            }

            let mut slots = BTreeSet::new();

            for item in &actor.equipment {
                let slot = self.items.item(*item)?.mechanics.slot.as_ref();
                require_with(slot.is_some_and(|slot| slots.insert(slot)), || {
                    format!(
                        "template {}: {item} is not equipment or its slot is taken",
                        actor.id
                    )
                })?;
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

                    for variant in &rule.variants {
                        self.dialogue_contract(variant.dialogue)?;
                    }
                }

                Ok(())
            })?;
        }

        Ok(())
    }

    /// A dialogue graph, read from the content's source the first time it is needed; the
    /// most recently used are kept. A graph that cannot be read, or that does not match its
    /// contract, is an error for whatever needed it.
    pub fn dialogue(&self, id: DialogueId) -> Result<Rc<Dialogue>> {
        if let Some(graph) = self.graphs.cached(id) {
            return Ok(graph);
        }

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
        Ok(self.graphs.keep(graph))
    }

    /// The graph if it is in memory already; nothing is read.
    pub fn loaded_dialogue(&self, id: DialogueId) -> Option<Rc<Dialogue>> {
        self.graphs.cached(id)
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
        require_with(
            (definition.scope == VariableScope::Actor) == of.is_some(),
            || format!("variable {id} is per actor exactly when `of` names one"),
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

    pub fn template(&self, id: ActorTemplateId) -> Result<&ActorTemplate> {
        let template = self.game.actors.get(&id);
        template.ok_or_else(|| Invalid(format!("unknown actor template {id}")).into())
    }
}
