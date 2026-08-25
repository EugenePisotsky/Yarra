use bevy::{
    asset::AssetPlugin,
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use engine::MinimalGamePlugin;

fn main() {
    let asset_root = resolve_asset_root();
    let runtime_database = runtime_database_path(&asset_root);
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.055, 0.065, 0.075)))
        .add_plugins(
            DefaultPlugins
                .set(AssetPlugin {
                    file_path: asset_root.to_string_lossy().into_owned(),
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Yarra — SQLite World Streaming".into(),
                        resolution: WindowResolution::new(1280, 720),
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                }),
        )
        .add_plugins(MinimalGamePlugin::new(runtime_database))
        .run();
}

fn runtime_database_path(asset_root: &std::path::Path) -> std::path::PathBuf {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == "--world-db" {
            return arguments
                .next()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| panic!("--world-db requires a database path"));
        }
    }
    asset_root.join("generated/demo.runtime.sqlite")
}

fn resolve_asset_root() -> std::path::PathBuf {
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
    candidates.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"));

    candidates
        .into_iter()
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("assets"))
}
