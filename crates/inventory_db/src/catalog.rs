use crate::{InventoryDbError, Result, storage};
use inventory::{Category, CategoryId, ItemCatalog};
use rusqlite::{Connection, TransactionBehavior};
use std::path::Path;

pub struct CatalogStore {
    connection: Connection,
}
impl CatalogStore {
    pub fn create(path: impl AsRef<Path>, catalog: &ItemCatalog) -> Result<Self> {
        Ok(Self {
            connection: storage::create(path.as_ref(), catalog)?,
        })
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = Self {
            connection: storage::open(path.as_ref())?,
        };
        store.read()?;
        Ok(store)
    }
    pub fn read(&self) -> Result<ItemCatalog> {
        let transaction = self.connection.unchecked_transaction()?;
        let catalog = storage::read_catalog(&transaction)?;
        transaction.commit()?;
        Ok(catalog)
    }
    pub fn categories(&self) -> Result<Vec<Category>> {
        crate::catalog_storage::read_categories(&self.connection)
    }
    /// Convenience command. Returns the new catalog revision.
    pub fn put_category(&mut self, expected_revision: u64, category: Category) -> Result<u64> {
        let mut catalog = self.read()?;
        catalog.put_category(category)?;
        self.replace(expected_revision, &catalog)?;
        Ok(catalog.revision)
    }
    pub fn remove_category(&mut self, expected_revision: u64, id: CategoryId) -> Result<u64> {
        let mut catalog = self.read()?;
        catalog.remove_category(id)?;
        self.replace(expected_revision, &catalog)?;
        Ok(catalog.revision)
    }
    /// Persist one or several domain edits. Old saves keep their pinned snapshot.
    /// Identity must match, revision must advance, and the baseline must be current.
    pub fn replace(&mut self, expected_revision: u64, replacement: &ItemCatalog) -> Result<()> {
        storage::validate_catalog(replacement)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = storage::read_catalog(&transaction)?;
        if current.revision != expected_revision {
            return Err(InventoryDbError::Conflict);
        }
        if current.id != replacement.id || replacement.revision <= current.revision {
            return Err(InventoryDbError::InvalidData(
                "catalog identity must match and revision must advance".into(),
            ));
        }
        crate::catalog_storage::write_catalog(&transaction, replacement)?;
        transaction.commit()?;
        Ok(())
    }
}
