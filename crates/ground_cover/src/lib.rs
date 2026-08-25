mod renderer;

use bevy::{
    asset::Asset,
    prelude::*,
    reflect::TypePath,
    render::{
        extract_component::ExtractComponent,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
    },
};
use world::{GroundCoverPage, GroundCoverSpecies, PageKey};

/// Installs the ground-cover asset and rendering systems.
///
/// Streaming only hands this plugin page assets. GPU extraction, visibility,
/// LOD, and drawing remain private so grass, flowers, and other decorative
/// fields can evolve without coupling the world streamer to their renderer.
pub struct GroundCoverPlugin;

impl Plugin for GroundCoverPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<GroundCoverPageAsset>()
            .init_resource::<GroundCoverWind>()
            .init_resource::<GroundCoverDebug>()
            .add_plugins((
                ExtractResourcePlugin::<GroundCoverWind>::default(),
                ExtractResourcePlugin::<GroundCoverDebug>::default(),
            ))
            .add_systems(
                Update,
                (advance_ground_cover_wind, cycle_ground_cover_debug),
            )
            .add_plugins(renderer::GroundCoverRenderPlugin);
    }
}

/// One coherent wind field shared by every ground-cover species.
///
/// These are authoring-facing controls: species decide their maximum response,
/// while this resource describes the current world's direction and motion.
#[derive(Resource, ExtractResource, Debug, Clone)]
pub struct GroundCoverWind {
    pub direction: Vec2,
    pub base_strength: f32,
    pub gust_strength: f32,
    pub spatial_scale: f32,
    pub speed: f32,
    pub elapsed_seconds: f32,
}

impl Default for GroundCoverWind {
    fn default() -> Self {
        Self {
            direction: Vec2::new(0.88, 0.47).normalize(),
            base_strength: 0.35,
            gust_strength: 0.65,
            spatial_scale: 0.12,
            speed: 1.15,
            elapsed_seconds: 0.0,
        }
    }
}

fn advance_ground_cover_wind(time: Res<Time>, mut wind: ResMut<GroundCoverWind>) {
    wind.elapsed_seconds = (wind.elapsed_seconds + time.delta_secs()) % 4096.0;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroundCoverDebugMode {
    #[default]
    Normal,
    LodColors,
    FarOnly,
    FarDisabled,
}

impl GroundCoverDebugMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::LodColors => "LOD colors",
            Self::FarOnly => "far only",
            Self::FarDisabled => "far disabled",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Normal => Self::LodColors,
            Self::LodColors => Self::FarOnly,
            Self::FarOnly => Self::FarDisabled,
            Self::FarDisabled => Self::Normal,
        }
    }

    pub(crate) fn gpu_value(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::LodColors => 1,
            Self::FarOnly => 2,
            Self::FarDisabled => 3,
        }
    }
}

#[derive(Resource, ExtractResource, Debug, Clone, Copy, Default)]
pub struct GroundCoverDebug {
    pub mode: GroundCoverDebugMode,
}

fn cycle_ground_cover_debug(
    keys: Res<ButtonInput<KeyCode>>,
    mut ground_cover_debug: ResMut<GroundCoverDebug>,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    ground_cover_debug.mode = ground_cover_debug.mode.next();
    warn!(
        "ground-cover debug mode: {}",
        ground_cover_debug.mode.label()
    );
}

/// A resolved runtime page ready for renderer upload.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct GroundCoverPageAsset {
    pub key: PageKey,
    pub cell_size: f32,
    pub page: GroundCoverPage,
    pub species: Vec<GroundCoverSpecies>,
}

/// Main-world attachment for one resident ground-cover page.
#[derive(Component, Debug, Clone, Deref, DerefMut)]
pub struct GroundCoverPage3d(pub Handle<GroundCoverPageAsset>);

/// Marks a camera as a view that should render ground cover and carries its resolved zoom.
#[derive(Component, ExtractComponent, Debug, Clone, Copy, Default)]
pub struct GroundCoverView {
    /// Zero is the closest third-person view and one is the farthest overhead view.
    pub normalized_zoom: f32,
}
