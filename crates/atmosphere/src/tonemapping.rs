//! The world's tone map: Khronos PBR Neutral with a toe that keeps hue. Bevy's toe subtracts the
//! same amount from every channel, which strips the smallest one from dark colours: shaded grass
//! and foliage lost their blue and read olive-yellow (blue/green 0.21 in the scene, 0.05 on
//! screen). `tonemapping.wesl` is Bevy's shader library with only that toe changed; it replaces
//! Bevy's embedded copy, so every path that tone maps (the temporal output pass and Bevy's own
//! node, game and editor) uses it.
use bevy::{asset::io::embedded::EmbeddedAssetRegistry, prelude::*};
use std::path::{Path, PathBuf};

const LIBRARY: &str = "bevy_core_pipeline/tonemapping.wesl";

pub(crate) fn plugin(app: &mut App) {
    // Apps without an asset server (unit tests) have nothing to tone map.
    let Some(registry) = app.world().get_resource::<EmbeddedAssetRegistry>() else {
        return;
    };
    registry.insert_asset(
        PathBuf::from(file!()).with_file_name("tonemapping.wesl"),
        Path::new(LIBRARY),
        include_str!("tonemapping.wesl").as_bytes(),
    );
    // Bevy queued its own copy's load when its plugin was built; load ours over it.
    if let Some(server) = app.world().get_resource::<AssetServer>() {
        server.reload(format!("embedded://{LIBRARY}"));
    }
}
