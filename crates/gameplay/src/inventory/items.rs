use super::{ItemCatalog, types::*};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_INVENTORY_ENTRIES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemEntry {
    pub id: ItemId,
    pub definition: ItemDefinitionId,
    pub quantity: u32,
}

/// What an inventory is to its owner. An owner has at most one of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum InventoryRole {
    /// What a character carries, what it wears included.
    Carried,
    /// What a container holds.
    Contents,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub id: InventoryId,
    pub owner: OwnerRef,
    pub role: InventoryRole,
    pub revision: u64,
    pub entries: Vec<ItemEntry>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ItemFilter<'a> {
    pub category: Option<CategoryId>,
    pub tag: Option<&'a str>,
}

impl Inventory {
    pub fn new(owner: OwnerRef, role: InventoryRole) -> Self {
        Self {
            id: InventoryId::new(),
            owner,
            role,
            revision: 1,
            entries: Vec::new(),
        }
    }
    /// The character this is the carried inventory of, if it is one.
    pub fn carried_by(&self) -> Option<ActorId> {
        self.owner
            .as_actor()
            .filter(|_| self.role == InventoryRole::Carried)
    }
    /// Checks the inventory against a catalog that was itself checked when it was loaded.
    pub fn validate(&self, catalog: &ItemCatalog) -> Result<()> {
        validate_revision(self.revision)?;
        if self.entries.len() > MAX_INVENTORY_ENTRIES {
            return Err(InventoryError::Capacity);
        }
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            if !ids.insert(entry.id) {
                return Err(InventoryError::Duplicate);
            }
            let definition = catalog.item(entry.definition)?;
            if entry.quantity == 0 || entry.quantity > definition.stack_limit {
                return Err(InventoryError::InvalidQuantity);
            }
        }
        Ok(())
    }
    pub fn entry(&self, id: ItemId) -> Result<&ItemEntry> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .ok_or(InventoryError::UnknownItem(id))
    }
    pub fn list_items<'a>(
        &'a self,
        catalog: &ItemCatalog,
        filter: ItemFilter<'_>,
    ) -> Result<Vec<&'a ItemEntry>> {
        self.validate(catalog)?;
        let mut result = Vec::new();
        for entry in &self.entries {
            let item = catalog.item(entry.definition)?;
            if filter.category.is_none_or(|c| item.category == c)
                && filter.tag.is_none_or(|t| item.tags.contains(t))
            {
                result.push(entry);
            }
        }
        Ok(result)
    }
    pub fn total_weight_grams(&self, catalog: &ItemCatalog) -> Result<u64> {
        self.validate(catalog)?;
        self.entries.iter().try_fold(0_u64, |total, entry| {
            let weight =
                u64::from(catalog.item(entry.definition)?.weight_grams) * u64::from(entry.quantity);
            total.checked_add(weight).ok_or(InventoryError::Overflow)
        })
    }
    /// Authoritative creation (loot generation, rewards, fixtures). Not a transfer.
    pub fn grant(
        &mut self,
        catalog: &ItemCatalog,
        definition: ItemDefinitionId,
        quantity: u32,
    ) -> Result<Vec<ItemId>> {
        self.validate(catalog)?;
        if quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }
        catalog.item(definition)?;
        let mut next = self.clone();
        let ids = deposit(
            catalog,
            &mut next,
            ItemEntry {
                id: ItemId::new(),
                definition,
                quantity,
            },
        )?;
        next.revision = next_revision(self.revision)?;
        *self = next;
        Ok(ids)
    }
    /// Authoritative consumption/destruction. Gameplay must authorize the reason.
    /// Use discard for user-requested destruction, which respects discardable.
    pub fn remove(&mut self, catalog: &ItemCatalog, entry: ItemId, quantity: u32) -> Result<()> {
        self.validate(catalog)?;
        let mut next = self.clone();
        withdraw(&mut next, entry, quantity)?;
        next.revision = next_revision(self.revision)?;
        *self = next;
        Ok(())
    }
    pub fn discard(&mut self, catalog: &ItemCatalog, entry: ItemId, quantity: u32) -> Result<()> {
        if !catalog
            .item(self.entry(entry)?.definition)?
            .permissions
            .discardable
        {
            return Err(InventoryError::Restricted);
        }
        self.remove(catalog, entry, quantity)
    }
    /// Splitting gives the new stack a new ID; the remaining stack keeps its ID.
    pub fn split_stack(
        &mut self,
        catalog: &ItemCatalog,
        entry: ItemId,
        quantity: u32,
    ) -> Result<ItemId> {
        self.validate(catalog)?;
        if quantity == 0 || quantity >= self.entry(entry)?.quantity {
            return Err(InventoryError::InvalidQuantity);
        }
        if self.entries.len() == MAX_INVENTORY_ENTRIES {
            return Err(InventoryError::Capacity);
        }
        let mut next = self.clone();
        let split = withdraw(&mut next, entry, quantity)?;
        let id = split.id;
        next.entries.push(split);
        next.revision = next_revision(self.revision)?;
        *self = next;
        Ok(id)
    }
    /// Merge the whole source stack into the destination; source ID is retired.
    pub fn merge_stacks(
        &mut self,
        catalog: &ItemCatalog,
        source: ItemId,
        destination: ItemId,
    ) -> Result<()> {
        self.validate(catalog)?;
        if source == destination {
            return Err(InventoryError::SameParticipant);
        }
        let from = self.entry(source)?;
        let to = self.entry(destination)?;
        let limit = catalog.item(from.definition)?.stack_limit;
        if from.definition != to.definition || limit == 1 {
            return Err(InventoryError::IncompatibleStack);
        }
        let quantity = from
            .quantity
            .checked_add(to.quantity)
            .ok_or(InventoryError::Overflow)?;
        if quantity > limit {
            return Err(InventoryError::InvalidQuantity);
        }
        let revision = next_revision(self.revision)?;
        self.entries.retain(|entry| entry.id != source);
        self.entries
            .iter_mut()
            .find(|entry| entry.id == destination)
            .unwrap()
            .quantity = quantity;
        self.revision = revision;
        Ok(())
    }
}

/// Atomic in memory. Persistence must write both inventories in one transaction.
/// Compatible stacks merge automatically; returned IDs identify destination entries.
/// An unmerged full entry keeps its ID (including every non-stackable item).
pub fn transfer(
    catalog: &ItemCatalog,
    source: &mut Inventory,
    destination: &mut Inventory,
    entry: ItemId,
    quantity: u32,
) -> Result<Vec<ItemId>> {
    validate_pair(catalog, source, destination)?;
    if !catalog
        .item(source.entry(entry)?.definition)?
        .permissions
        .transferable
    {
        return Err(InventoryError::Restricted);
    }
    let mut from = source.clone();
    let mut to = destination.clone();
    let moved = withdraw(&mut from, entry, quantity)?;
    let ids = deposit(catalog, &mut to, moved)?;
    from.revision = next_revision(source.revision)?;
    to.revision = next_revision(destination.revision)?;
    *source = from;
    *destination = to;
    Ok(ids)
}

pub(crate) fn validate_pair(catalog: &ItemCatalog, a: &Inventory, b: &Inventory) -> Result<()> {
    if a.id == b.id {
        return Err(InventoryError::SameParticipant);
    }
    a.validate(catalog)?;
    b.validate(catalog)?;
    let ids: BTreeSet<_> = a.entries.iter().map(|e| e.id).collect();
    if b.entries.iter().any(|e| ids.contains(&e.id)) {
        return Err(InventoryError::Duplicate);
    }
    Ok(())
}

pub(crate) fn withdraw(inventory: &mut Inventory, id: ItemId, quantity: u32) -> Result<ItemEntry> {
    let index = inventory
        .entries
        .iter()
        .position(|e| e.id == id)
        .ok_or(InventoryError::UnknownItem(id))?;
    let entry = &mut inventory.entries[index];
    if quantity == 0 || quantity > entry.quantity {
        return Err(InventoryError::InvalidQuantity);
    }
    if quantity == entry.quantity {
        return Ok(inventory.entries.remove(index));
    }
    entry.quantity -= quantity;
    Ok(ItemEntry {
        id: ItemId::new(),
        definition: entry.definition,
        quantity,
    })
}

pub(crate) fn deposit(
    catalog: &ItemCatalog,
    inventory: &mut Inventory,
    mut incoming: ItemEntry,
) -> Result<Vec<ItemId>> {
    let limit = catalog.item(incoming.definition)?.stack_limit;
    if inventory.entries.iter().any(|e| e.id == incoming.id) {
        return Err(InventoryError::Duplicate);
    }
    let mut ids = Vec::new();
    if limit > 1 {
        for existing in &mut inventory.entries {
            if existing.definition != incoming.definition || existing.quantity == limit {
                continue;
            }
            let amount = incoming.quantity.min(limit - existing.quantity);
            existing.quantity += amount;
            incoming.quantity -= amount;
            ids.push(existing.id);
            if incoming.quantity == 0 {
                return Ok(ids);
            }
        }
    }
    let needed = u64::from(incoming.quantity).div_ceil(u64::from(limit));
    if needed > (MAX_INVENTORY_ENTRIES - inventory.entries.len()) as u64 {
        return Err(InventoryError::Capacity);
    }
    while incoming.quantity > 0 {
        let quantity = incoming.quantity.min(limit);
        ids.push(incoming.id);
        inventory.entries.push(ItemEntry {
            quantity,
            ..incoming.clone()
        });
        incoming.quantity -= quantity;
        incoming.id = ItemId::new();
    }
    Ok(ids)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wallet {
    pub id: WalletId,
    pub owner: OwnerRef,
    pub revision: u64,
    pub balance: Money,
}
impl Wallet {
    pub fn new(owner: OwnerRef) -> Self {
        Self {
            id: WalletId::new(),
            owner,
            revision: 1,
            balance: Money::ZERO,
        }
    }
    pub fn validate(&self) -> Result<()> {
        validate_revision(self.revision)
    }
    /// Authoritative currency creation, such as a quest reward.
    pub fn credit(&mut self, amount: Money) -> Result<()> {
        self.validate()?;
        let balance = self.balance.checked_add(amount)?;
        let revision = next_revision(self.revision)?;
        self.balance = balance;
        self.revision = revision;
        Ok(())
    }
    /// Authoritative currency removal, such as paying for a service.
    pub fn debit(&mut self, amount: Money) -> Result<()> {
        self.validate()?;
        let balance = self.balance.checked_sub(amount)?;
        let revision = next_revision(self.revision)?;
        self.balance = balance;
        self.revision = revision;
        Ok(())
    }
}

pub fn transfer_money(source: &mut Wallet, destination: &mut Wallet, amount: Money) -> Result<()> {
    if source.id == destination.id {
        return Err(InventoryError::SameParticipant);
    }
    let mut from = source.clone();
    let mut to = destination.clone();
    from.debit(amount)?;
    to.credit(amount)?;
    *source = from;
    *destination = to;
    Ok(())
}
