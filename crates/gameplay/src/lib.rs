//! Bevy/SQLite-free gameplay: domain rules, the in-memory playthrough and its commands.
pub mod actors;
pub mod dialogue;
pub mod inventory;
pub mod quests;
pub mod rules;

#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;

mod actions;
mod authored;
mod character;
mod combat;
mod commands;
mod conditions;
mod content;
mod conversation;
mod driver;
mod graphs;
mod keyed;
mod narrative;
mod runtime;
mod script;
mod session;
mod state;
mod tx;
mod world;
mod world_runtime;

pub use actions::Action;
pub use authored::{AuthoredContent, ToolContent};
pub use character::teachable;
pub use commands::{Command, CommandOutcome, GameEvent};
pub use conditions::{Condition, NamedPredicate, Participant, Test};

pub use content::{
    ContentManifest, GameContent, GameDefinitions, Value, VariableDefinition, VariableKey,
    VariableScope,
};

pub use conversation::{ChoiceView, ConversationView, LineView};
pub use driver::Driver;
pub use graphs::{Graphs, MAX_LOADED_DIALOGUES};
pub use keyed::{Keyed, KeyedMap, keyed_map};

pub use narrative::{
    DialogueVariant, EntryPreview, EntryRule, InteractionPreview, InteractionProfile,
};

pub use runtime::{ContentIdentity, ContentSource, ConversationKey, SessionHeader};
pub use script::{ActScope, ReadScope, ScriptEngine, ScriptModule, ScriptName, Scripts};
pub use session::GameSession;
pub use state::{Floor, ModifierSource, Party, SessionState};

pub use world::{
    LocationState, Movement, ObjectDefinition, ObjectKind, ObjectState, Pending, TriggerDefinition,
    TriggerIndex, TriggerRepeat, TriggerState, WorldCommand, WorldDefinitions, WorldEvent,
    WorldSignal, WorldState,
};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GameplayError {
    #[error("runtime I/O: {0}")]
    Runtime(String),
    #[error(transparent)]
    Invalid(#[from] game_types::Invalid),
    #[error(transparent)]
    Inventory(inventory::InventoryError),
    #[error(transparent)]
    Rejected(#[from] Rejection),
    /// A rule refused something a script asked for.
    #[error("script {script}: {rejection}")]
    RejectedInScript {
        script: String,
        rejection: Rejection,
    },
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
    #[error("{} is not known", .0.as_str())]
    AbilityNotKnown(game_types::Key),
    #[error("{} needs a target", .0.as_str())]
    TargetRequired(game_types::Key),
    #[error("{} has no target", .0.as_str())]
    NoTarget(game_types::Key),
    #[error("too much is lined up already")]
    QueueFull,
    #[error("not enough {}", .0.as_str())]
    NotEnough(game_types::Key),
    #[error("{0} is locked")]
    Locked(game_types::ObjectId),
    #[error("{0} is closed")]
    Closed(game_types::ObjectId),
    #[error("{0} is destroyed")]
    Destroyed(game_types::ObjectId),
    #[error("the item cannot be used")]
    NotUsable,
    #[error("the item cannot be equipped")]
    NotEquippable,
    #[error("take the item off first")]
    Equipped,
    #[error("nothing is in the {} slot", .0.as_str())]
    SlotEmpty(game_types::Key),
    #[error("{missing} more of {definition} needed")]
    NotEnoughItems {
        definition: game_types::ItemDefinitionId,
        missing: u32,
    },
    #[error("{0} is the character being steered")]
    Controlled(game_types::ActorId),
    #[error("{0} has nothing to say")]
    NothingToSay(game_types::ActorId),
    #[error("already in this conversation")]
    AlreadyTalking,
    #[error("the conversation has moved on")]
    ConversationMoved,
    #[error("{} cannot be said now", .0.as_str())]
    ChoiceUnavailable(game_types::Key),
    #[error("there is no room for more")]
    NoRoom,
    #[error("the item cannot be given away, sold or dropped")]
    ItemRestricted,
    #[error("the item is not traded here")]
    NotForTrade,
    #[error("the prices have changed since the offer was made")]
    QuoteStale,
}

/// What a player can run into in an inventory becomes a `Rejection`; the rest are mistakes.
impl From<inventory::InventoryError> for GameplayError {
    fn from(error: inventory::InventoryError) -> Self {
        use inventory::InventoryError as E;
        let rejection = match error {
            E::Capacity => Rejection::NoRoom,
            E::Restricted => Rejection::ItemRestricted,
            E::InsufficientFunds { needed, available } => {
                Rejection::NotEnoughGold { needed, available }
            }
            E::NotForTrade => Rejection::NotForTrade,
            E::StaleQuote => Rejection::QuoteStale,
            error => return Self::Inventory(error),
        };
        Self::Rejected(rejection)
    }
}

impl GameplayError {
    /// The reason, when the command was refused by a rule rather than by a mistake.
    pub fn rejection(&self) -> Option<&Rejection> {
        match self {
            Self::Rejected(rejection) | Self::RejectedInScript { rejection, .. } => Some(rejection),
            _ => None,
        }
    }
}

impl Rejection {
    pub(crate) fn unless(self, holds: bool) -> Result<()> {
        if holds { Ok(()) } else { Err(self.into()) }
    }
}

pub type Result<T> = std::result::Result<T, GameplayError>;
