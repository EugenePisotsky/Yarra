//! Explicit eager authoring source. Runtime uses the indexed repository with the same closure.
use crate::Result;
use crate::*;
use game_types::*;
use std::collections::BTreeSet;
pub struct ToolContent {
    content: GameContent,
    identity: ContentIdentity,
}
impl ToolContent {
    pub fn new(content: GameContent) -> Result<Self> {
        let identity = ContentIdentity {
            manifest: content.manifest.clone(),
            fingerprint: content.fingerprint()?,
        };
        Ok(Self { content, identity })
    }
}
impl ContentSource for ToolContent {
    fn identity(&self) -> ContentIdentity {
        self.identity.clone()
    }
    fn areas_at(&mut self, position: &actors::Position) -> Result<BTreeSet<AreaId>> {
        let ids: BTreeSet<_> = self
            .content
            .game
            .world
            .areas
            .iter()
            .filter(|a| a.contains(position))
            .map(|a| a.id)
            .collect();
        require(
            ids.len() <= MAX_AREA_OVERLAP,
            "area overlap budget exceeded",
        )?;
        Ok(ids)
    }
    fn next_trigger(
        &mut self,
        signal: &WorldSignal,
        after: Option<TriggerId>,
    ) -> Result<Option<TriggerId>> {
        let mut ids = BTreeSet::new();
        for d in &self.content.game.world.triggers {
            if after.is_none_or(|a| d.id > a) && d.subscriptions(&self.content)?.contains(signal) {
                ids.insert(d.id);
            }
        }
        Ok(ids.first().copied())
    }
    fn resolve(&mut self, request: &ContentRequest) -> Result<GameContent> {
        let full = &self.content;
        let mut result = GameContent {
            text: vec![],
            manifest: full.manifest.clone(),
            items: inventory::ItemCatalog {
                id: full.items.id,
                revision: full.items.revision,
                categories: vec![],
                items: vec![],
            },
            game: GameDefinitions {
                world: Default::default(),
                dialogue_contracts: vec![],
                claims: vec![],
                quests: vec![],
                profiles: vec![],
                predicates: vec![],
                rules: full.game.rules.clone(),
                actors: vec![],
                dialogues: vec![],
                facts: request.facts.clone(),
                conditions: Default::default(),
                actions: Default::default(),
            },
        };
        let mut needed = request.clone();
        for id in &request.triggers {
            let d = full.trigger(*id)?;
            d.content_dependencies(&mut needed)?;
            result.game.world.triggers.push(d.clone());
        }
        for id in &request.actors {
            result.game.actors.push(
                full.game
                    .actors
                    .iter()
                    .find(|a| a.id == *id)
                    .ok_or_else(|| Invalid("missing actor template".into()))?
                    .clone(),
            );
        }
        for id in &request.dialogues {
            let graph = full.dialogue(*id)?;
            needed.dialogue_contracts.insert(*id);
            for choice in graph.nodes.iter().flat_map(|n| &n.choices) {
                for key in &choice.conditions {
                    let id = BindingId::new(*id, key.clone());
                    let condition = &full.game.conditions[&id];
                    condition.content_dependencies(&mut needed)?;
                    result.game.conditions.insert(id, condition.clone());
                }
                for key in &choice.actions {
                    let id = BindingId::new(*id, key.clone());
                    let action = &full.game.actions[&id];
                    action.content_dependencies(&mut needed)?;
                    result.game.actions.insert(id, action.clone());
                }
            }
            result.game.dialogues.push(graph.clone());
        }
        for id in &request.profiles {
            let profile = full.profile(*id)?;
            needed.dialogue_contracts.extend(
                profile
                    .rules
                    .iter()
                    .flat_map(|r| r.variants.iter().map(|v| v.dialogue)),
            );
            for rule in &profile.rules {
                rule.condition.content_dependencies(&mut needed)?;
            }
            result.game.profiles.push(profile.clone());
        }
        let mut done = BTreeSet::new();
        while let Some(id) = needed.predicates.difference(&done).next().copied() {
            require(done.len() < 256, "predicate dependency budget exceeded")?;
            let condition = full.predicate(id)?;
            condition.content_dependencies(&mut needed)?;
            result.game.predicates.push(NamedPredicate {
                id,
                condition: condition.clone(),
            });
            done.insert(id);
        }
        for id in &needed.objects {
            result.game.world.objects.push(full.object(*id)?.clone());
        }
        for id in &needed.areas {
            result.game.world.areas.push(full.area(*id)?.clone());
        }
        for id in &needed.dialogue_contracts {
            result
                .game
                .dialogue_contracts
                .push(full.dialogue_contract(*id)?.clone());
        }
        for id in &needed.claims {
            result.game.claims.push(full.claim(*id)?.clone());
        }
        // Graph text contracts are mechanical validation metadata; no wording is loaded here.
        let mut text_ids: BTreeSet<_> = result.text_keys().iter().map(|m| m.resource).collect();
        let mut done_text = BTreeSet::new();
        while let Some(id) = text_ids.difference(&done_text).next().copied() {
            let contract = full
                .text
                .iter()
                .find(|c| c.id == id)
                .ok_or_else(|| Invalid("missing text contract".into()))?;
            text_ids.extend(&contract.imports);
            done_text.insert(id);
            result.text.push(contract.clone());
        }
        for id in &needed.quests {
            result.game.quests.push(full.quest(*id)?.clone());
        }
        result.game.facts = needed.facts;
        require(
            result.game.facts.is_subset(&full.game.facts),
            "unknown fact",
        )?;
        let mut categories = BTreeSet::new();
        for id in &needed.items {
            let item = full.items.item(*id)?;
            categories.insert(item.category);
            result.items.items.push(item.clone());
        }
        for id in categories {
            result
                .items
                .categories
                .push(full.items.category(id)?.clone());
        }
        result.validate()?;
        Ok(result)
    }
}
