//! Bevy/SQLite-free gameplay: domain rules, the in-memory playthrough and its commands.
pub mod actors;
mod character;
pub use character::teachable;
mod content;
pub mod dialogue;
pub mod inventory;
pub mod quests;
pub mod rules;
mod world;
mod world_runtime;
pub use world::*;
mod conversation;
pub use conversation::{ChoiceView, ConversationView, LineView};
mod narrative;
pub use narrative::*;
pub mod fixtures;
mod runtime;
mod script;
pub use script::*;
mod session;
mod state;
mod tool_content;
mod tx;
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
    #[error(transparent)]
    Rejected(#[from] Rejection),
}
/// Why a command that made sense was not carried out: something a player runs into, as
/// opposed to a mistake in content or code. A UI can match on these and say it its own way.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Rejection {
    #[error("{0} is dead")]
    Dead(game_types::ActorId),
    #[error("{0} is not in the party")]
    NotInParty(game_types::ActorId),
    #[error("the party is full")]
    PartyFull,
    #[error("no attribute points to spend")]
    NoAttributePoints,
    #[error("{} cannot be raised further", .0.as_str())]
    StatAtMaximum(game_types::Key),
    #[error("{} is already mastered", .0.as_str())]
    SkillAtMaximum(game_types::Key),
    #[error("a {} cannot learn more {}", .class.as_str(), .skill.as_str())]
    ClassCannotLearn {
        class: game_types::Key,
        skill: game_types::Key,
    },
    #[error("{needed} learning points needed, {available} available")]
    NotEnoughLearningPoints { needed: u32, available: u32 },
    #[error("{needed} gold needed, {available} available")]
    NotEnoughGold { needed: u64, available: u64 },
}
pub type Result<T> = std::result::Result<T, GameplayError>;
