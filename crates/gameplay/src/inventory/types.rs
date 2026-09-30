use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use game_types::{
    ActorId, CatalogId, CategoryId, InventoryId, ItemDefinitionId, ItemId, OwnerId, OwnerKind,
    OwnerRef, WalletId,
};

/// Integer units of the one configured currency, within SQLite's integer range.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct Money(u64);
impl Money {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(i64::MAX as u64);
    pub fn new(units: u64) -> Result<Self> {
        if units > Self::MAX.0 {
            return Err(InventoryError::Overflow);
        }
        Ok(Self(units))
    }
    pub fn units(self) -> u64 {
        self.0
    }
    pub fn checked_add(self, other: Self) -> Result<Self> {
        Self::new(
            self.0
                .checked_add(other.0)
                .ok_or(InventoryError::Overflow)?,
        )
    }
    /// What is left after paying `other` out of this.
    pub fn checked_sub(self, other: Self) -> Result<Self> {
        let left = self.0.checked_sub(other.0);
        Ok(Self(left.ok_or(InventoryError::InsufficientFunds {
            needed: other.0,
            available: self.0,
        })?))
    }
    pub fn checked_mul(self, quantity: u32) -> Result<Self> {
        Self::new(
            self.0
                .checked_mul(u64::from(quantity))
                .ok_or(InventoryError::Overflow)?,
        )
    }
}
impl TryFrom<u64> for Money {
    type Error = InventoryError;
    fn try_from(value: u64) -> Result<Self> {
        Self::new(value)
    }
}
impl From<Money> for u64 {
    fn from(value: Money) -> Self {
        value.0
    }
}

pub type Result<T> = std::result::Result<T, InventoryError>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InventoryError {
    #[error(transparent)]
    Contract(#[from] game_types::Invalid),
    #[error("invalid inventory data: {0}")]
    Invalid(String),
    #[error("duplicate identity or key")]
    Duplicate,
    #[error("item definition not found: {0}")]
    UnknownDefinition(ItemDefinitionId),
    #[error("category not found: {0}")]
    UnknownCategory(CategoryId),
    #[error("category is still used by item definitions: {0}")]
    CategoryInUse(CategoryId),
    #[error("item entry not found: {0}")]
    UnknownItem(ItemId),
    #[error("quantity must be positive and available")]
    InvalidQuantity,
    #[error("inventory entry limit exceeded")]
    Capacity,
    #[error("item policy forbids this operation")]
    Restricted,
    #[error("these items cannot be stacked")]
    IncompatibleStack,
    #[error("source and destination must differ")]
    SameParticipant,
    #[error("{needed} needed, {available} available")]
    InsufficientFunds { needed: u64, available: u64 },
    #[error("price is unavailable for this trade direction")]
    NotForTrade,
    #[error("trade quote is stale")]
    StaleQuote,
    #[error("numeric range exceeded")]
    Overflow,
}

pub(crate) fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 96
        || !key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_/-".contains(&b))
    {
        return Err(InventoryError::Invalid(format!(
            "invalid semantic key: {key:?}"
        )));
    }
    Ok(())
}
pub(crate) fn validate_text(text: &str, max: usize) -> Result<()> {
    if text.trim().is_empty() || text.len() > max {
        return Err(InventoryError::Invalid("empty or oversized text".into()));
    }
    Ok(())
}
pub(crate) fn next_revision(revision: u64) -> Result<u64> {
    if revision == 0 || revision >= i64::MAX as u64 {
        return Err(InventoryError::Overflow);
    }
    Ok(revision + 1)
}
pub(crate) fn validate_revision(revision: u64) -> Result<()> {
    if revision == 0 || revision > i64::MAX as u64 {
        return Err(InventoryError::Invalid("invalid revision".into()));
    }
    Ok(())
}
