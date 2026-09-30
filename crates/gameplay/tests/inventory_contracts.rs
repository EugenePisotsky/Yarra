use yarra_gameplay::inventory::{fixtures::*, *};

fn owner() -> OwnerRef {
    OwnerRef::actor(ActorId::random())
}
fn bag() -> Inventory {
    Inventory::new(owner(), InventoryRole::Carried)
}
fn money(n: u64) -> Money {
    Money::new(n).unwrap()
}
fn count(bag: &Inventory, definition: ItemDefinitionId) -> u32 {
    bag.entries
        .iter()
        .filter(|e| e.definition == definition)
        .map(|e| e.quantity)
        .sum()
}
fn wallet(balance: u64) -> Wallet {
    let mut wallet = Wallet::new(owner());
    wallet.credit(money(balance)).unwrap();
    wallet
}

#[test]
fn catalog_edits_validate_identity_categories_and_limits_atomically() {
    let mut catalog = example_catalog();
    let before = catalog.clone();
    assert_eq!(
        catalog.remove_category(POTIONS),
        Err(InventoryError::CategoryInUse(POTIONS))
    );
    let mut invalid = catalog.item(POTION).unwrap().clone();
    invalid.stack_limit = 0;
    assert!(catalog.put_item(invalid).is_err());
    assert_eq!(catalog, before);
    let mut updated = catalog.item(POTION).unwrap().clone();
    updated.merchant_buy_price = Some(money(12));
    catalog.put_item(updated).unwrap();
    assert_eq!(catalog.revision, before.revision + 1);
    catalog.remove_item(POTION).unwrap();
    catalog.remove_category(POTIONS).unwrap();
    catalog.validate().unwrap();
}

#[test]
fn category_identity_survives_title_edits() {
    let mut catalog = example_catalog();
    let item_before = catalog.item(POTION).unwrap().clone();
    catalog
        .put_category(Category {
            id: POTIONS,
            name: "Alchemical supplies".into(),
        })
        .unwrap();
    assert_eq!(catalog.item(POTION).unwrap(), &item_before);
    assert_eq!(catalog.categories.len(), 4);
    let before = catalog.clone();
    assert_eq!(
        catalog.remove_category(POTIONS),
        Err(InventoryError::CategoryInUse(POTIONS))
    );
    assert_eq!(catalog, before);
}

#[test]
fn partial_transfers_conserve_quantity_and_stack_limits_for_every_split() {
    let catalog = example_catalog();
    for quantity in 1..=10 {
        let mut source = bag();
        let mut destination = bag();
        let source_id = source.grant(&catalog, POTION, 10).unwrap()[0];
        destination.grant(&catalog, POTION, 7).unwrap();
        transfer(&catalog, &mut source, &mut destination, source_id, quantity).unwrap();
        assert_eq!(count(&source, POTION), 10 - quantity);
        assert_eq!(count(&destination, POTION), 7 + quantity);
        assert_eq!(count(&source, POTION) + count(&destination, POTION), 17);
        source.validate(&catalog).unwrap();
        destination.validate(&catalog).unwrap();
        if quantity < 10 {
            assert_eq!(source.entries[0].id, source_id);
        }
    }
}

#[test]
fn unique_items_keep_identity_and_do_not_merge() {
    let catalog = example_catalog();
    let mut source = bag();
    let mut destination = bag();
    let swords = source.grant(&catalog, SWORD, 2).unwrap();
    assert_ne!(swords[0], swords[1]);
    assert_eq!(
        source.merge_stacks(&catalog, swords[0], swords[1]),
        Err(InventoryError::IncompatibleStack)
    );
    transfer(&catalog, &mut source, &mut destination, swords[0], 1).unwrap();
    assert_eq!(destination.entries[0].id, swords[0]);
    assert_eq!(source.entries[0].id, swords[1]);
}

#[test]
fn split_merge_filter_and_weight_agree() {
    let catalog = example_catalog();
    let mut inventory = bag();
    let first = inventory.grant(&catalog, POTION, 10).unwrap()[0];
    let split = inventory.split_stack(&catalog, first, 3).unwrap();
    assert_ne!(first, split);
    assert_eq!(inventory.entry(first).unwrap().quantity, 7);
    inventory.merge_stacks(&catalog, split, first).unwrap();
    let key = inventory.grant(&catalog, KEY, 1).unwrap()[0];
    assert_eq!(inventory.total_weight_grams(&catalog).unwrap(), 2550);
    assert_eq!(
        inventory
            .list_items(
                &catalog,
                ItemFilter {
                    category: Some(POTIONS),
                    tag: None
                }
            )
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        inventory
            .list_items(
                &catalog,
                ItemFilter {
                    category: None,
                    tag: Some("quest")
                }
            )
            .unwrap()[0]
            .id,
        key
    );
}

#[test]
fn failed_transfers_and_grants_do_not_lose_items() {
    let catalog = example_catalog();
    let mut source = bag();
    let id = source.grant(&catalog, POTION, 10).unwrap()[0];
    let mut full = bag();
    full.grant(&catalog, SWORD, MAX_INVENTORY_ENTRIES as u32)
        .unwrap();
    let before = (source.clone(), full.clone());
    assert_eq!(
        transfer(&catalog, &mut source, &mut full, id, 1),
        Err(InventoryError::Capacity)
    );
    assert_eq!((source.clone(), full.clone()), before);
    assert_eq!(
        source.grant(&catalog, SWORD, u32::MAX),
        Err(InventoryError::Capacity)
    );
    assert_eq!(source, before.0);
    for bad in [0, 11] {
        assert_eq!(
            source.remove(&catalog, id, bad),
            Err(InventoryError::InvalidQuantity)
        );
        assert_eq!(source, before.0);
    }
}

#[test]
fn quest_items_can_be_transferred_but_not_discarded_or_sold() {
    let catalog = example_catalog();
    let mut source = bag();
    let mut destination = bag();
    let key = source.grant(&catalog, KEY, 1).unwrap()[0];
    assert_eq!(
        source.discard(&catalog, key, 1),
        Err(InventoryError::Restricted)
    );
    transfer(&catalog, &mut source, &mut destination, key, 1).unwrap();
    let offer = TradeOffer {
        sales: vec![TradeLine {
            entry: key,
            quantity: 1,
        }],
        ..Default::default()
    };
    assert_eq!(
        quote_trade(
            &catalog,
            &source,
            &destination,
            &wallet(100),
            &wallet(100),
            &offer
        ),
        Err(InventoryError::Restricted)
    );
    // Quest completion can explicitly consume it.
    destination.remove(&catalog, key, 1).unwrap();
}

#[test]
fn wallet_transfers_are_atomic_and_check_overflow() {
    let mut from = wallet(10);
    let mut to = wallet(Money::MAX.units());
    let before = (from.clone(), to.clone());
    assert_eq!(
        transfer_money(&mut from, &mut to, money(1)),
        Err(InventoryError::Overflow)
    );
    assert_eq!((from.clone(), to.clone()), before);
    assert_eq!(
        transfer_money(&mut from, &mut to, money(11)),
        Err(InventoryError::InsufficientFunds)
    );
    assert_eq!((from, to), before);
    assert!(Money::new(u64::MAX).is_err());
}

struct TradeFixture {
    catalog: ItemCatalog,
    merchant: Inventory,
    customer: Inventory,
    merchant_wallet: Wallet,
    customer_wallet: Wallet,
    offer: TradeOffer,
}
impl TradeFixture {
    fn new() -> Self {
        let catalog = example_catalog();
        let mut merchant = bag();
        let mut customer = bag();
        let potion = merchant.grant(&catalog, POTION, 10).unwrap()[0];
        let sword = customer.grant(&catalog, SWORD, 1).unwrap()[0];
        Self {
            catalog,
            merchant,
            customer,
            merchant_wallet: wallet(100),
            customer_wallet: wallet(10),
            offer: TradeOffer {
                purchases: vec![TradeLine {
                    entry: potion,
                    quantity: 2,
                }],
                sales: vec![TradeLine {
                    entry: sword,
                    quantity: 1,
                }],
            },
        }
    }
    fn quote(&self) -> Result<TradeQuote> {
        quote_trade(
            &self.catalog,
            &self.merchant,
            &self.customer,
            &self.merchant_wallet,
            &self.customer_wallet,
            &self.offer,
        )
    }
    fn commit(&mut self, quote: &TradeQuote) -> Result<()> {
        commit_trade(
            &self.catalog,
            &mut self.merchant,
            &mut self.customer,
            &mut self.merchant_wallet,
            &mut self.customer_wallet,
            quote,
        )
    }
}

#[test]
fn basket_trade_uses_separate_prices_and_nets_payment() {
    let mut fixture = TradeFixture::new();
    let quote = fixture.quote().unwrap();
    assert_eq!(quote.purchase_total(), money(40));
    assert_eq!(quote.sale_total(), money(40));
    fixture.commit(&quote).unwrap();
    assert_eq!(count(&fixture.customer, POTION), 2);
    assert_eq!(count(&fixture.merchant, POTION), 8);
    assert_eq!(count(&fixture.merchant, SWORD), 1);
    assert_eq!(fixture.customer_wallet.balance, money(10));
    assert_eq!(fixture.merchant_wallet.balance, money(100));
    assert_eq!(fixture.commit(&quote), Err(InventoryError::StaleQuote));
}

#[test]
fn both_payment_directions_conserve_money_and_items() {
    for purchases in [1, 3] {
        let mut fixture = TradeFixture::new();
        fixture.customer_wallet.credit(money(100)).unwrap();
        fixture.offer.purchases[0].quantity = purchases;
        let quote = fixture.quote().unwrap();
        fixture.commit(&quote).unwrap();
        assert_eq!(
            fixture.customer_wallet.balance.units(),
            110 + 40 - 20 * u64::from(purchases)
        );
        assert_eq!(
            fixture.customer_wallet.balance.units() + fixture.merchant_wallet.balance.units(),
            210
        );
        assert_eq!(
            count(&fixture.customer, POTION) + count(&fixture.merchant, POTION),
            10
        );
    }
}

#[test]
fn stale_inventory_wallet_or_catalog_rejects_quote_without_mutation() {
    for changed in 0..3 {
        let mut fixture = TradeFixture::new();
        let quote = fixture.quote().unwrap();
        match changed {
            0 => {
                fixture.merchant.grant(&fixture.catalog, KEY, 1).unwrap();
            }
            1 => fixture.customer_wallet.credit(money(1)).unwrap(),
            _ => fixture
                .catalog
                .put_category(Category {
                    id: CategoryId::random(),
                    name: "Food".into(),
                })
                .unwrap(),
        }
        let before = (
            fixture.merchant.clone(),
            fixture.customer.clone(),
            fixture.merchant_wallet.clone(),
            fixture.customer_wallet.clone(),
        );
        assert_eq!(fixture.commit(&quote), Err(InventoryError::StaleQuote));
        assert_eq!(
            (
                fixture.merchant,
                fixture.customer,
                fixture.merchant_wallet,
                fixture.customer_wallet
            ),
            before
        );
    }
}

#[test]
fn trade_rejects_duplicate_lines_missing_prices_and_insufficient_funds() {
    let mut fixture = TradeFixture::new();
    fixture
        .offer
        .purchases
        .push(fixture.offer.purchases[0].clone());
    assert_eq!(fixture.quote(), Err(InventoryError::Duplicate));
    fixture.offer.purchases.pop();
    fixture.offer.sales.clear();
    assert_eq!(fixture.quote(), Err(InventoryError::InsufficientFunds));
    fixture.customer_wallet.credit(money(100)).unwrap();
    fixture.catalog.items[0].merchant_sell_price = None;
    assert_eq!(fixture.quote(), Err(InventoryError::NotForTrade));
    fixture.catalog.items[0].merchant_sell_price = Some(Money::ZERO);
    assert_eq!(fixture.quote().unwrap().purchase_total(), Money::ZERO);
    fixture.catalog.items[0].merchant_sell_price = Some(Money::MAX);
    assert_eq!(fixture.quote(), Err(InventoryError::Overflow));
}

#[test]
fn full_inventories_can_exchange_items_when_final_capacity_fits() {
    let mut fixture = TradeFixture::new();
    fixture
        .merchant
        .grant(&fixture.catalog, SWORD, (MAX_INVENTORY_ENTRIES - 1) as u32)
        .unwrap();
    fixture
        .customer
        .grant(&fixture.catalog, KEY, (MAX_INVENTORY_ENTRIES - 1) as u32)
        .unwrap();
    fixture.customer_wallet.credit(money(1000)).unwrap();
    fixture.offer.purchases[0].quantity = 10;
    let quote = fixture.quote().unwrap();
    fixture.commit(&quote).unwrap();
    assert_eq!(fixture.merchant.entries.len(), MAX_INVENTORY_ENTRIES);
    assert_eq!(fixture.customer.entries.len(), MAX_INVENTORY_ENTRIES);
}

#[test]
fn revision_overflow_is_atomic() {
    let catalog = example_catalog();
    let mut inventory = bag();
    inventory.revision = i64::MAX as u64;
    let before = inventory.clone();
    assert_eq!(
        inventory.grant(&catalog, POTION, 1),
        Err(InventoryError::Overflow)
    );
    assert_eq!(inventory, before);
}
