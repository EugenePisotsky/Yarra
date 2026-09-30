//! Relational catalog records shared by authored catalogs and pinned save catalogs.
use crate::{InventoryDbError, Result, storage};
use inventory::*;
use rusqlite::{Connection, Transaction, params};

pub const CATALOG_SCHEMA: &str = "
CREATE TABLE inventory_catalog (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    id BLOB NOT NULL CHECK(length(id) = 16),
    revision INTEGER NOT NULL CHECK(revision > 0)
) STRICT;
CREATE TABLE inventory_categories (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    key TEXT NOT NULL UNIQUE CHECK(length(CAST(key AS BLOB)) BETWEEN 1 AND 96),
    name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 65536),
    position INTEGER NOT NULL UNIQUE CHECK(position BETWEEN 0 AND 255)
) STRICT;
CREATE TABLE inventory_definitions (
    id BLOB PRIMARY KEY CHECK(length(id) = 16),
    key TEXT NOT NULL UNIQUE CHECK(length(CAST(key AS BLOB)) BETWEEN 1 AND 96),
    name TEXT NOT NULL CHECK(length(CAST(name AS BLOB)) BETWEEN 1 AND 65536),
    description TEXT NOT NULL CHECK(length(CAST(description AS BLOB)) <= 65536),
    category_id BLOB NOT NULL REFERENCES inventory_categories(id) ON DELETE RESTRICT,
    tags TEXT NOT NULL CHECK(length(CAST(tags AS BLOB)) <= 8192),
    icon TEXT CHECK(icon IS NULL OR length(CAST(icon AS BLOB)) BETWEEN 1 AND 1024),
    weight_grams INTEGER NOT NULL CHECK(weight_grams BETWEEN 0 AND 4294967295),
    stack_limit INTEGER NOT NULL CHECK(stack_limit BETWEEN 1 AND 1000000),
    merchant_sell_price INTEGER CHECK(merchant_sell_price >= 0),
    merchant_buy_price INTEGER CHECK(merchant_buy_price >= 0),
    transferable INTEGER NOT NULL CHECK(transferable IN (0, 1)),
    sellable INTEGER NOT NULL CHECK(sellable IN (0, 1)),
    discardable INTEGER NOT NULL CHECK(discardable IN (0, 1)),
    mechanics TEXT NOT NULL CHECK(length(CAST(mechanics AS BLOB)) <= 65536),
    position INTEGER NOT NULL UNIQUE CHECK(position BETWEEN 0 AND 9999)
) STRICT;
CREATE INDEX inventory_definitions_category ON inventory_definitions(category_id);
";

pub(crate) fn validate_catalog(catalog: &ItemCatalog) -> Result<()> {
    catalog.validate()?;
    // Bound authored/pinned content independently of SQLite column limits.
    if serde_json::to_vec(catalog)?.len() > 16 * 1024 * 1024 {
        return Err(InventoryDbError::InvalidData(
            "catalog exceeds 16 MiB".into(),
        ));
    }
    Ok(())
}

pub(crate) fn read_categories(connection: &Connection) -> Result<Vec<Category>> {
    let mut statement = connection
        .prepare("SELECT id, key, name FROM inventory_categories ORDER BY position LIMIT 257")?;
    let categories = statement
        .query_map([], |row| {
            Ok(Category {
                id: CategoryId(storage::id(row, 0)?),
                key: row.get(1)?,
                name: storage::json(row, 2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if categories.len() > MAX_CATEGORIES {
        return Err(InventoryDbError::InvalidData("too many categories".into()));
    }
    Ok(categories)
}

/// The caller holds a read or write transaction for a consistent multi-table snapshot.
pub fn read_catalog(connection: &Transaction<'_>) -> Result<ItemCatalog> {
    let (id, revision) = connection.query_row(
        "SELECT id, revision FROM inventory_catalog WHERE singleton = 1",
        [],
        |row| Ok((CatalogId(storage::id(row, 0)?), storage::unsigned(row, 1)?)),
    )?;
    let categories = read_categories(connection)?;
    let mut statement = connection.prepare("SELECT id, key, name, description, category_id, tags, icon, weight_grams, stack_limit, merchant_sell_price, merchant_buy_price, transferable, sellable, discardable, mechanics FROM inventory_definitions ORDER BY position LIMIT 10001")?;
    let mut rows = statement.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let tags: String = row.get(5)?;
        items.push(ItemDefinition {
            id: ItemDefinitionId(storage::id(row, 0)?),
            key: row.get(1)?,
            name: storage::json(row, 2)?,
            description: storage::json(row, 3)?,
            category: CategoryId(storage::id(row, 4)?),
            tags: serde_json::from_str(&tags)?,
            icon: row.get(6)?,
            weight_grams: row.get(7)?,
            stack_limit: row.get(8)?,
            merchant_sell_price: read_price(row, 9)?,
            merchant_buy_price: read_price(row, 10)?,
            mechanics: storage::json(row, 14)?,
            permissions: ItemPermissions {
                transferable: row.get(11)?,
                sellable: row.get(12)?,
                discardable: row.get(13)?,
            },
        });
    }
    let catalog = ItemCatalog {
        id,
        revision,
        categories,
        items,
    };
    validate_catalog(&catalog)?;
    Ok(catalog)
}

fn read_price(row: &rusqlite::Row<'_>, index: usize) -> Result<Option<Money>> {
    row.get::<_, Option<i64>>(index)?
        .map(|n| {
            Money::new(
                u64::try_from(n)
                    .map_err(|_| InventoryDbError::InvalidData("negative price".into()))?,
            )
            .map_err(Into::into)
        })
        .transpose()
}

/// Replace the catalog inside the caller's transaction. Existing referencing
/// entries must be cleared first when replacing an entire checkpoint.
pub fn write_catalog(transaction: &Transaction<'_>, catalog: &ItemCatalog) -> Result<()> {
    validate_catalog(catalog)?;
    transaction.execute("DELETE FROM inventory_definitions", [])?;
    transaction.execute("DELETE FROM inventory_categories", [])?;
    transaction.execute("INSERT INTO inventory_catalog VALUES (1, ?1, ?2) ON CONFLICT(singleton) DO UPDATE SET id = excluded.id, revision = excluded.revision", params![catalog.id.0, storage::integer(catalog.revision)?])?;
    let mut category_insert =
        transaction.prepare_cached("INSERT INTO inventory_categories VALUES (?1, ?2, ?3, ?4)")?;
    for (position, category) in catalog.categories.iter().enumerate() {
        category_insert.execute(params![
            category.id.0,
            category.key,
            serde_json::to_string(&category.name)?,
            position as i64
        ])?;
    }
    let mut item_insert = transaction.prepare_cached("INSERT INTO inventory_definitions VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)")?;
    for (position, item) in catalog.items.iter().enumerate() {
        item_insert.execute(params![
            item.id.0,
            item.key,
            serde_json::to_string(&item.name)?,
            serde_json::to_string(&item.description)?,
            item.category.0,
            serde_json::to_string(&item.tags)?,
            item.icon,
            item.weight_grams,
            item.stack_limit,
            item.merchant_sell_price
                .map(|p| storage::integer(p.units()))
                .transpose()?,
            item.merchant_buy_price
                .map(|p| storage::integer(p.units()))
                .transpose()?,
            item.permissions.transferable,
            item.permissions.sellable,
            item.permissions.discardable,
            serde_json::to_string(&item.mechanics)?,
            position as i64
        ])?;
    }
    Ok(())
}
