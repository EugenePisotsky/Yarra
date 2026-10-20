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
    /// Entries this inventory has created, which numbers the next one; see `mint`.
    pub minted: u64,
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
            id: InventoryId::random(),
            owner,
            role,
            revision: 1,
            minted: 0,
            entries: Vec::new(),
        }
    }

    /// An identity for an entry this inventory creates, from the inventory's own and how
    /// many it created before. A command replayed after a load makes the same items, and
    /// one that is undone gives the numbers back with the rest of the inventory.
    fn mint(&mut self) -> ItemId {
        let mut hash = blake3::Hasher::new();
        hash.update(b"yarra-item-v2");
        hash.update(&self.id.0);
        hash.update(&self.minted.to_le_bytes());
        self.minted += 1;
        let mut id = [0; 16];
        id.copy_from_slice(&hash.finalize().as_bytes()[..16]);
        ItemId(id)
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
        if quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }

        room(catalog, self, definition, quantity)?;
        let revision = next_revision(self.revision)?;
        let entry = ItemEntry {
            id: self.mint(),
            definition,
            quantity,
        };
        let ids = deposit(catalog, self, entry)?;
        self.revision = revision;
        Ok(ids)
    }

    /// Authoritative consumption/destruction. Gameplay must authorize the reason.
    /// Use discard for user-requested destruction, which respects discardable.
    pub fn remove(&mut self, entry: ItemId, quantity: u32) -> Result<()> {
        let revision = next_revision(self.revision)?;
        withdraw(self, entry, quantity)?;
        self.revision = revision;
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
        self.remove(entry, quantity)
    }

    /// Splitting gives the new stack a new ID; the remaining stack keeps its ID.
    pub fn split_stack(&mut self, entry: ItemId, quantity: u32) -> Result<ItemId> {
        if quantity == 0 || quantity >= self.entry(entry)?.quantity {
            return Err(InventoryError::InvalidQuantity);
        }

        if self.entries.len() == MAX_INVENTORY_ENTRIES {
            return Err(InventoryError::Capacity);
        }

        let revision = next_revision(self.revision)?;
        let split = withdraw(self, entry, quantity)?;
        let id = split.id;
        self.entries.push(split);
        self.revision = revision;
        Ok(id)
    }

    /// Merge the whole source stack into the destination; source ID is retired.
    pub fn merge_stacks(
        &mut self,
        catalog: &ItemCatalog,
        source: ItemId,
        destination: ItemId,
    ) -> Result<()> {
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

/// Moves items from one inventory to another, both changed or neither. Compatible stacks
/// merge; the returned IDs identify the destination's entries. A whole entry that does not
/// merge keeps its ID, as every non-stackable item does.
pub fn transfer(
    catalog: &ItemCatalog,
    source: &mut Inventory,
    destination: &mut Inventory,
    entry: ItemId,
    quantity: u32,
) -> Result<Vec<ItemId>> {
    validate_pair(source, destination)?;
    let held = source.entry(entry)?;
    let definition = held.definition;

    if !catalog.item(definition)?.permissions.transferable {
        return Err(InventoryError::Restricted);
    }

    if quantity == 0 || quantity > held.quantity {
        return Err(InventoryError::InvalidQuantity);
    }

    room(catalog, destination, definition, quantity)?;
    let revisions = (
        next_revision(source.revision)?,
        next_revision(destination.revision)?,
    );
    let moved = withdraw(source, entry, quantity)?;
    let ids = deposit(catalog, destination, moved)?;
    (source.revision, destination.revision) = revisions;
    Ok(ids)
}

/// Two inventories items pass between: distinct, and no item in both.
pub(crate) fn validate_pair(a: &Inventory, b: &Inventory) -> Result<()> {
    if a.id == b.id {
        return Err(InventoryError::SameParticipant);
    }

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
    let definition = entry.definition;
    Ok(ItemEntry {
        id: inventory.mint(),
        definition,
        quantity,
    })
}

/// Whether `quantity` of an item fits: topping up stacks that have room, then in new entries.
pub(crate) fn room(
    catalog: &ItemCatalog,
    inventory: &Inventory,
    definition: ItemDefinitionId,
    quantity: u32,
) -> Result<()> {
    let limit = catalog.item(definition)?.stack_limit;
    let mut rest = u64::from(quantity);

    if limit > 1 {
        for existing in &inventory.entries {
            if existing.definition == definition {
                rest -= rest.min(u64::from(limit - existing.quantity));
            }
        }
    }

    let needed = rest.div_ceil(u64::from(limit));

    if needed > (MAX_INVENTORY_ENTRIES - inventory.entries.len()) as u64 {
        return Err(InventoryError::Capacity);
    }

    Ok(())
}

/// Adds an entry, merging into stacks with room first. Checks that it fits before changing
/// anything.
pub(crate) fn deposit(
    catalog: &ItemCatalog,
    inventory: &mut Inventory,
    mut incoming: ItemEntry,
) -> Result<Vec<ItemId>> {
    let limit = catalog.item(incoming.definition)?.stack_limit;

    if inventory.entries.iter().any(|e| e.id == incoming.id) {
        return Err(InventoryError::Duplicate);
    }

    room(catalog, inventory, incoming.definition, incoming.quantity)?;
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

    while incoming.quantity > 0 {
        let quantity = incoming.quantity.min(limit);
        ids.push(incoming.id);
        inventory.entries.push(ItemEntry {
            quantity,
            ..incoming.clone()
        });
        incoming.quantity -= quantity;

        if incoming.quantity > 0 {
            incoming.id = inventory.mint();
        }
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
            id: WalletId::random(),
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

/// Moves money between wallets, both changed or neither.
pub fn transfer_money(source: &mut Wallet, destination: &mut Wallet, amount: Money) -> Result<()> {
    if source.id == destination.id {
        return Err(InventoryError::SameParticipant);
    }

    // Both sides are checked before either changes.
    source.balance.checked_sub(amount)?;
    destination.balance.checked_add(amount)?;
    source.debit(amount)?;
    destination.credit(amount)
}
