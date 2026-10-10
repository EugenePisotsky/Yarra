//! The environment the sky, clouds, air, sea and every lit surface share: one parameter buffer
//! (`EnvironmentParams`, `shaders/environment/params.wesl`) written from the atmosphere state each
//! frame, and its maps (cloud shadow, rain shelter, forest shadow, valley mist, shore).
mod material;
mod publish;
mod surface;
mod sync;
use crate::ApplyAtmosphere;
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_resource::*,
        storage::ShaderBuffer,
    },
};
use bytemuck::{Pod, Zeroable};
pub use material::{
    EnvironmentExtension, EnvironmentMaterial, EnvironmentMaterialOptIn, EnvironmentMaterialSystems,
};
use publish::ForestSkyLevels;
pub use surface::{EnvironmentSurfaceGpu, surface_layout};

/// The current world cell's origin, which the environment's world-anchored patterns (cloud field,
/// mist noise, shore and mist maps, surface patterns) are wrapped around.
#[derive(Resource, Default)]
pub struct EnvironmentOrigin(pub [f64; 2]);
/// The environment's GPU inputs: the parameter buffer and the maps it places.
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct EnvironmentAssets {
    pub parameters: Handle<ShaderBuffer>,
    pub shadows: Handle<Image>,
    pub noise: Handle<Image>,
    /// Top-down rain shelter around the camera; see `crate::shelter`.
    pub shelter: Handle<Image>,
    /// Crowns around the camera for distant forest shadows; see `crate::forest_shadow`.
    pub forest_shadow: Handle<Image>,
    /// Where mist pools over the world; see `crate::valley_mist`.
    pub mist: Handle<Image>,
    /// How waves come ashore; see `crate::shore`.
    pub shore: Handle<Image>,
}
/// Everything the environment's passes and lit materials read, in one storage buffer
/// (`shaders/environment/params.wesl`).
#[derive(Resource, Clone, Copy, Default, ExtractResource, Pod, Zeroable)]
#[extract_app(bevy::render::RenderApp)]
#[repr(C)]
pub(crate) struct EnvironmentParams {
    pub layer: [f32; 4],
    pub shape: [f32; 4],
    pub offset: [f32; 4],
    pub sun: [f32; 4],
    pub moon: [f32; 4],
    pub sun_color: [f32; 4],
    pub moon_color: [f32; 4],
    pub ambient: [f32; 4],
    pub haze: [f32; 4],
    /// Weather fog: unexposed in-scattered radiance, extra extinction per metre.
    pub fog: [f32; 4],
    /// Previous coverage, extinction and erosion, and linear change progress. Each region of
    /// the field blends from these to `shape` at its own time within the change.
    pub transition: [f32; 4],
    /// Game weather for surfaces and precipitation: wetness, precipitation intensity; and how
    /// closed the cloud deck is (0 open, 1 overcast).
    pub weather: [f32; 4],
    /// Rain shelter map: origin XZ, metres per texel, enabled.
    pub shelter: [f32; 4],
    /// Forest shadow map: origin XZ, metres per texel (0 when off), tallest crown top.
    pub forest_shadow: [f32; 4],
    /// Sky light under the crowns: x the share they hold back (0 off, 1 as their foliage does).
    pub forest_sky: [f32; 4],
    /// Ground haze: extinction per metre at its base and below, base height, height over which
    /// it thins by e (`world::fog::FogSettings`).
    pub low_haze: [f32; 4],
    /// Valley mist: extinction per metre in full mist (0 off), depth above a valley floor.
    pub mist: [f32; 4],
    /// Mist map (`crate::valley_mist`): first corner XZ in render coordinates, metres per
    /// texel, 1 when published.
    pub mist_map: [f32; 4],
    /// World-anchored mist noise offset XZ in metres, drifting with the wind.
    pub mist_drift: [f32; 4],
    /// Light the haze and mist scatter towards the eye, unexposed: rgb sky and moon light
    /// scattered evenly; `air_sun` rgb the sunlight that reaches them, scattered mostly forwards.
    pub air_light: [f32; 4],
    pub air_sun: [f32; 4],
    /// Light shafts: x extinction per metre of the air under crowns (0 off).
    pub shafts: [f32; 4],
    /// Unexposed sunlight at the camera after the atmosphere, for particles and drops; `sun`
    /// and `sun_color` hold it at the cloud layer.
    pub near_sun: [f32; 4],
    /// Open sea: level in render space, wind direction XZ, wave clock in seconds (wrapping
    /// every `WAVE_PERIOD`).
    pub ocean: [f32; 4],
    /// x: 1 with a sea to shade; y: wind strength scale of the waves.
    pub ocean_waves: [f32; 4],
    /// Shore map (`crate::shore`): first corner XZ in render coordinates, metres per texel and
    /// the swell's period (0 without a map).
    pub shore_map: [f32; 4],
    /// Lightning: where the channel leaves the cloud base (render space) and the flash's
    /// unexposed light on the clouds near it (0 without a strike).
    pub lightning: [f32; 4],
    /// x: the channel's brightness 0..1.
    pub lightning_channel: [f32; 4],
    /// The moon's disc: direction to it and angular radius (0: not drawn).
    pub moon_disc: [f32; 4],
    /// The moon's north, square to the direction to it, and the share of a fully lit face's
    /// light that earthshine gives its dark side.
    pub moon_frame: [f32; 4],
    /// Direction to the sun that lights the moon, wherever the sun is, and the moon's drawn
    /// face's light as a share of its light's: its glitter on the sea is the face's image.
    pub moon_sunward: [f32; 4],
    /// Unexposed light of white ground on the moon with the sun straight above it.
    pub moon_face: [f32; 4],
    /// Unexposed light of the moonless night sky just above the horizon: airglow and
    /// starlight, so the sky never turns black.
    pub night_sky: [f32; 4],
    /// The channel's segments, ends in pairs (xyz, width): `crate::lightning::channel`.
    pub lightning_segments: [[f32; 4]; 2 * crate::lightning::SEGMENTS],
}
/// Seconds after which the wave clock wraps; every wave completes whole cycles in it
/// (`shaders/water/waves.wesl`).
pub(crate) const WAVE_PERIOD: f64 = 3600.;
/// Mist noise tile, metres: a whole number, which the mist's shaders read as
/// `constants::MIST_NOISE_PERIOD` (`shaders/sky/air.wesl`).
pub(crate) const MIST_NOISE_PERIOD: f64 = 2048.;
pub(crate) fn mist_noise_period_def() -> bevy::shader::ShaderDefVal {
    bevy::shader::ShaderDefVal::UInt("MIST_NOISE_PERIOD".into(), MIST_NOISE_PERIOD as u32)
}
pub(crate) struct EnvironmentPlugin;
impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<crate::shelter::RainShelter>()
            .init_resource::<crate::forest_shadow::ForestShadow>()
            .init_resource::<crate::forest_shadow::ForestSkyOcclusion>()
            .init_resource::<crate::valley_mist::ValleyMist>()
            .init_resource::<crate::shore::Shore>()
            .init_resource::<EnvironmentOrigin>()
            .init_resource::<EnvironmentParams>()
            .init_resource::<ForestSkyLevels>();
        // Pure ECS atmosphere tests/tools do not install GPU or asset services.
        if app.get_sub_app(bevy::render::RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractResourcePlugin::<EnvironmentAssets>::default(),
            ExtractResourcePlugin::<EnvironmentParams>::default(),
            ExtractResourcePlugin::<ForestSkyLevels>::default(),
            material::EnvironmentMaterialPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(
            PostUpdate,
            (
                sync::sync,
                publish::publish::<crate::shelter::RainShelter>,
                publish::publish_forest_shadow,
                publish::publish::<crate::valley_mist::ValleyMist>,
                publish::publish::<crate::shore::Shore>,
            )
                .after(ApplyAtmosphere),
        );
        surface::install(app);
    }
}
fn repeat() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    })
}
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let mut noise = Image::new(
        Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 64,
        },
        TextureDimension::D3,
        crate::clouds::noise(64),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    noise.sampler = repeat();
    let mut shadows = Image::new_fill(
        Extent3d {
            width: 256,
            height: 256,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 255],
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    shadows.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
    shadows.sampler = repeat();
    commands.insert_resource(EnvironmentAssets {
        noise: images.add(noise),
        shadows: images.add(shadows),
        shelter: images.add(crate::shelter::RainShelter::image()),
        forest_shadow: images.add(crate::forest_shadow::ForestShadow::image()),
        mist: images.add(crate::valley_mist::ValleyMist::image()),
        shore: images.add(crate::shore::Shore::image()),
        parameters: buffers.add(ShaderBuffer::new(
            vec![EnvironmentParams::default()],
            RenderAssetUsages::RENDER_WORLD,
        )),
    });
}
/// Stable zero-density binding for isolated materials and renderer tests.
pub fn fallback_parameters() -> Handle<ShaderBuffer> {
    bevy::asset::uuid_handle!("59e29465-cc65-47d8-92a0-fc10a8e13a20")
}
pub fn init_fallback(mut buffers: ResMut<Assets<ShaderBuffer>>) {
    buffers
        .insert(
            fallback_parameters().id(),
            ShaderBuffer::new(
                vec![EnvironmentParams::default()],
                RenderAssetUsages::RENDER_WORLD,
            ),
        )
        .unwrap();
}
