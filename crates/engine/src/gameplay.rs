//! Default gameplay composition with independently optional native input, camera and marker.
pub(crate) mod actors;
mod camera;
mod input;
mod target;
pub use actors::{PlayerMovementSuspended, PlayerRoute, standing_character};
pub(crate) use camera::CAMERA_FOCUS_HEIGHT;
pub use camera::GameCameraPlugin;
pub use input::GameInputPlugin;
pub use target::MovementTargetPlugin;

use crate::{
    MsaaColorStorePlugin, WorldEnvironmentPlugin, WorldStartView, WorldStreamingPlugin,
    WorldStreamingSystems,
    actor::advance_character_motors,
    character::{CharacterPresentationPlugin, CharacterPresentationResolveSet},
};
use bevy::{app::PluginGroupBuilder, prelude::*};
use std::path::PathBuf;

/// Full game scene convenience composition. The application owns the window/output and
/// separately installs WorldVegetationPlugin. Custom applications can install these plugins
/// directly and configure GameplayPlugins with PluginGroupBuilder::disable.
pub struct MinimalGamePlugin {
    runtime_database: PathBuf,
}
impl MinimalGamePlugin {
    pub fn new(runtime_database: impl Into<PathBuf>) -> Self {
        Self {
            runtime_database: runtime_database.into(),
        }
    }
}
impl Plugin for MinimalGamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MsaaColorStorePlugin,
            WorldEnvironmentPlugin::game(),
            terrain_render::TerrainRenderPlugin,
            WorldStreamingPlugin::game(self.runtime_database.clone()),
        ))
        .add_plugins(GameplayPlugins);
    }
}

/// Normal gameplay defaults. Keep GameplayPlugin; input, camera and destination marker
/// can each be omitted at startup. This is composition, not runtime teardown/resume.
pub struct GameplayPlugins;
impl PluginGroup for GameplayPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(GameplayPlugin)
            .add(GameCameraPlugin)
            .add(GameInputPlugin)
            .add(MovementTargetPlugin)
    }
}

/// Actor creation, presentation, locomotion, grounding and shared schedule/control resources.
/// Requires world streaming/contact resources, Time and character assets supplied by DefaultPlugins.
/// Does not spawn a camera, read native input or create the destination marker.
pub struct GameplayPlugin;
impl Plugin for GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CharacterPresentationPlugin)
            .init_resource::<WorldStartView>()
            .add_message::<crate::WorldStartAdopted>()
            .init_resource::<GameInputEnabled>()
            .init_resource::<GamePointerInputBlocked>()
            .init_resource::<PlayerMovementSpeed>()
            .init_resource::<PlayerMovementSuspended>()
            .configure_sets(
                Update,
                (
                    GameplaySystems::CameraInput,
                    GameplaySystems::PointerInput,
                    GameplaySystems::MoveIntent,
                    GameplaySystems::Movement,
                    GameplaySystems::Grounding,
                    GameplaySystems::TargetIndicator,
                    GameplaySystems::CameraFollow,
                )
                    .chain()
                    .after(WorldStreamingSystems),
            )
            .configure_sets(
                Update,
                GameplaySystems::Movement.after(CharacterPresentationResolveSet),
            )
            .add_systems(Startup, actors::spawn_player)
            .add_systems(
                Update,
                (
                    advance_character_motors
                        .in_set(GameplaySystems::Movement)
                        .run_if(resource_equals(GameInputEnabled(true))),
                    actors::move_player_to_adopted_start.in_set(GameplaySystems::MoveIntent),
                    // After input, which clears the intent when no key is held.
                    actors::steer_player_along_route
                        .after(GameplaySystems::MoveIntent)
                        .before(GameplaySystems::Movement),
                    actors::hold_suspended_player
                        .after(actors::steer_player_along_route)
                        .before(GameplaySystems::Movement),
                    actors::ground_characters_to_streamed_terrain
                        .in_set(GameplaySystems::Grounding),
                ),
            );
    }
}

/// Ordered gameplay stages in Update, following world transitions/rebasing/source attachment.
/// UI routing/settings run before CameraInput; scripted camera overrides run after CameraFollow.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameplaySystems {
    CameraInput,
    PointerInput,
    MoveIntent,
    Movement,
    Grounding,
    TargetIndicator,
    CameraFollow,
}

/// Enables gameplay controls and actor movement. Profiling tools may explicitly freeze them.
/// Grounding and camera follow continue; native gesture readers drain events while disabled.
#[derive(Resource, Clone, Copy, PartialEq, Eq)]
pub struct GameInputEnabled(pub bool);
impl Default for GameInputEnabled {
    fn default() -> Self {
        Self(true)
    }
}

/// UI pointer capture does not suppress keyboard or gamepad input.
#[derive(Resource, Default)]
pub struct GamePointerInputBlocked(pub bool);

/// Temporary exploration speed for the controlled actor. Terrain contact still applies.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayerMovementSpeed {
    #[default]
    Normal,
    Fast,
}

impl PlayerMovementSpeed {
    pub fn multiplier(self) -> f32 {
        match self {
            Self::Normal => 1.0,
            Self::Fast => 10.0,
        }
    }
}

#[cfg(test)]
mod tests;
