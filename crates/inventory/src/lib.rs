//! Standalone item catalogs, inventories and fixed-price trading.
//!
//! No ECS, database, UI or combat dependencies. Gameplay authorizes access before
//! invoking commands. Public records are import/export DTOs; validate imported
//! data before use. All commands validate their inputs and leave them unchanged
//! on error. Persistence must commit all affected records together.
mod catalog;
pub mod fixtures;
mod items;
mod trade;
mod types;

pub use catalog::*;
pub use items::*;
pub use trade::*;
pub use types::*;
