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
    #[serde(default, deserialize_with = "game_types::deserialize_key_map")]
    pub resources: std::collections::BTreeMap<Key, i32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventorySeed {
    pub id: InventoryId,
    pub owner: OwnerRef,
    pub role: String,
    pub items: Vec<ItemAmount>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletSeed {
    pub id: WalletId,
    pub owner: OwnerRef,
    pub balance: Money,
}
/// Authored starting conditions, not a saved playthrough or a second actor model.
/// Characters are built from their templates; entry/playthrough IDs are fresh each start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub seed: u64,
    pub actors: Vec<ActorSpawn>,
    pub owners: Vec<OwnerRef>,
    pub inventories: Vec<InventorySeed>,
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
    pub steps: Vec<Step>,
}
impl Scenario {
    pub fn instantiate(&self, content: &GameContent) -> Result<crate::ToolSession> {
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
            let mut inventory = if seed.owner.kind == "actor" && seed.role == gameplay::CARRIED {
                let id = ActorId(seed.owner.id.0);
                let (inventory, equipment) = state.unopened_inventory(content, state.actor(id)?)?;
                if let Some(actor) = state.actors.get_mut(&id) {
                    actor.equipment = equipment;
                }
                inventory
            } else {
                Inventory::new(seed.owner.clone(), &seed.role)?
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
            let mut wallet = Wallet::new(seed.owner.clone())?;
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
    pub fn run(&self, content: &GameContent) -> Result<crate::ToolSession> {
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
