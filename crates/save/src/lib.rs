//! Named saves. Each slot is one file holding the whole playthrough state, written whole and
//! published atomically, so a failed save never damages the slot it replaces.
mod slots;
pub use slots::{SAVE_FORMAT, SaveDirectory, SaveInfo, SaveSlot};
use thiserror::Error;
#[derive(Debug, Error)]
pub enum SaveError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Gameplay(#[from] gameplay::GameplayError),
    #[error(transparent)]
    Invalid(#[from] game_types::Invalid),
    #[error("unsupported save format: {0}")]
    Format(u32),
    #[error("file is not a Yarra save")]
    NotASave,
    #[error("save requires different gameplay content or world publication")]
    ContentMismatch,
}
pub type Result<T> = std::result::Result<T, SaveError>;
