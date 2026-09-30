//! Authored RON/Fluent projects and immutable SQLite content bundles.
//! Domain crates never depend on this filesystem/composition layer.
use gameplay::inventory;
mod asset;
mod bundle;
mod languages;
mod materialize;
pub use languages::{LanguageRepository, LanguageSource, LanguageStats};
mod project;
mod repository;
mod runtime;
mod scenario;
mod script;
pub use asset::{Asset, AssetHeader, AssetId, AssetKind, MAX_ASSET_BYTES};
pub use bundle::{BUNDLE_APPLICATION_ID, BUNDLE_SCHEMA_VERSION, BundleManifest};
pub use project::{
    ConversationFile, LoadedProject, LocaleFile, PackageFile, ProjectFile, ResourceFile,
    SOURCE_FORMAT_VERSION, TranslationReview,
};
pub use repository::ContentRepository;
pub use runtime::{ContentLibrary, RuntimeSession, ToolSession};
pub use scenario::{ActorSpawn, InventorySeed, ItemAmount, Scenario, WalletSeed};
pub use script::Step;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("{path}: {message}")]
    Source { path: PathBuf, message: String },
    #[error("{context}: {message}")]
    Validation { context: String, message: String },
    #[error(transparent)]
    Gameplay(#[from] gameplay::GameplayError),
    #[error(transparent)]
    Invalid(#[from] game_types::Invalid),
    #[error(transparent)]
    Inventory(#[from] inventory::InventoryError),
    #[error(transparent)]
    Localization(#[from] localization::LocalizationError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Save(#[from] save::SaveError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("unsupported source format {0}")]
    SourceVersion(u32),
    #[error("unsupported content bundle schema {0}")]
    BundleVersion(i64),
    #[error("file is not a Yarra gameplay content bundle")]
    WrongDatabase,
    #[error("asset not found: {0:?}")]
    MissingAsset(AssetId),
    #[error("corrupt asset {id:?}: {message}")]
    CorruptAsset { id: AssetId, message: String },
}
pub type Result<T> = std::result::Result<T, ContentError>;
pub(crate) fn contextual<T, E: std::fmt::Display>(
    context: impl Into<String>,
    result: std::result::Result<T, E>,
) -> Result<T> {
    result.map_err(|error| ContentError::Validation {
        context: context.into(),
        message: error.to_string(),
    })
}
