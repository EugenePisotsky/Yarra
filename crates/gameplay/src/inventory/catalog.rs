use super::{CatalogId, ItemDefinitionId, Money, types::*};
use crate::rules::ItemMechanics;
use game_types::TextRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_STACK_LIMIT: u32 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// Identity survives display-name changes.
    pub id: CategoryId,
    pub name: TextRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemPermissions {
    pub transferable: bool,
    pub sellable: bool,
    pub discardable: bool,
}
impl Default for ItemPermissions {
    fn default() -> Self {
        Self {
            transferable: true,
            sellable: true,
            discardable: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemDefinition {
    pub id: ItemDefinitionId,
    pub name: TextRef,
    pub description: TextRef,
    pub category: CategoryId,
    pub tags: BTreeSet<String>,
    pub icon: Option<String>,
    pub weight_grams: u32,
    /// One means individually identified, non-stackable items.
    pub stack_limit: u32,
    /// Customer pays this amount. None means the merchant cannot sell it.
    pub merchant_sell_price: Option<Money>,
    /// Merchant pays this amount. None means the merchant refuses it; zero is valid.
    pub merchant_buy_price: Option<Money>,
    pub permissions: ItemPermissions,
    pub mechanics: ItemMechanics,
}
impl ItemDefinition {
    pub fn validate(&self) -> Result<()> {
        self.name.validate()?;
        self.description.validate()?;
        self.mechanics.validate()?;
        if self.mechanics.slot.is_some() && self.stack_limit != 1 {
            return Err(InventoryError::Invalid(
                "equipment must be individually identified".into(),
            ));
        }
        for tag in &self.tags {
            validate_key(tag)?;
        }
        if let Some(icon) = &self.icon {
            validate_text(icon, 1024)?;
        }
        if self.stack_limit == 0 || self.stack_limit > MAX_STACK_LIMIT {
            return Err(InventoryError::InvalidQuantity);
        }
        Ok(())
    }
}

/// An authored snapshot. Identity survives edits; revision changes on every edit.
/// Runtime operation subsets preserve this identity/revision; checkpoints reference immutable content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemCatalog {
    pub id: CatalogId,
    pub revision: u64,
    pub categories: Vec<Category>,
    pub items: Vec<ItemDefinition>,
}
impl Default for ItemCatalog {
    fn default() -> Self {
        Self::new()
    }
}
impl ItemCatalog {
    pub fn new() -> Self {
        Self {
            id: CatalogId::random(),
            revision: 1,
            categories: Vec::new(),
            items: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        validate_revision(self.revision)?;
        // Both lists are kept in order of identity, which also shows each is there once,
        // so a definition is found by bisection.
        let ordered = self.categories.windows(2).all(|w| w[0].id < w[1].id)
            && self.items.windows(2).all(|w| w[0].id < w[1].id);
        if !ordered {
            return Err(InventoryError::Duplicate);
        }
        for category in &self.categories {
            category.name.validate()?;
        }
        for item in &self.items {
            item.validate()?;
            self.category(item.category)?;
        }
        Ok(())
    }
    /// Puts the definitions in the order lookups rely on.
    pub fn sort(&mut self) {
        self.categories.sort_by_key(|v| v.id);
        self.items.sort_by_key(|v| v.id);
    }
    pub fn item(&self, id: ItemDefinitionId) -> Result<&ItemDefinition> {
        self.items
            .binary_search_by_key(&id, |item| item.id)
            .map(|index| &self.items[index])
            .map_err(|_| InventoryError::UnknownDefinition(id))
    }
    pub fn category(&self, id: CategoryId) -> Result<&Category> {
        self.categories
            .binary_search_by_key(&id, |category| category.id)
            .map(|index| &self.categories[index])
            .map_err(|_| InventoryError::UnknownCategory(id))
    }
    /// Add or replace by identity. Changing the title preserves item references.
    pub fn put_category(&mut self, category: Category) -> Result<()> {
        let mut next = self.clone();
        if let Some(existing) = next.categories.iter_mut().find(|c| c.id == category.id) {
            *existing = category;
        } else {
            next.categories.push(category);
        }
        self.replace(next)
    }
    /// Add or update by identity.
    pub fn put_item(&mut self, item: ItemDefinition) -> Result<()> {
        let mut next = self.clone();
        if let Some(existing) = next.items.iter_mut().find(|i| i.id == item.id) {
            *existing = item;
        } else {
            next.items.push(item);
        }
        self.replace(next)
    }
    pub fn remove_item(&mut self, id: ItemDefinitionId) -> Result<()> {
        self.item(id)?;
        let mut next = self.clone();
        next.items.retain(|i| i.id != id);
        self.replace(next)
    }
    pub fn remove_category(&mut self, id: CategoryId) -> Result<()> {
        self.category(id)?;
        if self.items.iter().any(|item| item.category == id) {
            return Err(InventoryError::CategoryInUse(id));
        }
        let mut next = self.clone();
        next.categories.retain(|c| c.id != id);
        self.replace(next)
    }
    fn replace(&mut self, mut next: Self) -> Result<()> {
        next.sort();
        next.revision = next_revision(self.revision)?;
        next.validate()?;
        *self = next;
        Ok(())
    }
}
