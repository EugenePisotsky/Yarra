//! Bevy/SQLite-free coordination. Only accepted commands replace live state.
mod content;
mod world;
mod world_runtime;
pub use world::*;
mod conversation;
pub use conversation::{ChoiceView, ConversationView, LineView};
mod narrative;
pub use narrative::*;
pub mod fixtures;
mod resolution;
mod runtime;
mod session;
mod state;
mod tool_content;
pub use tool_content::ToolContent;
mod driver;
pub use content::*;
pub use driver::*;
pub use runtime::*;
pub use session::*;
pub use state::*;
use thiserror::Error;
#[derive(Debug, Error)]
pub enum GameplayError {
    #[error("runtime I/O: {0}")]
    Runtime(String),
    #[error(transparent)]
    Invalid(#[from] game_types::Invalid),
    #[error(transparent)]
    Inventory(#[from] inventory::InventoryError),
}
pub type Result<T> = std::result::Result<T, GameplayError>;
