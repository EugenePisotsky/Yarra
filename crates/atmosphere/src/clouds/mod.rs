//! Ground-view cloud layer and shared, world-anchored sun/moon transmission.
mod material;
mod noise;
mod render;
mod sky_cache;
use crate::{ApplyAtmosphere, AtmosphereOwner, AtmosphereState, WorldEnvironmentView};
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
pub use material::{CloudExtension, CloudMaterial, CloudMaterialOptIn, CloudMaterialSystems};
pub use render::{CloudShadowGpu, CloudShadowLayout, surface_layout};
pub(crate) use render::{CloudTarget, Pipelines as CloudPipelines, refresh as refresh_clouds};
use world::{atmosphere::linear_rgb, weather::VISIBILITY_EXTINCTION};

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
#[derive(Resource, Default)]
pub struct CloudOrigin(pub [f64; 2]);
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct CloudAssets {
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
#[derive(Resource, Clone, Copy, Default, ExtractResource, Pod, Zeroable)]
#[extract_app(bevy::render::RenderApp)]
#[repr(C)]
pub struct CloudParams {
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
/// The moon's face is drawn as the sunlit moon by day. At night it is dimmed to stay readable
/// at the night exposure: its seas show instead of a glare-white disc.
const MOON_NIGHT_GAIN: f32 = 0.09;
/// Earthshine on the moon's dark side, as a share of its sunlit face's light with the earth
/// full as seen from the moon (a new moon from here).
const EARTHSHINE: f32 = 0.04;
/// Light of the moonless night sky just above the horizon, as a share of the night's sky light
/// (as the haze's own light).
const NIGHT_SKY_GLOW: f32 = 0.15;
/// Mean normal albedo of the moon's drawn face (`moon_albedo` in `shaders/sky/composite.wesl`).
const MOON_MEAN_ALBEDO: f32 = 0.11;
/// The moon's path on the sea as the eye sees it: each wave facet mirrors the whole disc, which
/// the sea's averaged glitter spreads into nothing, so its image is drawn this much brighter.
const MOON_GLITTER: f32 = 40.;
/// Seconds after which the wave clock wraps; every wave completes whole cycles in it
/// (`shaders/water/waves.wesl`).
pub const WAVE_PERIOD: f64 = 3600.;
/// Mist noise tile, metres; `MIST_NOISE_PERIOD` in `shaders/sky/composite.wesl`.
pub const MIST_NOISE_PERIOD: f64 = 2048.;
/// Mist drifts slowly with the cloud wind.
const MIST_DRIFT_METRES_PER_SECOND: f64 = 0.6;
/// A newly published mist map fades in rather than appearing at once.
const MIST_FADE_SECONDS: f32 = 3.;
#[derive(Component, Clone, bevy::render::extract_component::ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub struct CloudView;
pub struct CloudsPlugin;
impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CloudClock>()
            .init_resource::<crate::shelter::RainShelter>()
            .init_resource::<crate::forest_shadow::ForestShadow>()
            .init_resource::<crate::forest_shadow::ForestSkyOcclusion>()
            .init_resource::<crate::valley_mist::ValleyMist>()
            .init_resource::<crate::shore::Shore>()
            .init_resource::<CloudOrigin>()
            .init_resource::<CloudQuality>()
            .init_resource::<CloudParams>()
            .init_resource::<ForestSkyLevels>();
        // Pure ECS atmosphere tests/tools do not install GPU or asset services.
        if app.get_sub_app(bevy::render::RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractResourcePlugin::<CloudAssets>::default(),
            ExtractResourcePlugin::<CloudParams>::default(),
            ExtractResourcePlugin::<CloudQuality>::default(),
            ExtractResourcePlugin::<ForestSkyLevels>::default(),
            bevy::render::extract_component::ExtractComponentPlugin::<CloudView>::default(),
            material::CloudMaterialPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(
            PostUpdate,
            (
                sync,
                publish_shelter,
                publish_forest_shadow,
                publish_valley_mist,
                publish_shore,
            )
                .after(ApplyAtmosphere),
        );
        render::install(app);
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
        noise::generate(64),
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
    commands.insert_resource(CloudAssets {
        noise: images.add(noise),
        shadows: images.add(shadows),
        shelter: images.add(crate::shelter::RainShelter::image()),
        forest_shadow: images.add(crate::forest_shadow::ForestShadow::image()),
        mist: images.add(crate::valley_mist::ValleyMist::image()),
        shore: images.add(crate::shore::Shore::image()),
        parameters: buffers.add(ShaderBuffer::new(
            vec![CloudParams::default()],
            RenderAssetUsages::RENDER_WORLD,
        )),
    });
}
/// Inputs for the low air and the sea: valley mist, presentation switches, look tuning, the
/// sea surface and its shore.
type LowAir<'w> = (
    Res<'w, crate::valley_mist::ValleyMist>,
    Option<Res<'w, crate::AtmospherePresentation>>,
    Option<Res<'w, crate::FogTuning>>,
    Option<Res<'w, crate::SeaSurface>>,
    Res<'w, crate::shore::Shore>,
);
fn sync(
    mut commands: Commands,
    state: Res<AtmosphereState>,
    origin: Res<CloudOrigin>,
    mut clock: ResMut<CloudClock>,
    time: Res<Time>,
    quality: Res<CloudQuality>,
    mut params: ResMut<CloudParams>,
    views: Query<(
        Entity,
        Option<&CloudView>,
        &WorldEnvironmentView,
        Option<&GlobalTransform>,
    )>,
    shelter: Res<crate::shelter::RainShelter>,
    (forest, sky): (
        Res<crate::forest_shadow::ForestShadow>,
        Res<crate::forest_shadow::ForestSkyOcclusion>,
    ),
    (mist, presentation, tuning, sea, shore): LowAir,
    mut mist_fade: Local<(u64, f32)>,
) {
    let profile = state.effective_profile();
    let profile = profile.as_ref();
    let p = &profile.clouds;
    let active = state.owner != AtmosphereOwner::Isolated
        && *quality != CloudQuality::Off
        && profile.outdoor
        && p.enabled
        && p.validate().is_ok();
    if active && (state.owner == AtmosphereOwner::Game || clock.playing) {
        clock.seconds += time.delta_secs_f64();
    }
    let value = state.evaluate(profile);
    let sun = state
        .direction_override
        .filter(|v| v.is_finite() && v.length_squared() > 0.01)
        .map(Vec3::normalize)
        .unwrap_or(Vec3::from_array(value.direction_to_sun));
    let period = p.period_metres();
    let wind = p.wind_degrees.to_radians();
    let travel = clock.seconds * f64::from(p.wind_metres_per_second);
    let offset = p.wrapped_origin(origin.0);
    *params = CloudParams {
        layer: [
            p.base_metres,
            p.thickness_metres,
            p.size_metres * 4.,
            if active { 1. } else { 0. },
        ],
        shape: [
            p.coverage,
            p.density * 0.025,
            p.erosion,
            (p.seed.wrapping_mul(747796405) % 65536) as f32 / 8192.,
        ],
        offset: [
            offset[0],
            offset[1],
            (travel * f64::from(wind.cos())).rem_euclid(period) as f32,
            (travel * f64::from(wind.sin())).rem_euclid(period) as f32,
        ],
        // Lux at the cloud layer, below.
        sun: sun.extend(0.).to_array(),
        moon: Vec3::from_array(value.direction_to_moon)
            .extend(cloud_illuminance(
                value.moon_lux,
                value.direction_to_moon[1],
            ))
            .to_array(),
        sun_color: Vec3::from_array(value.sun_linear).extend(0.).to_array(),
        moon_color: Vec3::from_array(value.moon_linear).extend(0.).to_array(),
        ambient: Vec3::from_array(value.ambient_linear)
            .extend(value.ambient_lux)
            .to_array(),
        haze: Vec3::from_array(linear_rgb(state.profile.haze_srgb))
            .extend(
                views
                    .iter()
                    .find_map(|(_, _, v, _)| v.visibility_override)
                    .unwrap_or(state.profile.visibility_metres),
            )
            .to_array(),
        fog: [0.; 4],
        transition: [0.; 4],
        weather: [0.; 4],
        shelter: shelter.parameters(),
        forest_shadow: forest.parameters(),
        forest_sky: [sky.0.clamp(0., 1.), 0., 0., 0.],
        low_haze: [0.; 4],
        mist: [0.; 4],
        mist_map: [0.; 4],
        mist_drift: [0.; 4],
        air_light: [0.; 4],
        air_sun: [0.; 4],
        shafts: [0.; 4],
        near_sun: [0.; 4],
        ocean: [0.; 4],
        ocean_waves: [0.; 4],
        shore_map: [0.; 4],
        lightning: [0.; 4],
        lightning_channel: [0.; 4],
        lightning_segments: [[0.; 4]; 2 * crate::lightning::SEGMENTS],
        moon_disc: [0.; 4],
        moon_frame: [0.; 4],
        moon_sunward: [0.; 4],
        moon_face: [0.; 4],
        night_sky: [0.; 4],
    };
    if profile.outdoor && profile.night.enabled && state.owner != AtmosphereOwner::Isolated {
        let moon = Vec3::from_array(value.direction_to_moon);
        let sunward = Vec3::from_array(value.direction_to_sun);
        params.moon_disc = moon
            .extend((0.5 * profile.night.diameter_degrees).to_radians())
            .to_array();
        let earth_lit = 0.5 * (1. + moon.dot(sunward));
        params.moon_frame = Vec3::from_array(value.moon_north)
            .extend(EARTHSHINE * earth_lit)
            .to_array();
        let gain = 1. + (MOON_NIGHT_GAIN - 1.) * value.adaptation;
        let face = Vec3::from_array(linear_rgb(profile.sun_srgb)) * profile.sun_lux
            / std::f32::consts::PI
            * gain;
        params.moon_face = face.extend(0.).to_array();
        let luminance = |c: Vec3| c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        let radius = params.moon_disc[3];
        let light = value.moon_lux * luminance(Vec3::from_array(value.moon_linear))
            / (std::f32::consts::PI * radius * radius);
        let image = if light > 0. {
            luminance(face) * MOON_MEAN_ALBEDO * value.moon_lit * MOON_GLITTER / light
        } else {
            0.
        };
        params.moon_sunward = sunward.extend(image).to_array();
        params.night_sky = (Vec3::from_array(value.ambient_linear)
            * value.ambient_lux
            * NIGHT_SKY_GLOW
            * value.night_weight)
            .extend(0.)
            .to_array();
    }
    let flash = state
        .lightning
        .filter(|_| profile.outdoor && state.owner != AtmosphereOwner::Isolated);
    if let Some(flash) = flash {
        params.lightning = flash
            .top
            .extend(flash.flash * crate::lightning::FLASH_SKY)
            .to_array();
        params.lightning_channel = [flash.channel, 0., 0., 0.];
        params.lightning_segments = flash.segments;
    }
    // The flash lights the haze and rain fog all around.
    let flash_air = Vec3::from_array(crate::lightning::FLASH_COLOR)
        * flash.map_or(0., |f| f.flash)
        * crate::lightning::FLASH_SKY
        * 0.015;
    if let Some(level) = sea
        .and_then(|s| s.level)
        .filter(|_| profile.outdoor && state.owner != AtmosphereOwner::Isolated)
    {
        let wind = p.wind_degrees.to_radians();
        params.ocean = [
            level,
            wind.cos(),
            wind.sin(),
            time.elapsed_secs_f64().rem_euclid(WAVE_PERIOD) as f32,
        ];
        params.ocean_waves = [
            1.,
            state.weather.map_or(1., |w| w.wind_strength).clamp(0.2, 3.),
            0.,
            0.,
        ];
        params.shore_map = shore.parameters(origin.0);
    }
    // Sunlight after the atmosphere: dimmed and reddened as Bevy lights surfaces, at the camera
    // for the air around it and at the middle of the cloud layer for the clouds.
    let visibility = views
        .iter()
        .find_map(|(_, _, v, _)| v.visibility_override)
        .unwrap_or(profile.visibility_metres);
    let altitude = views
        .iter()
        .find_map(|(_, _, _, t)| t.map(|t| t.translation().y))
        .unwrap_or(0.);
    let through = |altitude: f32| {
        crate::sunlight::sun_transmittance(
            profile.molecular_density,
            visibility,
            altitude,
            sun.y,
            0.5 * profile.sun_diameter_degrees.to_radians(),
        )
    };
    let sun_lux = if profile.outdoor { value.sun_lux } else { 0. };
    let sun_linear = Vec3::from_array(value.sun_linear);
    let near_sun = sun_linear * sun_lux * through(altitude);
    let cloud = through(p.base_metres + 0.5 * p.thickness_metres);
    let cloud_lux = cloud.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    params.sun[3] = sun_lux * cloud_lux;
    if cloud_lux > 0. {
        params.sun_color = (sun_linear * cloud / cloud_lux).extend(0.).to_array();
    }
    params.near_sun = near_sun.extend(0.).to_array();
    if state.owner != AtmosphereOwner::Isolated && profile.outdoor {
        params.weather = [
            state.wetness.clamp(0., 1.),
            state.weather.map_or(0., |w| w.precipitation.clamp(0., 1.)),
            0.,
            0.,
        ];
    }
    params.transition = [params.shape[0], params.shape[1], params.shape[2], 1.];
    if let Some(change) = state
        .weather_transition
        .filter(|_| state.owner != AtmosphereOwner::Isolated)
    {
        let from = change.from.apply(&state.profile).clouds;
        let to = change.to.apply(&state.profile).clouds;
        params.shape[0] = to.coverage;
        params.shape[1] = to.density * 0.025;
        params.shape[2] = to.erosion;
        params.transition = [
            from.coverage,
            from.density * 0.025,
            from.erosion,
            change.progress.clamp(0., 1.),
        ];
    }
    // Fog is lit by the sky; a cloud deck hides the sun and moon, and the light under it is
    // close to neutral grey, not blue skylight.
    let overcast = if p.enabled {
        ((p.coverage - 0.5) / 0.45).clamp(0., 1.)
    } else {
        0.
    };
    let ambient = Vec3::from_array(value.ambient_linear);
    let ambient = ambient.lerp(Vec3::splat(ambient.element_sum() / 3.), overcast * 0.8)
        * value.ambient_lux
        * 0.3;
    // The haze and far clouds take the clear sky's horizon colour; under a closing deck they
    // turn to its grey instead.
    params.weather[2] = overcast;
    let sun_light = near_sun * (1. - overcast);
    let moon_light = Vec3::from_array(value.moon_linear) * params.moon[3] * (1. - overcast);
    if let Some(fog) = state
        .weather_fog()
        .filter(|f| profile.outdoor && f.extinction > 0.)
    {
        let light = ambient + (sun_light + moon_light) * 0.025;
        params.fog = (Vec3::from_array(fog.tint_linear) * light + flash_air)
            .extend(fog.extinction)
            .to_array();
    }
    // The map also gives falling leaves their ground.
    if profile.outdoor && state.owner != AtmosphereOwner::Isolated {
        params.mist_map = mist.parameters(origin.0);
    }
    // Ground haze and valley mist in every weather; rain fog adds to them.
    if mist_fade.0 != mist.revision() {
        *mist_fade = (mist.revision(), 0.);
    }
    mist_fade.1 = (mist_fade.1 + time.delta_secs() / MIST_FADE_SECONDS).min(1.);
    let fog = &profile.fog;
    if profile.outdoor
        && fog.enabled
        && fog.validate().is_ok()
        && state.owner != AtmosphereOwner::Isolated
        && presentation.as_ref().is_none_or(|p| p.low_air)
    {
        let tuning = tuning.as_deref().copied().unwrap_or_default();
        params.low_haze = [
            VISIBILITY_EXTINCTION / fog.haze_visibility_metres * tuning.haze,
            mist.sea_level(),
            fog.haze_height_metres,
            0.,
        ];
        // Wet ground after rain feeds the mist; wind stirs it away.
        let stir = state.weather.map_or(1., |w| w.wind_strength);
        let amount = (value.mist + fog.mist_after_rain * state.wetness.clamp(0., 1.)).min(1.)
            * (1.5 - 0.5 * stir).clamp(0.4, 1.)
            * mist_fade.1;
        params.mist = [
            VISIBILITY_EXTINCTION / fog.mist_visibility_metres * amount * tuning.mist,
            fog.mist_depth_metres * (0.5 + 0.5 * amount) * tuning.mist_depth,
            0.,
            0.,
        ];
        let travel = clock.seconds * MIST_DRIFT_METRES_PER_SECOND;
        let drift = [wind.cos(), wind.sin()].map(|d| travel * f64::from(d));
        params.mist_drift = [
            (origin.0[0] - drift[0]).rem_euclid(MIST_NOISE_PERIOD) as f32,
            (origin.0[1] - drift[1]).rem_euclid(MIST_NOISE_PERIOD) as f32,
            0.,
            0.,
        ];
        params.air_light = (ambient + moon_light * 0.025 + flash_air)
            .extend(0.)
            .to_array();
        params.air_sun = sun_light.extend(0.).to_array();
        if presentation.is_none_or(|p| p.light_shafts) {
            // Humid morning air under the crowns holds more.
            params.shafts = [
                VISIBILITY_EXTINCTION / fog.canopy_air_visibility_metres
                    * (0.6 + 0.8 * value.mist.min(1.))
                    * tuning.canopy_air,
                0.,
                0.,
                0.,
            ];
        }
    }
    for (e, view, _, _) in &views {
        if active && view.is_none() {
            commands.entity(e).insert(CloudView);
        } else if !active && view.is_some() {
            commands.entity(e).remove::<CloudView>();
        }
    }
}

/// Upload the shelter map only when a rebuild changed it.
fn publish_shelter(
    shelter: Res<crate::shelter::RainShelter>,
    assets: Option<Res<CloudAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut published: Local<Option<u64>>,
) {
    let Some(assets) = assets else {
        return;
    };
    if *published == Some(shelter.revision()) {
        return;
    }
    if let Some(mut image) = images.get_mut(&assets.shelter) {
        shelter.write(&mut image);
        *published = Some(shelter.revision());
    }
}

/// The forest map's sky levels (mip levels 1 and up) for the render world to upload, since
/// Bevy rewrites only level 0 of an existing texture.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct ForestSkyLevels {
    pub revision: u64,
    pub levels: std::sync::Arc<Vec<Vec<u8>>>,
}

/// Upload the forest shadow map only when a rebuild changed it.
fn publish_forest_shadow(
    forest: Res<crate::forest_shadow::ForestShadow>,
    assets: Option<Res<CloudAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut levels: ResMut<ForestSkyLevels>,
    mut published: Local<Option<u64>>,
) {
    let Some(assets) = assets else {
        return;
    };
    if *published == Some(forest.revision()) {
        return;
    }
    if let Some(mut image) = images.get_mut(&assets.forest_shadow) {
        forest.write(&mut image);
        *levels = ForestSkyLevels {
            revision: forest.revision(),
            levels: std::sync::Arc::new(forest.sky_level_bytes()),
        };
        *published = Some(forest.revision());
    }
}

/// Upload the mist map only when a rebuild changed it.
fn publish_valley_mist(
    mist: Res<crate::valley_mist::ValleyMist>,
    assets: Option<Res<CloudAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut published: Local<Option<u64>>,
) {
    let Some(assets) = assets else {
        return;
    };
    if *published == Some(mist.revision()) {
        return;
    }
    if let Some(mut image) = images.get_mut(&assets.mist) {
        mist.write(&mut image);
        *published = Some(mist.revision());
    }
}

/// Upload the shore map only when a rebuild changed it.
fn publish_shore(
    shore: Res<crate::shore::Shore>,
    assets: Option<Res<CloudAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut published: Local<Option<u64>>,
) {
    let Some(assets) = assets else {
        return;
    };
    if *published == Some(shore.revision()) {
        return;
    }
    if let Some(mut image) = images.get_mut(&assets.shore) {
        shore.write(&mut image);
        *published = Some(shore.revision());
    }
}

/// Approximate clear-air extinction of moonlight for the cloud lighting path. Authored lux is
/// outside the atmosphere; it must not illuminate clouds or haze after the moon has set.
fn cloud_illuminance(lux: f32, elevation_sine: f32) -> f32 {
    let horizon = (elevation_sine / 3_f32.to_radians().sin()).clamp(0., 1.);
    let horizon = horizon * horizon * (3. - 2. * horizon);
    lux * horizon * (-0.12 / elevation_sine.max(0.04)).exp()
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
    #[test]
    fn wind_reaches_render_inputs_in_game_and_editor_without_double_advancing() {
        let mut app = App::new();
        let mut state = AtmosphereState::default();
        state.profile.clouds = world::clouds::CloudSettings::scattered();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs(1));
        app.insert_resource(state)
            .insert_resource(time)
            .init_resource::<CloudClock>()
            .init_resource::<CloudOrigin>()
            .init_resource::<CloudQuality>()
            .init_resource::<CloudParams>()
            .init_resource::<crate::shelter::RainShelter>()
            .init_resource::<crate::forest_shadow::ForestShadow>()
            .init_resource::<crate::forest_shadow::ForestSkyOcclusion>()
            .init_resource::<crate::valley_mist::ValleyMist>()
            .init_resource::<crate::shore::Shore>()
            .add_systems(Update, sync);
        app.update();
        let first = app.world().resource::<CloudParams>().offset;
        assert_eq!(app.world().resource::<CloudClock>().seconds, 1.);
        app.update();
        assert_ne!(app.world().resource::<CloudParams>().offset, first);
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Editor;
        app.world_mut().resource_mut::<CloudClock>().seconds = 30.;
        app.update();
        assert_eq!(app.world().resource::<CloudClock>().seconds, 30.);
        let paused = app.world().resource::<CloudParams>().offset;
        app.update();
        assert_eq!(app.world().resource::<CloudParams>().offset, paused);
        app.world_mut().resource_mut::<CloudClock>().playing = true;
        app.update();
        assert_eq!(app.world().resource::<CloudClock>().seconds, 31.);
        assert_ne!(app.world().resource::<CloudParams>().offset, paused);
        *app.world_mut().resource_mut::<CloudQuality>() = CloudQuality::Off;
        app.update();
        assert_eq!(app.world().resource::<CloudParams>().layer[3], 0.);
        assert_eq!(app.world().resource::<CloudClock>().seconds, 31.);
        *app.world_mut().resource_mut::<CloudQuality>() = CloudQuality::High;
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Isolated;
        app.update();
        assert_eq!(app.world().resource::<CloudParams>().layer[3], 0.);
        assert_eq!(app.world().resource::<CloudClock>().seconds, 31.);
    }

    #[test]
    fn weather_changes_send_both_cloud_shapes_and_settled_weather_matches_the_target() {
        use world::weather::WeatherKind;
        let mut app = App::new();
        let mut state = AtmosphereState::default();
        state.profile.clouds = world::clouds::CloudSettings::scattered();
        let authored = state.profile.clone();
        let change = world::weather::WeatherTransition {
            from: WeatherKind::Clear.preset(),
            to: WeatherKind::Storm.preset(),
            progress: 0.25,
        };
        state.weather = Some(change.from.lerp(change.to, 0.1));
        state.weather_transition = Some(change);
        app.insert_resource(state)
            .init_resource::<Time>()
            .init_resource::<CloudClock>()
            .init_resource::<CloudOrigin>()
            .init_resource::<CloudQuality>()
            .init_resource::<CloudParams>()
            .init_resource::<crate::shelter::RainShelter>()
            .init_resource::<crate::forest_shadow::ForestShadow>()
            .init_resource::<crate::forest_shadow::ForestSkyOcclusion>()
            .init_resource::<crate::valley_mist::ValleyMist>()
            .init_resource::<crate::shore::Shore>()
            .add_systems(Update, sync);
        app.update();
        let params = *app.world().resource::<CloudParams>();
        let from = change.from.apply(&authored).clouds;
        let to = change.to.apply(&authored).clouds;
        assert_eq!(params.shape[0], to.coverage);
        assert_eq!(params.shape[1], to.density * 0.025);
        assert_eq!(
            params.transition,
            [from.coverage, from.density * 0.025, from.erosion, 0.25]
        );

        // Settled or authored weather: the previous shape equals the target at full progress.
        let mut state = app.world_mut().resource_mut::<AtmosphereState>();
        state.weather = None;
        state.weather_transition = None;
        app.update();
        let params = *app.world().resource::<CloudParams>();
        assert_eq!(
            params.transition,
            [params.shape[0], params.shape[1], params.shape[2], 1.]
        );
    }

    #[test]
    fn twilight_haze_cannot_receive_daylight_from_a_set_sun() {
        let profile = world::atmosphere::AtmosphereProfile::default();
        let sunlight = |hour: f32, altitude: f32| {
            let light = world::atmosphere::evaluate(&profile, hour / 24.);
            crate::sunlight::sun_transmittance(
                1.,
                profile.visibility_metres,
                altitude,
                light.direction_to_sun[1],
                0.0065,
            ) * light.sun_lux
        };
        for hour in [0., 5.3, 18.3, 23.] {
            assert_eq!(sunlight(hour, 30.), Vec3::ZERO, "{hour}");
            assert_eq!(sunlight(hour, 2000.), Vec3::ZERO, "{hour}");
        }
        assert!(sunlight(12., 30.).y > 50_000.);
        // No switch from full moonlight to zero at the horizon.
        assert!(cloud_illuminance(100_000., 0.0001) < 1.);
        assert_eq!(cloud_illuminance(100_000., -0.0001), 0.);
        let night = world::atmosphere::evaluate(&profile, 0.);
        assert!(cloud_illuminance(night.moon_lux, night.direction_to_moon[1]) > 0.);
    }
}

/// Stable zero-density binding for isolated material/renderer studies.
pub fn fallback_parameters() -> Handle<ShaderBuffer> {
    bevy::asset::uuid_handle!("59e29465-cc65-47d8-92a0-fc10a8e13a20")
}
pub fn init_fallback(mut buffers: ResMut<Assets<ShaderBuffer>>) {
    buffers
        .insert(
            fallback_parameters().id(),
            ShaderBuffer::new(
                vec![CloudParams::default()],
                RenderAssetUsages::RENDER_WORLD,
            ),
        )
        .unwrap();
}
