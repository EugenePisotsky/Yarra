//! Content as authored, whole in memory: what tools, tests and the publisher hold. A session
//! reads it through `ToolContent` exactly as it reads a published bundle.
use crate::content::{filed, within};
use crate::dialogue::{ArgumentSource, Dialogue};
use crate::{ContentIdentity, ContentSource, GameContent, Result};
use game_types::*;
use serde::Serialize;
use std::collections::BTreeMap;
use std::rc::Rc;

/// The definitions a playthrough reads up front, every dialogue graph, and the text
/// contracts of the messages the content names.
#[derive(Debug, Clone)]
pub struct AuthoredContent {
    pub core: GameContent,
    pub dialogues: BTreeMap<DialogueId, Dialogue>,
    pub text: BTreeMap<TextResourceId, TextContract>,
}

impl AuthoredContent {
    /// What a publication is checked for: the core, every graph against its contract, and
    /// every message the content names against the text contracts.
    pub fn validate(&self) -> Result<()> {
        let core = &self.core;
        core.validate()?;
        filed(&self.text, "text resources")?;

        for contract in self.text.values() {
            contract.validate()?;
        }

        filed(&self.dialogues, "dialogues")?;
        require(
            self.dialogues
                .keys()
                .eq(core.game.dialogue_contracts.keys()),
            "every dialogue has a contract and every contract a dialogue",
        )?;

        for graph in self.dialogues.values() {
            within(format!("dialogue {}", graph.id), || {
                graph.validate()?;
                require(
                    core.dialogue_contract(graph.id)? == &graph.contract(),
                    "dialogue contract mismatch",
                )?;
                self.validate_dialogue_text(graph)?;

                for node in &graph.nodes {
                    within(format!("node {}", node.id), || {
                        if let Some(condition) = &node.condition {
                            core.validate_condition(condition)?;
                        }

                        for action in &node.actions {
                            core.validate_action(action, 0)?;
                        }

                        Ok(())
                    })?;
                }

                Ok(())
            })?;
        }

        for reference in self.text_keys() {
            self.message(reference)?;
        }

        // A name or description is shown on its own, with nothing to fill in.
        for reference in self.names() {
            require(
                self.message(reference)?.arguments.is_empty(),
                &format!(
                    "{}/{} is shown on its own and cannot require arguments",
                    reference.resource,
                    reference.key.as_str()
                ),
            )?;
        }

        Ok(())
    }

    fn message(&self, reference: &MessageRef) -> Result<&MessageContract> {
        let contract = self.text.get(&reference.resource);
        contract
            .and_then(|c| c.messages.get(&reference.key))
            .ok_or_else(|| {
                let (resource, key) = (reference.resource, reference.key.as_str());
                Invalid(format!("undeclared message {resource}/{key}")).into()
            })
    }

    fn validate_dialogue_text(&self, graph: &Dialogue) -> Result<()> {
        graph.validate_messages(&|m| {
            self.text
                .get(&m.resource)
                .and_then(|c| c.messages.get(&m.key))
                .ok_or_else(|| Invalid("unknown dialogue message contract".into()))
        })?;

        for (_, args) in graph.messages() {
            for source in args.values() {
                if let ArgumentSource::Stat { stat, .. } = source {
                    self.core.game.rules.stat(stat)?;
                }
            }
        }

        Ok(())
    }

    /// Identity of the mechanics: the definitions, every graph, and what each message takes,
    /// which the graphs fill in. The wording is no part of it, so a translation or a reworded
    /// line keeps saves' identity.
    pub fn fingerprint(&self) -> Result<[u8; 32]> {
        #[derive(Serialize)]
        struct Mechanics<'a> {
            manifest: &'a crate::ContentManifest,
            items: &'a crate::inventory::ItemCatalog,
            game: &'a crate::GameDefinitions,
            dialogues: Vec<&'a Dialogue>,
            text: Vec<&'a TextContract>,
        }

        let mechanics = Mechanics {
            manifest: &self.core.manifest,
            items: &self.core.items,
            game: &self.core.game,
            dialogues: self.dialogues.values().collect(),
            text: self.text.values().collect(),
        };

        // Definitions are in maps, so they serialize in order of identity whatever order they
        // were added in. Identities as bytes: a name shows only once something has parsed it,
        // and the fingerprint must not depend on that.
        let bytes =
            names::raw(|| serde_json::to_vec(&mechanics)).map_err(|e| Invalid(e.to_string()))?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }

    /// Messages shown on their own: the names and descriptions of what the content defines.
    pub fn names(&self) -> Vec<&MessageRef> {
        let (core, items) = (&self.core.game, &self.core.items);
        let mut refs: Vec<&TextRef> = items.categories.values().map(|c| &c.name).collect();

        for item in items.items.values() {
            refs.extend([&item.name, &item.description]);
        }

        refs.extend(core.world.objects.values().map(|o| &o.name));
        refs.extend(core.actors.values().map(|a| &a.name));
        refs.extend(core.characters.values().filter_map(|c| c.name.as_ref()));
        refs.extend(core.rules.names());

        for quest in core.quests.values() {
            refs.push(&quest.title);
            refs.extend(quest.objectives.iter().map(|o| &o.title));
        }

        for profile in core.profiles.values() {
            refs.extend(profile.rules.iter().filter_map(|r| r.topic.as_ref()));
        }

        messages(refs)
    }

    /// Every message the content names: its names, and the lines of its dialogue graphs with
    /// the text they fill in.
    pub fn text_keys(&self) -> Vec<&MessageRef> {
        let mut refs = self.names();

        for graph in self.dialogues.values() {
            for (text, arguments) in graph.messages() {
                refs.extend(messages([text]));
                refs.extend(messages(arguments.values().filter_map(|a| match a {
                    ArgumentSource::Text(t) => Some(t),
                    _ => None,
                })));
            }
        }

        refs
    }
}

fn messages<'a>(refs: impl IntoIterator<Item = &'a TextRef>) -> Vec<&'a MessageRef> {
    refs.into_iter()
        .filter_map(|r| match r {
            TextRef::Message(m) => Some(m),
            TextRef::Literal(_) => None,
        })
        .collect()
}

/// Authored content checked once and served to sessions as a published bundle is. Clones
/// share the content.
#[derive(Debug, Clone)]
pub struct ToolContent {
    authored: Rc<AuthoredContent>,
    identity: ContentIdentity,
}

impl ToolContent {
    pub fn new(authored: AuthoredContent) -> Result<Self> {
        authored.validate()?;
        let identity = ContentIdentity {
            manifest: authored.core.manifest.clone(),
            fingerprint: authored.fingerprint()?,
        };
        Ok(Self {
            authored: Rc::new(authored),
            identity,
        })
    }

    pub fn authored(&self) -> &AuthoredContent {
        &self.authored
    }

    pub fn content(&self) -> &GameContent {
        &self.authored.core
    }
}

impl ContentSource for ToolContent {
    fn identity(&self) -> ContentIdentity {
        self.identity.clone()
    }

    fn core(&mut self) -> Result<GameContent> {
        Ok(self.authored.core.clone())
    }

    fn dialogue(&mut self, id: DialogueId) -> Result<Dialogue> {
        let graph = self.authored.dialogues.get(&id);
        graph
            .cloned()
            .ok_or_else(|| Invalid(format!("unknown dialogue {id}")).into())
    }
}
