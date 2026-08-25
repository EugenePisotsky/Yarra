use bevy::{
    prelude::*,
    window::{PresentMode, WindowResolution},
};
use engine::MinimalGamePlugin;

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.055, 0.065, 0.075)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Yarra — Minimal Baseline".into(),
                resolution: WindowResolution::new(1280, 720),
                present_mode: PresentMode::AutoNoVsync,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(MinimalGamePlugin)
        .run();
}
