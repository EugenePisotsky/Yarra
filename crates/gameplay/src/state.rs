use crate::{GameContent, Result};
use actors::Actor;
use dialogue::Conversation;
use game_types::*;
use inventory::{Inventory, Wallet};
use rules::{Attributes, RandomState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Resolved operation state, or an explicitly eager authoring seed/export.
/// Runtime sessions keep only a bounded working set; storage owns nonresident records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionState {
    pub world: crate::WorldState,
    pub histories: Vec<dialogue::History>,
    pub claims: Vec<dialogue::Claim>,
    pub narrative_random: RandomState,
    pub quests: Vec<quests::Progress>,
    pub relationships: Vec<actors::Relationship>,
    pub interactions: Vec<dialogue::Interaction>,
    pub playthrough: PlaythroughId,
    pub generation: u64,
    pub time: GameTime,
    pub random: RandomState,
    pub actors: Vec<Actor>,
    pub owners: Vec<OwnerRef>,
    pub inventories: Vec<Inventory>,
    pub wallets: Vec<Wallet>,
    pub conversations: Vec<Conversation>,
    pub facts: BTreeSet<Key>,
}
impl SessionState {
    pub fn empty(seed: u64) -> Self {
        Self {
            world: Default::default(),
            playthrough: PlaythroughId::new(),
            generation: 1,
            time: GameTime::default(),
            random: RandomState(seed),
            narrative_random: RandomState(seed ^ 0xd1b54a32d192ed03),
            histories: vec![],
            claims: vec![],
            quests: vec![],
            relationships: vec![],
            interactions: vec![],
            actors: Vec::new(),
            owners: Vec::new(),
            inventories: Vec::new(),
            wallets: Vec::new(),
            conversations: Vec::new(),
            facts: BTreeSet::new(),
        }
    }
    pub fn history(&self, key: dialogue::HistoryKey) -> Result<&dialogue::History> {
        self.histories
            .iter()
            .find(|v| v.key == key)
            .ok_or_else(|| Invalid("history not resolved".into()).into())
    }
    pub fn claim(&self, key: dialogue::ClaimKey) -> Result<&dialogue::Claim> {
        self.claims
            .iter()
            .find(|v| v.key == key)
            .ok_or_else(|| Invalid("claim not resolved".into()).into())
    }
    pub fn quest(&self, id: QuestId) -> Result<&quests::Progress> {
        self.quests
            .iter()
            .find(|q| q.quest == id)
            .ok_or_else(|| Invalid("quest state not resolved".into()).into())
    }
    pub fn relationship(&self, key: actors::RelationshipKey) -> Result<&actors::Relationship> {
        self.relationships
            .iter()
            .find(|r| r.key == key)
            .ok_or_else(|| Invalid("relationship state not resolved".into()).into())
    }
    pub fn interaction(&self, key: dialogue::InteractionKey) -> Result<&dialogue::Interaction> {
        self.interactions
            .iter()
            .find(|r| r.key == key)
            .ok_or_else(|| Invalid("interaction state not resolved".into()).into())
    }
    pub fn actor(&self, id: ActorId) -> Result<&Actor> {
        self.actors
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Invalid("unknown actor".into()).into())
    }
    pub fn inventory(&self, id: InventoryId) -> Result<&Inventory> {
        self.inventories
            .iter()
            .find(|i| i.id == id)
            .ok_or_else(|| Invalid("unknown inventory".into()).into())
    }
    pub fn wallet(&self, id: WalletId) -> Result<&Wallet> {
        self.wallets
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| Invalid("unknown wallet".into()).into())
    }
    pub fn carried(&self, actor: ActorId) -> Result<&Inventory> {
        self.inventories
            .iter()
            .find(|i| i.owner == OwnerRef::actor(actor) && i.role == "carried")
            .ok_or_else(|| Invalid("actor has no carried inventory".into()).into())
    }
    pub fn derived(&self, content: &GameContent, actor: ActorId) -> Result<Attributes> {
        let actor = self.actor(actor)?;
        let carried = self.carried(actor.id)?;
        content.game.rules.validate()?;
        let mut modifiers = Vec::new();
        let mut ids = BTreeSet::new();
        for (slot, item) in &actor.equipment {
            require(ids.insert(*item), "item equipped more than once")?;
            let definition = content.items.item(carried.entry(*item)?.definition)?;
            require(
                definition.mechanics.slot.as_ref() == Some(slot),
                "incompatible equipment slot",
            )?;
            modifiers.extend(&definition.mechanics.modifiers);
        }
        modifiers.extend(
            actor
                .effects
                .iter()
                .filter(|e| e.expires_at > self.time)
                .map(|e| &e.modifier),
        );
        Ok(content.game.rules.derive(&actor.base, modifiers)?)
    }
    pub fn validate(&self, content: &GameContent) -> Result<()> {
        content.validate()?;
        self.world.validate(content, self)?;
        require(
            self.generation > 0 && self.generation <= i64::MAX as u64,
            "invalid session generation",
        )?;
        require(
            self.actors.len() <= 10000
                && self.owners.len() <= 10000
                && self.inventories.len() <= 20000
                && self.wallets.len() <= 20000
                && self.conversations.len() <= 10000,
            "session exceeds limits",
        )?;
        let mut actors = BTreeSet::new();
        for actor in &self.actors {
            require(actors.insert(actor.id), "duplicate actor")?;
            require(
                content.game.actors.iter().any(|t| t.id == actor.template),
                "unknown actor template",
            )?;
            actor.validate(&content.game.rules)?;
        }
        let mut owners = BTreeSet::new();
        for owner in &self.owners {
            owner.validate()?;
            require(
                owner.kind != "actor" && owners.insert((owner.kind.clone(), owner.id)),
                "invalid/duplicate external owner",
            )?;
        }
        let valid_owner = |owner: &OwnerRef| {
            if owner.kind == "actor" {
                actors.contains(&ActorId(owner.id.0))
            } else {
                owners.contains(&(owner.kind.clone(), owner.id))
            }
        };
        let mut inventories = BTreeSet::new();
        let mut roles = BTreeSet::new();
        let mut entries = BTreeSet::new();
        for inv in &self.inventories {
            inv.validate(&content.items)?;
            require(
                inventories.insert(inv.id)
                    && roles.insert((&inv.owner.kind, inv.owner.id, &inv.role))
                    && valid_owner(&inv.owner),
                "invalid inventory identity/owner/role",
            )?;
            for item in &inv.entries {
                require(entries.insert(item.id), "duplicate global item identity")?;
            }
        }
        let mut wallets = BTreeSet::new();
        for wallet in &self.wallets {
            wallet.validate()?;
            require(
                wallets.insert(wallet.id) && valid_owner(&wallet.owner),
                "invalid wallet identity/owner",
            )?;
        }
        for actor in &self.actors {
            let stats = self.derived(content, actor.id)?;
            require(
                actor.health <= stats[&content.game.rules.health_attribute] as u32,
                "health exceeds derived maximum",
            )?;
        }
        require(
            self.quests.len() <= 10000
                && self.relationships.len() <= 10000
                && self.interactions.len() <= 10000,
            "narrative state exceeds limits",
        )?;
        require(
            self.histories.len() <= 10000 && self.claims.len() <= 10000,
            "history/claim state budget exceeded",
        )?;
        let mut histories = BTreeSet::new();
        let mut claims = BTreeSet::new();
        for h in &self.histories {
            require(histories.insert(h.key), "duplicate history")?;
            h.validate(content.dialogue_contract(h.key.dialogue)?, self.time)?;
            require(
                h.key.scope.actors().iter().all(|id| actors.contains(id)),
                "invalid history actor",
            )?;
        }
        for c in &self.claims {
            c.key.scope.validate()?;
            require(
                claims.insert(c.key) && content.claim(c.key.claim)?.scope.accepts(c.key.scope),
                "invalid claim identity/scope",
            )?;
            require(
                c.key.scope.actors().iter().all(|id| actors.contains(id)),
                "invalid claim actor",
            )?;
        }
        let mut quests = BTreeSet::new();
        let mut relationships = BTreeSet::new();
        let mut interactions = BTreeSet::new();
        for q in &self.quests {
            require(quests.insert(q.quest), "duplicate quest state")?;
            q.validate(content.quest(q.quest)?)?;
        }
        for r in &self.relationships {
            r.validate()?;
            require(
                relationships.insert(r.key)
                    && actors.contains(&r.key.from)
                    && actors.contains(&r.key.to),
                "invalid relationship actors/identity",
            )?;
        }
        for i in &self.interactions {
            require(
                interactions.insert(i.key)
                    && i.key.participant != i.key.speaker
                    && actors.contains(&i.key.participant)
                    && actors.contains(&i.key.speaker),
                "invalid interaction actors/identity",
            )?;
            if let Some(selection) = &i.current {
                let profile = content.profile(selection.profile)?;
                require(
                    profile.rules.iter().any(|r| {
                        r.id == selection.rule
                            && r.variants.iter().any(|v| v.dialogue == selection.dialogue)
                    }),
                    "invalid saved interaction selection",
                )?;
                require(
                    self.conversations.iter().any(|c| {
                        c.dialogue == selection.dialogue
                            && c.participant == i.key.participant
                            && c.speaker == i.key.speaker
                    }),
                    "selected conversation is not resolved",
                )?;
            }
        }
        let mut conversations = BTreeSet::new();
        for state in &self.conversations {
            require(
                conversations.insert((state.dialogue, state.participant, state.speaker))
                    && actors.contains(&state.participant)
                    && actors.contains(&state.speaker),
                "invalid conversation participants/identity",
            )?;
            state.validate(content.dialogue(state.dialogue)?)?;
            require(
                state.bindings.values().all(|id| actors.contains(id)),
                "unknown bound actor",
            )?;
        }
        require(
            self.facts.is_subset(&content.game.facts),
            "unknown saved fact",
        )?;
        Ok(())
    }
    pub(crate) fn actor_mut(&mut self, id: ActorId) -> Result<&mut Actor> {
        self.actors
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| Invalid("unknown actor".into()).into())
    }
    pub(crate) fn inventory_mut(&mut self, id: InventoryId) -> Result<&mut Inventory> {
        self.inventories
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| Invalid("unknown inventory".into()).into())
    }
    pub(crate) fn clamp_health(&mut self, content: &GameContent) -> Result<()> {
        for index in 0..self.actors.len() {
            let max = self.derived(content, self.actors[index].id)?
                [&content.game.rules.health_attribute] as u32;
            self.actors[index].health = self.actors[index].health.min(max);
        }
        Ok(())
    }
}
