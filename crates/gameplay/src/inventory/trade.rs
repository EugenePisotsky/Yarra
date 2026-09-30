use super::{
    Inventory, ItemCatalog, Wallet,
    items::{deposit, validate_pair, withdraw},
    transfer_money,
    types::*,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeLine {
    pub entry: ItemId,
    pub quantity: u32,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeOffer {
    /// Entries from the merchant's stock.
    pub purchases: Vec<TradeLine>,
    /// Entries from the customer's inventory.
    pub sales: Vec<TradeLine>,
}

/// Explicit wallets allow a party wallet to pay for an individual character.
/// Gameplay must authorize all four IDs, including access to shop stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeParticipants {
    pub merchant_inventory: InventoryId,
    pub customer_inventory: InventoryId,
    pub merchant_wallet: WalletId,
    pub customer_wallet: WalletId,
}

/// Immutable preview. Only quote_trade can construct one. Commit checks all
/// revisions and recalculates prices; a quote can never be reused after a trade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeQuote {
    participants: TradeParticipants,
    catalog: (CatalogId, u64),
    revisions: [u64; 4],
    offer: TradeOffer,
    purchase_total: Money,
    sale_total: Money,
}
impl TradeQuote {
    pub fn participants(&self) -> TradeParticipants {
        self.participants
    }
    pub fn offer(&self) -> &TradeOffer {
        &self.offer
    }
    pub fn purchase_total(&self) -> Money {
        self.purchase_total
    }
    pub fn sale_total(&self) -> Money {
        self.sale_total
    }
}

pub fn quote_trade(
    catalog: &ItemCatalog,
    merchant: &Inventory,
    customer: &Inventory,
    merchant_wallet: &Wallet,
    customer_wallet: &Wallet,
    offer: &TradeOffer,
) -> Result<TradeQuote> {
    validate_pair(catalog, merchant, customer)?;
    merchant_wallet.validate()?;
    customer_wallet.validate()?;
    if merchant_wallet.id == customer_wallet.id {
        return Err(InventoryError::SameParticipant);
    }
    if offer.purchases.len() + offer.sales.len() == 0 {
        return Err(InventoryError::Invalid("empty trade".into()));
    }
    let quote = TradeQuote {
        participants: TradeParticipants {
            merchant_inventory: merchant.id,
            customer_inventory: customer.id,
            merchant_wallet: merchant_wallet.id,
            customer_wallet: customer_wallet.id,
        },
        catalog: (catalog.id, catalog.revision),
        revisions: [
            merchant.revision,
            customer.revision,
            merchant_wallet.revision,
            customer_wallet.revision,
        ],
        purchase_total: price_lines(catalog, merchant, &offer.purchases, true)?,
        sale_total: price_lines(catalog, customer, &offer.sales, false)?,
        offer: offer.clone(),
    };
    // Preview final capacity and settlement as well as prices, without mutating inputs.
    prepare_trade(
        catalog,
        merchant,
        customer,
        merchant_wallet,
        customer_wallet,
        &quote,
    )?;
    Ok(quote)
}

/// All four records change together, or remain byte-for-byte unchanged on error.
pub fn commit_trade(
    catalog: &ItemCatalog,
    merchant: &mut Inventory,
    customer: &mut Inventory,
    merchant_wallet: &mut Wallet,
    customer_wallet: &mut Wallet,
    quote: &TradeQuote,
) -> Result<()> {
    let participants = TradeParticipants {
        merchant_inventory: merchant.id,
        customer_inventory: customer.id,
        merchant_wallet: merchant_wallet.id,
        customer_wallet: customer_wallet.id,
    };
    if participants != quote.participants
        || (catalog.id, catalog.revision) != quote.catalog
        || [
            merchant.revision,
            customer.revision,
            merchant_wallet.revision,
            customer_wallet.revision,
        ] != quote.revisions
    {
        return Err(InventoryError::StaleQuote);
    }
    let current = quote_trade(
        catalog,
        merchant,
        customer,
        merchant_wallet,
        customer_wallet,
        &quote.offer,
    )?;
    if current != *quote {
        return Err(InventoryError::StaleQuote);
    }
    let (next_merchant, next_customer, next_merchant_wallet, next_customer_wallet) = prepare_trade(
        catalog,
        merchant,
        customer,
        merchant_wallet,
        customer_wallet,
        quote,
    )?;
    *merchant = next_merchant;
    *customer = next_customer;
    *merchant_wallet = next_merchant_wallet;
    *customer_wallet = next_customer_wallet;
    Ok(())
}

fn price_lines(
    catalog: &ItemCatalog,
    inventory: &Inventory,
    lines: &[TradeLine],
    purchase: bool,
) -> Result<Money> {
    let mut total = Money::ZERO;
    let mut seen = BTreeSet::new();
    for line in lines {
        if !seen.insert(line.entry) {
            return Err(InventoryError::Duplicate);
        }
        let entry = inventory.entry(line.entry)?;
        if line.quantity == 0 || line.quantity > entry.quantity {
            return Err(InventoryError::InvalidQuantity);
        }
        let definition = catalog.item(entry.definition)?;
        if !definition.permissions.sellable || !definition.permissions.transferable {
            return Err(InventoryError::Restricted);
        }
        let price = if purchase {
            definition.merchant_sell_price
        } else {
            definition.merchant_buy_price
        }
        .ok_or(InventoryError::NotForTrade)?;
        total = total.checked_add(price.checked_mul(line.quantity)?)?;
    }
    Ok(total)
}

fn prepare_trade(
    catalog: &ItemCatalog,
    merchant: &Inventory,
    customer: &Inventory,
    merchant_wallet: &Wallet,
    customer_wallet: &Wallet,
    quote: &TradeQuote,
) -> Result<(Inventory, Inventory, Wallet, Wallet)> {
    let mut merchant = merchant.clone();
    let mut customer = customer.clone();
    let mut merchant_wallet = merchant_wallet.clone();
    let mut customer_wallet = customer_wallet.clone();
    // Withdraw both sides first so a full inventory can exchange its last slot.
    let purchases = quote
        .offer
        .purchases
        .iter()
        .map(|l| withdraw(&mut merchant, l.entry, l.quantity))
        .collect::<Result<Vec<_>>>()?;
    let sales = quote
        .offer
        .sales
        .iter()
        .map(|l| withdraw(&mut customer, l.entry, l.quantity))
        .collect::<Result<Vec<_>>>()?;
    for entry in purchases {
        deposit(catalog, &mut customer, entry)?;
    }
    for entry in sales {
        deposit(catalog, &mut merchant, entry)?;
    }
    // Settle the net price: selling goods can fund purchases in the same basket.
    if quote.purchase_total > quote.sale_total {
        transfer_money(
            &mut customer_wallet,
            &mut merchant_wallet,
            quote.purchase_total.checked_sub(quote.sale_total)?,
        )?;
    } else if quote.sale_total > quote.purchase_total {
        transfer_money(
            &mut merchant_wallet,
            &mut customer_wallet,
            quote.sale_total.checked_sub(quote.purchase_total)?,
        )?;
    }
    merchant.revision = next_revision(merchant.revision)?;
    customer.revision = next_revision(customer.revision)?;
    Ok((merchant, customer, merchant_wallet, customer_wallet))
}
