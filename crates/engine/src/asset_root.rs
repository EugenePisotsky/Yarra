//! Where the game and the editor find their assets.
use std::path::PathBuf;

/// The `assets` folder: under the working directory, beside the executable or in a macOS
/// bundle's Resources, else the source tree's (a binary run from elsewhere while developing).
pub fn resolve_asset_root() -> PathBuf {
    let mut candidates = Vec::new();

    if let Ok(current_directory) = std::env::current_dir() {
        candidates.push(current_directory.join("assets"));
    }

    if let Ok(executable) = std::env::current_exe()
        && let Some(executable_directory) = executable.parent()
    {
        candidates.push(executable_directory.join("assets"));
        candidates.push(executable_directory.join("../Resources/assets"));
    }

    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"));

    candidates
        .into_iter()
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| PathBuf::from("assets"))
}
