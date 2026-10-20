//! What the game carries for its developers: the F1 performance panel with its timing probes
//! (`render_audit`), timed profiles, scripted camera routes, look and LOD-lab captures, Metal
//! captures and live grass looks. Each installs only when a launch option asks for it, except
//! F1, which `--diagnostics` composes.
pub(crate) mod grass_pack;
pub(crate) mod lod_lab;
pub(crate) mod look_capture;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod metal_capture;

pub(crate) mod profile;
pub(crate) mod render_audit;
pub(crate) mod repro;

use bevy::prelude::*;
use std::path::Path;

/// Installs the developer tools after the game's own settings. Precedence: launch defaults, a
/// reproduction, then the profile's presentation.
pub(crate) fn install(app: &mut App, asset_root: &Path) -> Result<(), String> {
    app.add_plugins(grass_pack::GrassPackPlugin {
        path: asset_root.join(grass_pack::PACK),
    });
    profile::install(app);
    repro::install(app);
    profile::apply_runtime_settings(app);
    lod_lab::install(app)?;
    look_capture::install(app);
    render_audit::install(app);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    metal_capture::install(app);
    Ok(())
}
