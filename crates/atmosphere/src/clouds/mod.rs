//! The ground-view cloud layer: its noise, the sun and moon transmittance map it casts and its
//! image on the sky. Its shape and light come from the environment parameters
//! (`crate::environment`).
mod noise;
mod render;
mod sky_cache;
use crate::environment::{EnvironmentAssets, EnvironmentParams};
use bevy::{
    prelude::*,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::*,
    },
};
pub(crate) use noise::generate as noise;
pub(crate) use render::{CloudTarget, Pipelines as CloudPipelines, refresh as refresh_clouds};

#[derive(Resource, Default)]
pub struct CloudClock {
    pub seconds: f64,
    /// Editor owns preview transport; the standalone game advances automatically.
    pub playing: bool,
}
/// Presentation quality, independent of the authored weather profile.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub enum CloudQuality {
    Off,
    #[default]
    Balanced,
    High,
}
impl CloudQuality {
    pub(super) fn target_size(self, full: UVec2) -> UVec2 {
        if self == Self::Balanced {
            return UVec2::new(sky_cache::WIDTH, sky_cache::HEIGHT);
        }
        let divisor = 2;
        UVec2::new(
            full.x.div_ceil(divisor).max(1),
            full.y.div_ceil(divisor).max(1),
        )
    }
}
/// A world view that shows clouds this frame.
#[derive(Component, Clone, bevy::render::extract_component::ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub struct CloudView;
pub struct CloudsPlugin;
impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CloudClock>()
            .init_resource::<CloudQuality>();
        // Pure ECS atmosphere tests/tools do not install GPU or asset services.
        if app.get_sub_app(bevy::render::RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractResourcePlugin::<CloudQuality>::default(),
            bevy::render::extract_component::ExtractComponentPlugin::<CloudView>::default(),
        ));
        render::install(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_sky_has_a_fixed_budget_and_high_handles_odd_or_tiny_views() {
        assert_eq!(
            CloudQuality::Balanced.target_size(UVec2::new(2592, 1456)),
            UVec2::new(2048, 256)
        );
        assert_eq!(
            CloudQuality::High.target_size(UVec2::new(2592, 1456)),
            UVec2::new(1296, 728)
        );
        assert_eq!(
            CloudQuality::Balanced.target_size(UVec2::new(5, 3)),
            UVec2::new(2048, 256)
        );
        assert_eq!(CloudQuality::High.target_size(UVec2::ONE), UVec2::ONE);
    }
}
