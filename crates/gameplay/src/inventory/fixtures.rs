//! Small deterministic catalog for standalone examples and contract tests.
use super::*;

pub const POTION: ItemDefinitionId = ItemDefinitionId([1; 16]);
pub const SWORD: ItemDefinitionId = ItemDefinitionId([2; 16]);
pub const KEY: ItemDefinitionId = ItemDefinitionId([3; 16]);
pub const KEYS: CategoryId = CategoryId([11; 16]);
pub const WEAPONS: CategoryId = CategoryId([12; 16]);
pub const ARMOUR: CategoryId = CategoryId([13; 16]);
pub const POTIONS: CategoryId = CategoryId([14; 16]);

pub fn example_catalog() -> ItemCatalog {
    let mut catalog = ItemCatalog {
        id: CatalogId([1; 16]),
        revision: 1,
        categories: [
            (KEYS, "keys", "Keys"),
            (WEAPONS, "weapons", "Weapons"),
            (ARMOUR, "armour", "Armour"),
            (POTIONS, "potions", "Potions"),
        ]
        .into_iter()
        .map(|(id, key, name)| Category {
            id,
            key: key.into(),
            name: name.into(),
        })
        .collect(),
        items: Vec::new(),
    };
    for (id, key, name, category, stack_limit, weight, ask, bid) in [
        (
            POTION,
            "healing_potion",
            "Healing potion",
            POTIONS,
            10,
            250,
            20,
            8,
        ),
        (SWORD, "iron_sword", "Iron sword", WEAPONS, 1, 2000, 100, 40),
        (KEY, "old_gate_key", "Old gate key", KEYS, 1, 50, 0, 0),
    ] {
        catalog.items.push(ItemDefinition {
            id,
            key: key.into(),
            name: name.into(),
            category,
            description: "".into(),
            tags: Default::default(),
            icon: None,
            stack_limit,
            weight_grams: weight,
            merchant_sell_price: Some(Money::new(ask).unwrap()),
            merchant_buy_price: Some(Money::new(bid).unwrap()),
            permissions: ItemPermissions::default(),
            mechanics: Default::default(),
        });
    }
    let key = catalog.items.last_mut().unwrap();
    key.tags.insert("quest".into());
    key.permissions.sellable = false;
    key.permissions.discardable = false;
    key.merchant_buy_price = None;
    key.merchant_sell_price = None;
    catalog
}
