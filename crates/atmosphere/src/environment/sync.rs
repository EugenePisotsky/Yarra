//! Writes `EnvironmentParams` from the atmosphere state each frame: the cloud layer, the sun and
//! moon after the atmosphere, lightning, the sea, weather and the low air.
use super::{EnvironmentOrigin, EnvironmentParams, MIST_NOISE_PERIOD, WAVE_PERIOD};
use crate::clouds::{CloudClock, CloudQuality, CloudView};
use crate::{AtmosphereOwner, AtmosphereState, WorldEnvironmentView};
use bevy::prelude::*;
use world::{
    atmosphere::{AtmosphereProfile, EvaluatedAtmosphere, linear_rgb},
    weather::VISIBILITY_EXTINCTION,
};

/// The moon's face is drawn as the sunlit moon by day. At night it is dimmed to stay readable
/// at the night exposure: its seas show instead of a glare-white disc.
const MOON_NIGHT_GAIN: f32 = 0.09;
/// Earthshine on the moon's dark side, as a share of its sunlit face's light with the earth
/// full as seen from the moon (a new moon from here).
const EARTHSHINE: f32 = 0.04;
/// Light of the moonless night sky just above the horizon, as a share of the night's sky light
/// (as the haze's own light).
const NIGHT_SKY_GLOW: f32 = 0.15;
/// Mean normal albedo of the moon's drawn face (`moon_albedo` in `shaders/sky/celestial.wesl`).
const MOON_MEAN_ALBEDO: f32 = 0.11;
/// The moon's path on the sea as the eye sees it: each wave facet mirrors the whole disc, which
/// the sea's averaged glitter spreads into nothing, so its image is drawn this much brighter.
const MOON_GLITTER: f32 = 40.;
/// Mist drifts slowly with the cloud wind.
const MIST_DRIFT_METRES_PER_SECOND: f64 = 0.6;
/// A newly published mist map fades in rather than appearing at once.
const MIST_FADE_SECONDS: f32 = 3.;
const LUMINANCE: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// Inputs for the low air and the sea: valley mist, presentation switches, look tuning, the
/// sea surface and its shore.
type LowAir<'w> = (
    Res<'w, crate::valley_mist::ValleyMist>,
    Option<Res<'w, crate::AtmospherePresentation>>,
    Option<Res<'w, crate::FogTuning>>,
    Option<Res<'w, crate::SeaSurface>>,
    Res<'w, crate::shore::Shore>,
);
#[allow(clippy::too_many_arguments)] // The environment's inputs.
pub(super) fn sync(
    mut commands: Commands,
    state: Res<AtmosphereState>,
    origin: Res<EnvironmentOrigin>,
    mut clock: ResMut<CloudClock>,
    time: Res<Time>,
    quality: Res<CloudQuality>,
    mut params: ResMut<EnvironmentParams>,
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
    let active = !state.isolated()
        && *quality != CloudQuality::Off
        && profile.outdoor
        && p.enabled
        && p.validate().is_ok();
    if active && (state.owner == AtmosphereOwner::Game || clock.playing) {
        clock.seconds += time.delta_secs_f64();
    }
    // The world's sky, sea and weather; an isolated workspace shows none of them.
    let world = profile.outdoor && !state.isolated();
    let value = state.evaluate(profile);
    let sun = state
        .direction_override
        .filter(|v| v.is_finite() && v.length_squared() > 0.01)
        .map(Vec3::normalize)
        .unwrap_or(Vec3::from_array(value.direction_to_sun));
    let visibility_override = views.iter().find_map(|(_, _, v, _)| v.visibility_override);
    let params = &mut *params;
    *params = EnvironmentParams {
        haze: Vec3::from_array(linear_rgb(state.profile.haze_srgb))
            .extend(visibility_override.unwrap_or(state.profile.visibility_metres))
            .to_array(),
        shelter: shelter.parameters(),
        forest_shadow: forest.parameters(),
        forest_sky: [sky.0.clamp(0., 1.), 0., 0., 0.],
        ..cloud_layer(profile, &value, sun, active, clock.seconds, origin.0)
    };
    if world && profile.night.enabled {
        moon(params, profile, &value);
    }
    let flash = state.lightning.filter(|_| world);
    if let Some(flash) = flash {
        params.lightning = flash
            .top
            .extend(flash.flash * crate::lightning::FLASH_SKY)
            .to_array();
        params.lightning_channel = [flash.channel, 0., 0., 0.];
        params.lightning_segments = flash.segments;
    }
    if let Some(level) = sea.and_then(|s| s.level).filter(|_| world) {
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
    let altitude = views
        .iter()
        .find_map(|(_, _, _, t)| t.map(|t| t.translation().y))
        .unwrap_or(0.);
    let visibility = visibility_override.unwrap_or(profile.visibility_metres);
    let near_sun = sunlight(params, profile, &value, sun, visibility, altitude);
    if world {
        params.weather = [
            state.wetness.clamp(0., 1.),
            state.weather.map_or(0., |w| w.precipitation.clamp(0., 1.)),
            0.,
            0.,
        ];
    }
    weather_transition(params, &state);
    let light = air_light(
        params,
        profile,
        &value,
        near_sun,
        flash.map_or(0., |f| f.flash),
    );
    if let Some(fog) = state
        .weather_fog()
        .filter(|f| profile.outdoor && f.extinction > 0.)
    {
        let lit = light.ambient + (light.sun + light.moon) * 0.025;
        params.fog = (Vec3::from_array(fog.tint_linear) * lit + light.flash)
            .extend(fog.extinction)
            .to_array();
    }
    // The map also gives falling leaves their ground.
    if world {
        params.mist_map = mist.parameters(origin.0);
    }
    // Ground haze and valley mist in every weather; rain fog adds to them.
    if mist_fade.0 != mist.revision() {
        *mist_fade = (mist.revision(), 0.);
    }
    mist_fade.1 = (mist_fade.1 + time.delta_secs() / MIST_FADE_SECONDS).min(1.);
    let fog = &profile.fog;
    if world
        && fog.enabled
        && fog.validate().is_ok()
        && presentation.as_ref().is_none_or(|p| p.low_air)
    {
        let tuning = tuning.as_deref().copied().unwrap_or_default();
        low_air(
            params,
            profile,
            &state,
            &value,
            &tuning,
            mist.sea_level(),
            mist_fade.1,
        );
        params.air_light = (light.ambient + light.moon * 0.025 + light.flash)
            .extend(0.)
            .to_array();
        params.air_sun = light.sun.extend(0.).to_array();
        let wind = p.wind_degrees.to_radians();
        let travel = clock.seconds * MIST_DRIFT_METRES_PER_SECOND;
        let drift = [wind.cos(), wind.sin()].map(|d| travel * f64::from(d));
        params.mist_drift = [
            (origin.0[0] - drift[0]).rem_euclid(MIST_NOISE_PERIOD) as f32,
            (origin.0[1] - drift[1]).rem_euclid(MIST_NOISE_PERIOD) as f32,
            0.,
            0.,
        ];
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

/// The cloud layer, its travel with the wind, and the light reaching it: lux at the cloud layer,
/// below. Everything else starts at zero.
fn cloud_layer(
    profile: &AtmosphereProfile,
    value: &EvaluatedAtmosphere,
    sun: Vec3,
    active: bool,
    seconds: f64,
    origin: [f64; 2],
) -> EnvironmentParams {
    let p = &profile.clouds;
    let period = p.period_metres();
    let wind = p.wind_degrees.to_radians();
    let travel = seconds * f64::from(p.wind_metres_per_second);
    let offset = p.wrapped_origin(origin);
    EnvironmentParams {
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
        ..default()
    }
}

/// The moon's disc, face, earthshine and glitter, and the night sky's own glow.
fn moon(params: &mut EnvironmentParams, profile: &AtmosphereProfile, value: &EvaluatedAtmosphere) {
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
    let radius = params.moon_disc[3];
    let light = value.moon_lux * Vec3::from_array(value.moon_linear).dot(LUMINANCE)
        / (std::f32::consts::PI * radius * radius);
    let image = if light > 0. {
        face.dot(LUMINANCE) * MOON_MEAN_ALBEDO * value.moon_lit * MOON_GLITTER / light
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

/// Sunlight after the atmosphere, dimmed and reddened as Bevy lights surfaces: at the middle of
/// the cloud layer for the clouds, and at the camera for the air around it (returned).
fn sunlight(
    params: &mut EnvironmentParams,
    profile: &AtmosphereProfile,
    value: &EvaluatedAtmosphere,
    sun: Vec3,
    visibility: f32,
    altitude: f32,
) -> Vec3 {
    let p = &profile.clouds;
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
    let cloud_lux = cloud.dot(LUMINANCE);
    params.sun[3] = sun_lux * cloud_lux;
    if cloud_lux > 0. {
        params.sun_color = (sun_linear * cloud / cloud_lux).extend(0.).to_array();
    }
    params.near_sun = near_sun.extend(0.).to_array();
    near_sun
}

/// Previous coverage, extinction and erosion during a weather change: each region of the field
/// blends from them to the next weather's at its own time. Settled weather blends from itself.
fn weather_transition(params: &mut EnvironmentParams, state: &AtmosphereState) {
    params.transition = [params.shape[0], params.shape[1], params.shape[2], 1.];
    if let Some(change) = state.weather_transition.filter(|_| !state.isolated()) {
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
}

/// Ground haze and valley mist: wet ground after rain feeds the mist, wind stirs it away, and a
/// newly published mist map fades in by `fade`.
fn low_air(
    params: &mut EnvironmentParams,
    profile: &AtmosphereProfile,
    state: &AtmosphereState,
    value: &EvaluatedAtmosphere,
    tuning: &crate::FogTuning,
    sea_level: f32,
    fade: f32,
) {
    let fog = &profile.fog;
    params.low_haze = [
        VISIBILITY_EXTINCTION / fog.haze_visibility_metres * tuning.haze,
        sea_level,
        fog.haze_height_metres,
        0.,
    ];
    let stir = state.weather.map_or(1., |w| w.wind_strength);
    let amount = (value.mist + fog.mist_after_rain * state.wetness.clamp(0., 1.)).min(1.)
        * (1.5 - 0.5 * stir).clamp(0.4, 1.)
        * fade;
    params.mist = [
        VISIBILITY_EXTINCTION / fog.mist_visibility_metres * amount * tuning.mist,
        fog.mist_depth_metres * (0.5 + 0.5 * amount) * tuning.mist_depth,
        0.,
        0.,
    ];
}

/// Light the haze, mist and rain fog scatter: the sky's, the sun's and the moon's through any
/// cloud deck, and a lightning flash's.
struct AirLight {
    ambient: Vec3,
    sun: Vec3,
    moon: Vec3,
    flash: Vec3,
}
/// Fog is lit by the sky; a cloud deck hides the sun and moon, and the light under it is close
/// to neutral grey, not blue skylight. Also sends how closed the deck is.
fn air_light(
    params: &mut EnvironmentParams,
    profile: &AtmosphereProfile,
    value: &EvaluatedAtmosphere,
    near_sun: Vec3,
    flash: f32,
) -> AirLight {
    let p = &profile.clouds;
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
    AirLight {
        ambient,
        sun: near_sun * (1. - overcast),
        moon: Vec3::from_array(value.moon_linear) * params.moon[3] * (1. - overcast),
        // The flash lights the haze and rain fog all around.
        flash: Vec3::from_array(crate::lightning::FLASH_COLOR)
            * flash
            * crate::lightning::FLASH_SKY
            * 0.015,
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
    fn wind_reaches_render_inputs_in_game_and_editor_without_double_advancing() {
        let mut app = App::new();
        let mut state = AtmosphereState::default();
        state.profile.clouds = world::clouds::CloudSettings::scattered();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs(1));
        app.insert_resource(state)
            .insert_resource(time)
            .init_resource::<CloudClock>()
            .init_resource::<EnvironmentOrigin>()
            .init_resource::<CloudQuality>()
            .init_resource::<EnvironmentParams>()
            .init_resource::<crate::shelter::RainShelter>()
            .init_resource::<crate::forest_shadow::ForestShadow>()
            .init_resource::<crate::forest_shadow::ForestSkyOcclusion>()
            .init_resource::<crate::valley_mist::ValleyMist>()
            .init_resource::<crate::shore::Shore>()
            .add_systems(Update, sync);
        app.update();
        let first = app.world().resource::<EnvironmentParams>().offset;
        assert_eq!(app.world().resource::<CloudClock>().seconds, 1.);
        app.update();
        assert_ne!(app.world().resource::<EnvironmentParams>().offset, first);
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Editor;
        app.world_mut().resource_mut::<CloudClock>().seconds = 30.;
        app.update();
        assert_eq!(app.world().resource::<CloudClock>().seconds, 30.);
        let paused = app.world().resource::<EnvironmentParams>().offset;
        app.update();
        assert_eq!(app.world().resource::<EnvironmentParams>().offset, paused);
        app.world_mut().resource_mut::<CloudClock>().playing = true;
        app.update();
        assert_eq!(app.world().resource::<CloudClock>().seconds, 31.);
        assert_ne!(app.world().resource::<EnvironmentParams>().offset, paused);
        *app.world_mut().resource_mut::<CloudQuality>() = CloudQuality::Off;
        app.update();
        assert_eq!(app.world().resource::<EnvironmentParams>().layer[3], 0.);
        assert_eq!(app.world().resource::<CloudClock>().seconds, 31.);
        *app.world_mut().resource_mut::<CloudQuality>() = CloudQuality::High;
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Isolated;
        app.update();
        assert_eq!(app.world().resource::<EnvironmentParams>().layer[3], 0.);
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
            .init_resource::<EnvironmentOrigin>()
            .init_resource::<CloudQuality>()
            .init_resource::<EnvironmentParams>()
            .init_resource::<crate::shelter::RainShelter>()
            .init_resource::<crate::forest_shadow::ForestShadow>()
            .init_resource::<crate::forest_shadow::ForestSkyOcclusion>()
            .init_resource::<crate::valley_mist::ValleyMist>()
            .init_resource::<crate::shore::Shore>()
            .add_systems(Update, sync);
        app.update();
        let params = *app.world().resource::<EnvironmentParams>();
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
        let params = *app.world().resource::<EnvironmentParams>();
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
