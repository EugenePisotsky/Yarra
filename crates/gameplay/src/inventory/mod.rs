//! Item catalogs, inventories, wallets and fixed-price trading.
//!
//! Operations check what they are given and change nothing when they fail. The session
//! authorizes access, and runs them inside a command, which undoes them with everything else
//! when the command fails. An inventory is checked in full when a playthrough is loaded.
mod catalog;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;

mod items;
mod loot;
mod trade;
mod types;

pub use catalog::*;
pub use items::*;
pub use loot::*;
pub use trade::*;
pub use types::*;
