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
use world::{
    atmosphere::{evaluate, linear_rgb},
    weather::VISIBILITY_EXTINCTION,
};

#[derive(Resource, Default)]
pub struct CloudClock {
    pub seconds: f64,
    /// Editor owns preview transport; the standalone game advances automatically.
    pub playing: bool,
}
/// Presentation quality, independent of the authored weather profile.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, ExtractResource)]
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
}
#[derive(Resource, Clone, Copy, Default, ExtractResource, Pod, Zeroable)]
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
    /// Game weather for surfaces and precipitation: wetness, precipitation intensity.
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
}
/// Mist noise tile, metres; `MIST_NOISE_PERIOD` in `shaders/sky/composite.wgsl`.
pub const MIST_NOISE_PERIOD: f64 = 2048.;
/// Mist drifts slowly with the cloud wind.
const MIST_DRIFT_METRES_PER_SECOND: f64 = 0.6;
/// A newly published mist map fades in rather than appearing at once.
const MIST_FADE_SECONDS: f32 = 3.;
#[derive(Component, Clone, bevy::render::extract_component::ExtractComponent)]
pub struct CloudView;
pub struct CloudsPlugin;
impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CloudClock>()
            .init_resource::<crate::shelter::RainShelter>()
            .init_resource::<crate::forest_shadow::ForestShadow>()
            .init_resource::<crate::forest_shadow::ForestSkyOcclusion>()
            .init_resource::<crate::valley_mist::ValleyMist>()
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
        parameters: buffers.add(ShaderBuffer::new(
            bytemuck::bytes_of(&CloudParams::default()),
            RenderAssetUsages::RENDER_WORLD,
        )),
    });
}
fn sync(
    mut commands: Commands,
    state: Res<AtmosphereState>,
    origin: Res<CloudOrigin>,
    mut clock: ResMut<CloudClock>,
    time: Res<Time>,
    quality: Res<CloudQuality>,
    mut params: ResMut<CloudParams>,
    views: Query<(Entity, Option<&CloudView>, &WorldEnvironmentView)>,
    shelter: Res<crate::shelter::RainShelter>,
    (forest, sky): (
        Res<crate::forest_shadow::ForestShadow>,
        Res<crate::forest_shadow::ForestSkyOcclusion>,
    ),
    (mist, presentation, tuning): (
        Res<crate::valley_mist::ValleyMist>,
        Option<Res<crate::AtmospherePresentation>>,
        Option<Res<crate::FogTuning>>,
    ),
    mut mist_fade: Local<(u64, f32)>,
) {
    let profile = state.effective_profile();
    let profile = profile.as_ref();
    let p = &profile.clouds;
    let active = state.owner != AtmosphereOwner::Study
        && *quality != CloudQuality::Off
        && profile.outdoor
        && p.enabled
        && p.validate().is_ok();
    if active && (state.owner == AtmosphereOwner::Game || clock.playing) {
        clock.seconds += time.delta_secs_f64();
    }
    let value = evaluate(profile, state.phase);
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
        sun: sun
            .extend(cloud_illuminance(value.sun_lux, sun.y))
            .to_array(),
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
                    .find_map(|(_, _, v)| v.visibility_override)
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
    };
    if state.owner != AtmosphereOwner::Study && profile.outdoor {
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
        .filter(|_| state.owner != AtmosphereOwner::Study)
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
    let sun_light = Vec3::from_array(value.sun_linear) * params.sun[3] * (1. - overcast);
    let moon_light = Vec3::from_array(value.moon_linear) * params.moon[3] * (1. - overcast);
    if let Some(fog) = state
        .weather_fog()
        .filter(|f| profile.outdoor && f.extinction > 0.)
    {
        let light = ambient + (sun_light + moon_light) * 0.025;
        params.fog = (Vec3::from_array(fog.tint_linear) * light)
            .extend(fog.extinction)
            .to_array();
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
        && state.owner != AtmosphereOwner::Study
        && presentation.is_none_or(|p| p.low_air)
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
        params.mist_map = mist.parameters(origin.0);
        let travel = clock.seconds * MIST_DRIFT_METRES_PER_SECOND;
        let drift = [wind.cos(), wind.sin()].map(|d| travel * f64::from(d));
        params.mist_drift = [
            (origin.0[0] - drift[0]).rem_euclid(MIST_NOISE_PERIOD) as f32,
            (origin.0[1] - drift[1]).rem_euclid(MIST_NOISE_PERIOD) as f32,
            0.,
            0.,
        ];
        params.air_light = (ambient + moon_light * 0.025).extend(0.).to_array();
        params.air_sun = sun_light.extend(0.).to_array();
    }
    for (e, view, _) in &views {
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

/// Approximate clear-air extinction for the cloud lighting path, which does not
/// sample Bevy's atmosphere LUT. Authored lux is outside the atmosphere; it must
/// not illuminate clouds or their foreground haze after the light has set.
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
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Study;
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
        for hour in [0., 5.3, 18.3, 23.] {
            let light = evaluate(&profile, hour / 24.);
            assert_eq!(
                cloud_illuminance(light.sun_lux, light.direction_to_sun[1]),
                0.
            );
        }
        let noon = evaluate(&profile, 0.5);
        assert!(cloud_illuminance(noon.sun_lux, noon.direction_to_sun[1]) > 80_000.);
        // No switch from full sunlight to zero at the horizon.
        assert!(cloud_illuminance(100_000., 0.0001) < 1.);
        assert_eq!(cloud_illuminance(100_000., -0.0001), 0.);
        let night = evaluate(&profile, 0.);
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
                bytemuck::bytes_of(&CloudParams::default()),
                RenderAssetUsages::RENDER_WORLD,
            ),
        )
        .unwrap();
}
