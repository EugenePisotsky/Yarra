use super::{CatalogId, ItemDefinitionId, Money, types::*};
use crate::rules::ItemMechanics;
use game_types::TextRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_CATALOG_ITEMS: usize = 10_000;
pub const MAX_CATEGORIES: usize = 256;
pub const MAX_STACK_LIMIT: u32 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// Identity survives key and display-name changes.
    pub id: CategoryId,
    pub key: String,
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
    pub key: String,
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
        validate_key(&self.key)?;
        self.name.validate()?;
        self.description.validate()?;
        self.mechanics.validate()?;
        if self.mechanics.slot.is_some() && self.stack_limit != 1 {
            return Err(InventoryError::Invalid(
                "equipment must be individually identified".into(),
            ));
        }
        if self.tags.len() > 64 {
            return Err(InventoryError::Invalid("oversized item definition".into()));
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
            id: CatalogId::new(),
            revision: 1,
            categories: Vec::new(),
            items: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        validate_revision(self.revision)?;
        if self.categories.len() > MAX_CATEGORIES || self.items.len() > MAX_CATALOG_ITEMS {
            return Err(InventoryError::Invalid("catalog exceeds limits".into()));
        }
        let mut categories = BTreeSet::new();
        let mut category_keys = BTreeSet::new();
        for category in &self.categories {
            validate_key(&category.key)?;
            category.name.validate()?;
            if !categories.insert(category.id) || !category_keys.insert(&category.key) {
                return Err(InventoryError::Duplicate);
            }
        }
        let mut ids = BTreeSet::new();
        let mut keys = BTreeSet::new();
        for item in &self.items {
            item.validate()?;
            if !ids.insert(item.id) || !keys.insert(&item.key) {
                return Err(InventoryError::Duplicate);
            }
            if !categories.contains(&item.category) {
                return Err(InventoryError::UnknownCategory(item.category));
            }
        }
        Ok(())
    }
    pub fn item(&self, id: ItemDefinitionId) -> Result<&ItemDefinition> {
        self.items
            .iter()
            .find(|item| item.id == id)
            .ok_or(InventoryError::UnknownDefinition(id))
    }
    pub fn item_by_key(&self, key: &str) -> Option<&ItemDefinition> {
        self.items.iter().find(|item| item.key == key)
    }
    pub fn category(&self, id: CategoryId) -> Result<&Category> {
        self.categories
            .iter()
            .find(|category| category.id == id)
            .ok_or(InventoryError::UnknownCategory(id))
    }
    pub fn category_by_key(&self, key: &str) -> Option<&Category> {
        self.categories.iter().find(|category| category.key == key)
    }
    /// Add or replace by stable ID. Renaming a key/title preserves item references.
    pub fn put_category(&mut self, category: Category) -> Result<()> {
        let mut next = self.clone();
        if let Some(existing) = next.categories.iter_mut().find(|c| c.id == category.id) {
            *existing = category;
        } else {
            next.categories.push(category);
        }
        self.replace(next)
    }
    /// Add or update by stable ID. Keys remain unique across definitions.
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
        next.revision = next_revision(self.revision)?;
        next.validate()?;
        *self = next;
        Ok(())
    }
}
