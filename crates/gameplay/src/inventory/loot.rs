//! What an inventory holds the first time it is opened. NPCs and containers start as a
//! reference to a table; their inventory is made when something first needs it, so the
//! world does not carry thousands of inventories nobody has looked into.
use super::{Inventory, ItemCatalog, ItemDefinitionId, types::*};
use crate::rules::RandomState;
use game_types::LootId;
use serde::{Deserialize, Serialize};

fn always() -> u8 {
    100
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LootEntry {
    pub item: ItemDefinitionId,
    /// At least and at most this many.
    pub quantity: (u32, u32),
    /// Out of a hundred times, how often it is there at all.
    #[serde(default = "always")]
    pub chance: u8,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LootTable {
    pub id: LootId,
    pub entries: Vec<LootEntry>,
}
impl LootTable {
    pub fn validate(&self, catalog: &ItemCatalog) -> Result<()> {
        for entry in &self.entries {
            catalog.item(entry.item)?;
            let (least, most) = entry.quantity;
            if least == 0 || least > most || most > 10_000 || !(1..=100).contains(&entry.chance) {
                return Err(InventoryError::Invalid(format!(
                    "loot {}: bad quantity or chance for {}",
                    self.id, entry.item
                )));
            }
        }
        Ok(())
    }
    /// Puts what the table yields into an inventory, drawing from `random`.
    pub fn roll(
        &self,
        catalog: &ItemCatalog,
        random: &mut RandomState,
        inventory: &mut Inventory,
    ) -> Result<()> {
        for entry in &self.entries {
            if entry.chance < 100 && random.die(100)? > u32::from(entry.chance) {
                continue;
            }
            let (least, most) = entry.quantity;
            let quantity = least + random.below(most - least + 1)?;
            inventory.grant(catalog, entry.item, quantity)?;
        }
        Ok(())
    }
}
