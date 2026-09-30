//! One transaction owner for complete, synchronous RPG checkpoints.
mod working;
pub use working::{StateStats, StoredSession, WorkingStore};
mod slots;
mod storage;
pub use slots::{SaveDirectory, SaveInfo, SaveSlot};
use thiserror::Error;
pub use working::{APPLICATION_ID, SCHEMA_VERSION};
#[derive(Debug, Error)]
pub enum SaveError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Gameplay(#[from] gameplay::GameplayError),
    #[error(transparent)]
    Invalid(#[from] game_types::Invalid),
    #[error("unsupported save schema: {0}")]
    Schema(i64),
    #[error("file is not a Yarra save")]
    WrongDatabase,
    #[error("save requires different gameplay content or world publication")]
    ContentMismatch,
}
pub type Result<T> = std::result::Result<T, SaveError>;
