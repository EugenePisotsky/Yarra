//! Platform-neutral image upscaling. Backend SDKs are optional implementation details.
mod backends;
mod capabilities;
#[cfg(test)]
mod gpu_tests;
mod output;
mod render;
pub mod temporal;
#[cfg(all(test, feature = "metalfx", any(target_os = "macos", target_os = "ios")))]
mod temporal_motion_tests;
#[cfg(test)]
mod tests;

use bevy::{
    prelude::*,
    render::extract_component::{ExtractComponent, ExtractComponentPlugin},
};
pub use capabilities::*;
pub use output::DirectTonemapOutput;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UpscaleMethod {
    #[default]
    Auto,
    Linear,
    MetalFxSpatial,
    MetalFxTemporal,
}
impl UpscaleMethod {
    pub const ALL: &[Self] = &[
        Self::Auto,
        Self::Linear,
        Self::MetalFxSpatial,
        Self::MetalFxTemporal,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Linear => "Linear",
            Self::MetalFxSpatial => "MetalFX Spatial",
            Self::MetalFxTemporal => "MetalFX Temporal",
        }
    }
}

/// Attach to a Camera3d whose resolved, tone-mapped sRGB RGBA8/BGRA8 output is `input`.
/// `output` must be a distinct display-sized image created with `output_image`.
/// Render UI in a later camera sampling `output`, keeping UI at native resolution.
/// When `TemporalView` is attached, this spatial pass is bypassed; the camera writes
/// its native-sized LDR target after temporal HDR reconstruction.
#[derive(Component, Clone, ExtractComponent, PartialEq)]
#[require(UpscaleStatus)]
pub struct UpscaleView {
    pub input: Handle<Image>,
    pub output: Handle<Image>,
    pub method: UpscaleMethod,
    pub enabled: bool,
}

#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct UpscaleStatus {
    pub requested: UpscaleMethod,
    pub active: Option<UpscaleMethod>,
    pub reason: Option<String>,
    pub input_size: UVec2,
    pub output_size: UVec2,
}
impl Default for UpscaleStatus {
    fn default() -> Self {
        Self {
            requested: default(),
            active: None,
            reason: Some("Waiting for render resources".into()),
            input_size: UVec2::ZERO,
            output_size: UVec2::ZERO,
        }
    }
}
impl UpscaleStatus {
    pub fn description(&self) -> String {
        format!(
            "Upscaler: {} -> {}{}",
            self.requested.label(),
            self.active.map_or("inactive", UpscaleMethod::label),
            self.reason
                .as_ref()
                .map_or(String::new(), |s| format!(" ({s})"))
        )
    }
}

/// RGBA8 storage with an sRGB sampling/render view: native upscalers can write
/// perceptual output while normal Bevy UI reads linear colour. No CPU pixel copies.
pub fn output_image(size: UVec2) -> Image {
    use bevy::{image::ImageSampler, render::render_resource::*};
    let mut image = Image::new_target_texture(
        size.x.max(1),
        size.y.max(1),
        TextureFormat::Rgba8Unorm,
        None,
    );
    image.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
    image.texture_descriptor.view_formats = &[TextureFormat::Rgba8UnormSrgb];
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        format: Some(TextureFormat::Rgba8UnormSrgb),
        ..default()
    });
    image.sampler = ImageSampler::linear();
    image
}

#[derive(Default)]
struct Messages {
    statuses: HashMap<Entity, UpscaleStatus>,
    capabilities: UpscalingCapabilities,
}
#[derive(Resource, Clone, Default)]
struct Bridge(Arc<Mutex<Messages>>);

pub struct UpscalingPlugin;
impl Plugin for UpscalingPlugin {
    fn build(&self, app: &mut App) {
        let bridge = Bridge::default();
        app.insert_resource(bridge.clone())
            .init_resource::<UpscalingCapabilities>()
            .add_plugins(ExtractComponentPlugin::<UpscaleView>::default())
            .add_systems(First, receive);
        render::install(app, bridge);
        temporal::install(app);
        output::install(app);
    }
    fn finish(&self, app: &mut App) {
        output::finish(app);
    }
}
fn receive(
    bridge: Res<Bridge>,
    mut capabilities: ResMut<UpscalingCapabilities>,
    mut views: Query<(Entity, &UpscaleView, &mut UpscaleStatus)>,
) {
    let Ok(mut messages) = bridge.0.lock() else {
        return;
    };
    if *capabilities != messages.capabilities {
        *capabilities = messages.capabilities.clone();
    }
    for (entity, view, mut status) in &mut views {
        if let Some(value) = messages.statuses.remove(&entity) {
            // A delayed render result must not label a newly requested backend as active.
            if value.requested == view.method {
                status.set_if_neq(value);
            }
        }
    }
    messages.statuses.clear();
}
