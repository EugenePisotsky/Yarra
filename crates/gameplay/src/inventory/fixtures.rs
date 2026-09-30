//! Small deterministic catalog for standalone examples and contract tests.
use super::*;

pub const POTION: ItemDefinitionId = ItemDefinitionId::named("healing_potion");
pub const SWORD: ItemDefinitionId = ItemDefinitionId::named("iron_sword");
pub const KEY: ItemDefinitionId = ItemDefinitionId::named("old_gate_key");
pub const KEYS: CategoryId = CategoryId::named("keys");
pub const WEAPONS: CategoryId = CategoryId::named("weapons");
pub const ARMOUR: CategoryId = CategoryId::named("armour");
pub const POTIONS: CategoryId = CategoryId::named("potions");

/// Parses the name so the process knows it, and checks it is the constant's identity.
fn named<T: TryFrom<String, Error = game_types::Invalid> + PartialEq + Copy>(
    id: T,
    name: &str,
) -> T {
    assert!(T::try_from(name.to_owned()).is_ok_and(|parsed| parsed == id));
    id
}
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
        .map(|(id, key, name)| {
            let id = named(id, key);
            let name = name.into();
            (id, Category { id, name })
        })
        .collect(),
        items: Default::default(),
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
        let id = named(id, key);
        catalog.items.insert(
            id,
            ItemDefinition {
                id,
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
            },
        );
    }
    let key = catalog.items.get_mut(&KEY).unwrap();
    key.tags.insert("quest".into());
    key.permissions.sellable = false;
    key.permissions.discardable = false;
    key.merchant_buy_price = None;
    key.merchant_sell_price = None;
    catalog
}
