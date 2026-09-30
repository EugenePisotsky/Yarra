use inventory::{fixtures::*, *};
use rusqlite::Connection;
use yarra_inventory_db::*;
struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("yarra-catalog-{:?}.sqlite", OwnerId::new().0)))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-wal", self.0.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.0.display()));
    }
}
#[test]
fn category_management_is_relational_revision_checked_and_atomic() {
    let temp = Temp::new();
    let catalog = example_catalog();
    let mut store = CatalogStore::create(&temp.0, &catalog).unwrap();
    let mut renamed = catalog.categories[0].clone();
    renamed.name = "Quest keys".into();
    store.put_category(1, renamed.clone()).unwrap();
    assert_eq!(store.read().unwrap().category(KEYS).unwrap(), &renamed);
    assert!(store.put_category(1, renamed).is_err());
    assert!(store.remove_category(2, KEYS).is_err());
    let connection = Connection::open(&temp.0).unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    assert!(
        connection
            .execute("DELETE FROM inventory_categories WHERE id=?1", [KEYS.0])
            .is_err()
    );
    let before = store.read().unwrap();
    let mut next = before.clone();
    next.items[0].weight_grams = 999;
    next.revision += 1;
    connection.execute_batch("CREATE TRIGGER reject_item BEFORE INSERT ON inventory_definitions BEGIN SELECT RAISE(ABORT,'reject'); END;").unwrap();
    assert!(store.replace(before.revision, &next).is_err());
    assert_eq!(store.read().unwrap(), before);
    assert!(CatalogStore::create(&temp.0, &catalog).is_err());
    drop(store);
    assert_eq!(CatalogStore::open(&temp.0).unwrap().read().unwrap(), before);
}
#[test]
fn catalog_adapter_uses_callers_transaction() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    connection.execute_batch(CATALOG_SCHEMA).unwrap();
    let catalog = example_catalog();
    {
        let tx = connection.transaction().unwrap();
        write_catalog(&tx, &catalog).unwrap();
        tx.commit().unwrap();
    }
    {
        let tx = connection.transaction().unwrap();
        let mut changed = catalog.clone();
        changed.revision += 1;
        changed.items[0].weight_grams += 1;
        write_catalog(&tx, &changed).unwrap(); // Rollback on drop.
    }
    let tx = connection.transaction().unwrap();
    assert_eq!(read_catalog(&tx).unwrap(), catalog);
}
