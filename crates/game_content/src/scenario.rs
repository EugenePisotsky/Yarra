use crate::{Result, Step, contextual};
use game_types::*;
use gameplay::actors::Position;
use gameplay::inventory;
use gameplay::inventory::{Inventory, Money, Wallet};
use gameplay::{GameContent, GameSession, SessionState, ToolContent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemAmount {
    pub definition: ItemDefinitionId,
    pub quantity: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorSpawn {
    pub id: ActorId,
    pub template: ActorTemplateId,
    pub position: Position,
    pub name: Option<TextRef>,
    /// Resources that do not start full, e.g. `{"health": 50}`.
    #[serde(default, deserialize_with = "game_types::deserialize_unique_map")]
    pub resources: std::collections::BTreeMap<Key, i32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventorySeed {
    pub id: InventoryId,
    pub owner: OwnerRef,
    pub role: inventory::InventoryRole,
    pub items: Vec<ItemAmount>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletSeed {
    pub id: WalletId,
    pub owner: OwnerRef,
    pub balance: Money,
}
/// Authored starting conditions and the steps played from them; not a saved playthrough or
/// a second actor model. Characters are built from their templates; the playthrough ID is
/// fresh each start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Another scenario, relative to the project, whose starting conditions this one plays
    /// from. A scenario with a base has only steps of its own; the base has only a start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default)]
    pub seed: u64,
    #[serde(default)]
    pub actors: Vec<ActorSpawn>,
    #[serde(default)]
    pub owners: Vec<OwnerRef>,
    #[serde(default)]
    pub inventories: Vec<InventorySeed>,
    #[serde(default)]
    pub wallets: Vec<WalletSeed>,
    /// Variables that start away from their initial value.
    #[serde(default)]
    pub variables: std::collections::BTreeMap<VariableId, gameplay::Value>,
    /// Characters travelling together at the start.
    #[serde(default)]
    pub party: BTreeSet<ActorId>,
    /// The party member the player steers.
    #[serde(default)]
    pub controlled: Option<ActorId>,
    /// The wallet holding the party's shared gold.
    #[serde(default)]
    pub purse: Option<WalletId>,
    #[serde(default)]
    pub steps: Vec<Step>,
}
impl Scenario {
    /// This scenario's steps played from `base`'s starting conditions.
    pub fn starting_from(self, base: Scenario) -> Result<Self> {
        let start_of_its_own = self.seed != 0
            || !self.actors.is_empty()
            || !self.owners.is_empty()
            || !self.inventories.is_empty()
            || !self.wallets.is_empty()
            || !self.variables.is_empty()
            || !self.party.is_empty()
            || self.controlled.is_some()
            || self.purse.is_some();
        require(
            !start_of_its_own,
            "a scenario with a base takes its whole start from it",
        )?;
        require(
            base.base.is_none() && base.steps.is_empty(),
            "a base scenario is a start only: no base or steps of its own",
        )?;
        Ok(Self {
            base: None,
            steps: self.steps,
            ..base
        })
    }
    pub fn instantiate(&self, content: &GameContent) -> Result<gameplay::GameSession> {
        content.validate()?;
        let mut state = SessionState::empty(self.seed);
        state.owners = self.owners.iter().cloned().collect();
        state.variables = self
            .variables
            .iter()
            .map(|(variable, value)| {
                let key = gameplay::VariableKey {
                    variable: *variable,
                    actor: None,
                };
                (key, value.clone())
            })
            .collect();
        state.party = gameplay::Party {
            members: self.party.clone(),
            controlled: self.controlled,
            experience: 0,
            wallet: self.purse,
        };
        for spawn in &self.actors {
            content.template(spawn.template).map_err(|_| {
                Invalid(format!(
                    "actor {}: unknown template {}",
                    spawn.id, spawn.template
                ))
            })?;
            let actor = contextual(
                format!("actor {}", spawn.id),
                state.spawn(content, spawn.template, spawn.id),
            )?;
            actor.position = spawn.position.clone();
            actor.name_override = spawn.name.clone();
            for (resource, amount) in &spawn.resources {
                require(
                    actor.resources.contains_key(resource),
                    &format!(
                        "actor {}: {} is not a resource",
                        spawn.id,
                        resource.as_str()
                    ),
                )?;
                actor.resources.insert(resource.clone(), *amount);
            }
        }
        for seed in &self.inventories {
            require(
                seed.items.len() <= inventory::MAX_INVENTORY_ENTRIES,
                "too many starting inventory lines",
            )?;
            // A character's own inventory starts from what its template wears and carries;
            // the scenario adds to it.
            let carried = seed.owner.as_actor();
            let carried = carried.filter(|_| seed.role == inventory::InventoryRole::Carried);
            let mut inventory = if let Some(id) = carried {
                let (inventory, equipment) = state.unopened_inventory(content, state.actor(id)?)?;
                if let Some(actor) = state.actors.get_mut(&id) {
                    actor.equipment = equipment;
                }
                inventory
            } else {
                Inventory::new(seed.owner, seed.role)
            };
            inventory.id = seed.id;
            for item in &seed.items {
                contextual(
                    format!("inventory {} / item {}", seed.id, item.definition),
                    inventory.grant(&content.items, item.definition, item.quantity),
                )?;
            }
            state.add_inventory(inventory);
        }
        for seed in &self.wallets {
            let mut wallet = Wallet::new(seed.owner);
            wallet.id = seed.id;
            wallet.credit(seed.balance)?;
            state.add_wallet(wallet);
        }
        contextual(
            "scenario starting state",
            GameSession::new(ToolContent::new(content.clone())?, state),
        )
    }
    /// Run in a new isolated session. Failure never mutates an existing playthrough.
    pub fn run(&self, content: &GameContent) -> Result<gameplay::GameSession> {
        let mut session = self.instantiate(content)?;
        for (index, step) in self.steps.iter().enumerate() {
            contextual(
                format!("scenario step {} ({})", index + 1, step.kind()),
                step.apply(&mut session),
            )?;
        }
        Ok(session)
    }
    pub fn text_keys(&self) -> impl Iterator<Item = &MessageRef> {
        self.actors.iter().filter_map(|a| match &a.name {
            Some(TextRef::Message(key)) => Some(key),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scenario(text: &str) -> Scenario {
        ron::from_str(text).unwrap()
    }
    #[test]
    fn a_scenario_with_a_base_takes_its_start_and_keeps_its_own_steps() {
        let base = scenario(r#"(seed: 7, party: ["hero"])"#);
        let steps = r#"steps: [ExpectLevel(actor: "hero", level: 1)]"#;
        let played = scenario(&format!(r#"(base: Some("start.ron"), {steps})"#));
        let played = played.starting_from(base.clone()).unwrap();
        assert_eq!((played.seed, played.steps.len()), (7, 1));
        assert!(played.base.is_none() && played.party.len() == 1);
        // A start of its own as well would be ambiguous, and a base plays no steps.
        let both = scenario(&format!(r#"(base: Some("start.ron"), seed: 1, {steps})"#));
        assert!(both.starting_from(base).is_err());
        let with_steps = scenario(&format!("({steps})"));
        let bare = scenario(r#"(base: Some("start.ron"))"#);
        assert!(bare.starting_from(with_steps).is_err());
    }
}
