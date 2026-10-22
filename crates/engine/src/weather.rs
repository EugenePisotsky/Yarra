//! Game weather: advances the shared sequence and overlays it on the authored atmosphere and
//! vegetation wind. The mountains have a sequence of their own (`WeatherSettings::mountains`),
//! blended in by the share of snow ground around the camera, where their precipitation falls as
//! snow: a sunny valley can lie under a snowing mountain. In their blizzards fresh snow frosts
//! snow country's bare ground wherever the camera is, and blows off after. Editor workspaces own
//! the authored profile and never see it.
mod rain;

use crate::{
    ActiveWorldSpace, ActiveWorldView, ApplyAtmosphere, AtmosphereOwner, AtmosphereState,
    ClipmapTerrain, TreeWindSystems, WorldOrigin,
};
use bevy::prelude::*;
use vegetation_render::{VegetationAmbientGain, VegetationWind};

use world::{
    atmosphere::{AtmosphereProfile, evaluate},
    weather::{WeatherKind, WeatherRuntime, WeatherSettings, WeatherTransition},
};

/// Snow ground counts this many leaves (cells) round the camera, so the weather changes over a
/// few hundred metres of the way up rather than at the snow's edge.
const SNOW_GROUND_RADIUS: i32 = 3;

/// Seconds over which the sensed snow ground follows the camera.
const SNOW_GROUND_SECONDS: f32 = 8.0;

/// What falls turns from rain to snow once snow ground covers this much of the ground around,
/// and back below this much, over this many seconds: rain and snow falling together read as a
/// fault, not as sleet.
const SNOWFALL_SHARES: [f32; 2] = [0.4, 0.6];

const SNOWFALL_CHANGE_SECONDS: f32 = 4.0;

/// The mountains' sequence runs from its own seed.
const MOUNTAIN_SEED: u64 = 0x6d6f_756e_7461_696e;

/// Fresh snow frosts bare ground only in snowfall this heavy (from a share of a blizzard's to all
/// of it), with this time constant, and blows off with this one once the blizzard eases: steadier
/// snow leaves the authored ground as it is.
const BLIZZARD_SNOWFALL: [f32; 2] = [0.85, 1.0];

const FROST_SETTLING_SECONDS: f32 = 90.0;

const FROST_CLEARING_SECONDS: f32 = 240.0;

/// How weather starts once the first world space is active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WeatherStart {
    /// Present the authored profile. F1 requests can still start weather.
    Authored,
    /// Begin near the authored cloud layer, then follow the random sequence.
    #[default]
    Automatic,
    /// Hold one preset until changed.
    Manual(WeatherKind),
}

#[derive(Resource)]
pub struct GameWeather {
    start: WeatherStart,
    seed: u64,
    runtime: Option<WeatherRuntime>,
    /// The mountains' sequence, beside the island's.
    mountains: Option<WeatherRuntime>,
    /// How far the mountains' blizzards have frosted snow country's bare ground, 0..1.
    fresh_snow: f32,
    /// Freeze the weather clock, e.g. during an A/B capture.
    pub paused: bool,
    /// Weather clock multiplier for testing transitions.
    pub time_scale: f32,
    /// Holds the ground this wet, 0..1, in place of what the weather has soaked it to (look
    /// captures of wet ground).
    pub wetness_override: Option<f32>,
}

impl GameWeather {
    pub fn new(start: WeatherStart, seed: u64) -> Self {
        Self {
            start,
            seed,
            runtime: None,
            mountains: None,
            fresh_snow: 0.0,
            paused: false,
            time_scale: 1.0,
            wetness_override: None,
        }
    }

    pub fn start(&self) -> WeatherStart {
        self.start
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// None while authored or before the first world space is active.
    pub fn runtime(&self) -> Option<&WeatherRuntime> {
        self.runtime.as_ref()
    }

    /// The running weather of the island and the mountains, starting settled near the authored
    /// cloud layer if needed.
    fn running(&mut self, profile: &AtmosphereProfile) -> [&mut WeatherRuntime; 2] {
        let seed = self.seed;
        let island = self.runtime.get_or_insert_with(|| {
            WeatherRuntime::new(
                profile.weather.clone(),
                seed,
                WeatherKind::nearest_to(profile),
                false,
            )
        });
        let (initial, automatic) = (island.target(), island.automatic);
        let mountains = self
            .mountains
            .get_or_insert_with(|| mountains(seed, initial, automatic));
        [island, mountains]
    }

    /// Blend to `kind` and hold it, starting weather from the authored profile if needed. The
    /// mountains take it too, as their snow.
    pub fn request(&mut self, profile: &AtmosphereProfile, kind: WeatherKind, seconds: f32) {
        for runtime in self.running(profile) {
            runtime.automatic = false;
            runtime.request(kind, seconds);
        }
    }

    /// Follow or stop the random sequences, starting weather from the authored profile if
    /// needed.
    pub fn set_automatic(&mut self, profile: &AtmosphereProfile, automatic: bool) {
        for runtime in self.running(profile) {
            runtime.automatic = automatic;
        }
    }

    /// Start the next scheduled changes now, keeping the automatic setting; the island's next
    /// state.
    pub fn next_random(&mut self, profile: &AtmosphereProfile) -> WeatherKind {
        let [island, mountains] = self.running(profile);
        mountains.next_random();
        island.next_random()
    }

    /// Return to the authored profile.
    pub fn clear(&mut self) {
        self.runtime = None;
        self.mountains = None;
        self.fresh_snow = 0.0;
        self.start = WeatherStart::Authored;
    }

    fn initialize(&mut self, profile: &AtmosphereProfile) {
        if self.runtime.is_some() {
            return;
        }

        self.runtime = match self.start {
            WeatherStart::Authored => None,
            WeatherStart::Automatic => Some(WeatherRuntime::new(
                profile.weather.clone(),
                self.seed,
                WeatherKind::nearest_to(profile),
                true,
            )),
            WeatherStart::Manual(kind) => Some(WeatherRuntime::new(
                profile.weather.clone(),
                self.seed,
                kind,
                false,
            )),
        };

        if let Some(runtime) = &self.runtime {
            // Held weather holds in the mountains too; a running sequence starts them in snow.
            let initial = match self.start {
                WeatherStart::Manual(kind) => kind,
                _ => WeatherKind::Overcast,
            };
            let mountains = mountains(self.seed, initial, runtime.automatic);

            // Weather that starts in a blizzard finds its frost lying.
            self.fresh_snow = if blizzard_snowfall(&mountains) > 0.01 {
                1.0
            } else {
                0.0
            };
            self.mountains = Some(mountains);
            info!(
                "WEATHER start={:?} seed={} initial={:?}",
                self.start,
                self.seed,
                runtime.target()
            );
        }
    }

    /// The weather where the camera is: the island's, with the mountains' blended in by the snow
    /// ground share; both ends of the change; and the wetness rain has left.
    fn local(&self, snow: f32) -> Option<(world::weather::WeatherParams, WeatherTransition, f32)> {
        let island = self.runtime.as_ref()?;
        let Some(mountains) = self.mountains.as_ref().filter(|_| snow > 0.0) else {
            return Some((island.current(), island.transition(), island.wetness()));
        };

        // Snow ground stays dry: the rain fell below it.
        if snow >= 1.0 {
            return Some((mountains.current(), mountains.transition(), 0.0));
        }

        let (a, b) = (island.transition(), mountains.transition());
        let transition = WeatherTransition {
            from: a.from.lerp(b.from, snow),
            to: a.to.lerp(b.to, snow),
            progress: a.progress + (b.progress - a.progress) * snow,
        };

        let wetness = island.wetness() * (1.0 - snow);
        Some((
            island.current().lerp(mountains.current(), snow),
            transition,
            wetness,
        ))
    }
}

/// The mountains' sequence from `initial`.
fn mountains(seed: u64, initial: WeatherKind, automatic: bool) -> WeatherRuntime {
    WeatherRuntime::new(
        WeatherSettings::mountains(),
        seed ^ MOUNTAIN_SEED,
        initial,
        automatic,
    )
}

/// How much of the ground around the camera lies under snow, followed smoothly: where the
/// mountains' weather is; and the share of what falls there as snow.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct SnowGround {
    pub share: f32,
    pub falling: f32,
    snowing: bool,
}

impl SnowGround {
    /// Turns what falls towards snow or rain as the share passes `SNOWFALL_SHARES`.
    fn change_falling(&mut self, seconds: f32) {
        if self.share >= SNOWFALL_SHARES[1] {
            self.snowing = true;
        } else if self.share <= SNOWFALL_SHARES[0] {
            self.snowing = false;
        }

        let change = seconds / SNOWFALL_CHANGE_SECONDS;
        let target = f32::from(u8::from(self.snowing));
        self.falling += (target - self.falling).clamp(-change, change);
    }
}

pub struct GameWeatherPlugin;

impl Plugin for GameWeatherPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GameWeather>() {
            app.insert_resource(GameWeather::new(WeatherStart::Automatic, 0));
        }

        crate::lightning::plugin(app);
        app.init_resource::<SnowGround>().add_systems(
            PostUpdate,
            (
                sense_snow_ground,
                advance_weather
                    .after(crate::world_streaming::sync_world_atmosphere)
                    .before(ApplyAtmosphere),
                (
                    apply_weather_wind.before(TreeWindSystems),
                    apply_weather_ambient,
                    rain::update_rain_shelter.before(ApplyAtmosphere),
                ),
                rain::spawn_rain_splashes,
                rain::update_precipitation_reference.before(ApplyAtmosphere),
            )
                .chain(),
        );
    }
}

/// Follows the share of snow ground around the camera.
fn sense_snow_ground(
    time: Res<Time>,
    view: ActiveWorldView,
    origin: Option<Res<WorldOrigin>>,
    terrain: Res<ClipmapTerrain>,
    mut ground: ResMut<SnowGround>,
    mut sensed: Local<bool>,
) {
    let (Some(camera), Some(origin)) = (view.current().map(|v| v.transform), origin) else {
        return;
    };
    let Some(share) = terrain.snow_share(camera.translation(), &origin, SNOW_GROUND_RADIUS) else {
        return;
    };

    // The first reading holds at once: a game starting in the mountains starts in their weather.
    if !std::mem::replace(&mut *sensed, true) {
        ground.share = share;
        ground.snowing = share >= SNOWFALL_SHARES[1];
        ground.falling = f32::from(u8::from(ground.snowing));
    } else if ground.share != share {
        let follow = 1.0 - (-time.delta_secs() / SNOW_GROUND_SECONDS).exp();
        let next = ground.share + (share - ground.share) * follow;

        if (next * 10.0).round() != (ground.share * 10.0).round() {
            debug!("WEATHER_SNOW_GROUND share={next:.2} around={share:.2}");
        }

        ground.share = if (share - next).abs() < 1e-4 {
            share
        } else {
            next
        };
    }

    ground.change_falling(time.delta_secs());
}

/// How hard the mountains' snowfall comes as a blizzard's, 0..1: what frosts bare ground.
fn blizzard_snowfall(mountains: &WeatherRuntime) -> f32 {
    let [low, high] = BLIZZARD_SNOWFALL;
    let t = ((mountains.current().precipitation - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn advance_weather(
    time: Res<Time>,
    active: Option<Res<ActiveWorldSpace>>,
    ground: Res<SnowGround>,
    mut weather: ResMut<GameWeather>,
    mut atmosphere: ResMut<AtmosphereState>,
    mut previous_target: Local<Option<WeatherKind>>,
) {
    if atmosphere.owner != AtmosphereOwner::Game {
        if atmosphere.weather.is_some() || atmosphere.weather_transition.is_some() {
            atmosphere.weather = None;
            atmosphere.weather_transition = None;
            atmosphere.wetness = 0.0;
            atmosphere.snow = 0.0;
            atmosphere.snow_ground = 0.0;
            atmosphere.fresh_snow = 0.0;
        }
        return;
    }

    // The authored profile arrives with the first active world space.
    if active.is_some_and(|a| a.current().is_some()) {
        weather.initialize(&atmosphere.profile);
    }

    let paused = weather.paused;
    let scale = if weather.time_scale.is_finite() {
        weather.time_scale.clamp(0.0, 1000.0)
    } else {
        1.0
    };

    if let Some(runtime) = weather.runtime.as_mut() {
        // Published or previewed edits apply live, without restarting the sequence.
        if runtime.settings() != &atmosphere.profile.weather {
            runtime.set_settings(atmosphere.profile.weather.clone());
        }
    }

    if !paused {
        let GameWeather {
            runtime,
            mountains,
            fresh_snow,
            ..
        } = &mut *weather;

        let seconds = time.delta_secs() * scale;

        for runtime in runtime.iter_mut().chain(mountains.iter_mut()) {
            runtime.advance(seconds);
        }

        if let Some(mountains) = mountains {
            *fresh_snow = world::weather::lying(
                *fresh_snow,
                blizzard_snowfall(mountains),
                seconds,
                FROST_SETTLING_SECONDS,
                FROST_CLEARING_SECONDS,
            );
        }
    }

    let share = ground.share.clamp(0.0, 1.0);
    let snow = ground.falling.clamp(0.0, 1.0);
    let local = weather.local(share);
    let current = local.map(|(params, ..)| params);
    let target = weather.runtime.as_ref().map(WeatherRuntime::target);

    if target != *previous_target {
        if let (Some(target), Some(runtime)) = (target, weather.runtime.as_ref()) {
            info!(
                "WEATHER target={target:?} automatic={} progress={:.2}",
                runtime.automatic,
                runtime.progress()
            );
        }
        *previous_target = target;
    }

    let transition = local.map(|(_, transition, _)| transition);
    let wetness = weather
        .wetness_override
        .unwrap_or_else(|| local.map_or(0.0, |(.., wetness)| wetness));

    if atmosphere.weather != current
        || atmosphere.weather_transition != transition
        || atmosphere.wetness != wetness
        || atmosphere.snow != snow
        || atmosphere.snow_ground != share
        || atmosphere.fresh_snow != weather.fresh_snow
    {
        atmosphere.weather = current;
        atmosphere.weather_transition = transition;
        atmosphere.wetness = wetness;
        atmosphere.snow = snow;
        atmosphere.snow_ground = share;
        atmosphere.fresh_snow = weather.fresh_snow;
    }
}

/// The unscaled wind restored when weather stops driving it.
#[derive(Clone, Copy)]
struct WindBaseline {
    direction: Vec2,
    strength: f32,
    gustiness: f32,
    rate: f32,
}

/// Weather scales the baseline field. Direction follows the authored cloud drift, so grass,
/// trees and clouds share one wind. Wind driven externally (editor previews, tests) is left alone.
fn apply_weather_wind(
    atmosphere: Res<AtmosphereState>,
    wind: Option<ResMut<VegetationWind>>,
    mut baseline: Local<Option<WindBaseline>>,
) {
    let Some(mut wind) = wind else {
        return;
    };
    let weather = atmosphere
        .weather
        .filter(|_| atmosphere.owner == AtmosphereOwner::Game && !wind.externally_driven);
    let Some(weather) = weather else {
        if let Some(base) = baseline.take() {
            wind.direction = base.direction;
            wind.strength = base.strength;
            wind.gustiness = base.gustiness;
            wind.rate = base.rate;
        }
        return;
    };
    let base = *baseline.get_or_insert(WindBaseline {
        direction: wind.direction,
        strength: wind.strength,
        gustiness: wind.gustiness,
        rate: wind.rate,
    });
    let degrees = atmosphere.profile.clouds.wind_degrees.to_radians();
    let direction = Vec2::new(degrees.cos(), degrees.sin());
    let strength = base.strength * weather.wind_strength;
    let gustiness = (base.gustiness * weather.wind_gustiness).clamp(0.0, 1.0);
    let rate = base.rate * weather.wind_rate;

    // Compare first: VegetationWind is extracted and uploaded when it changes.
    if wind.direction != direction
        || wind.strength != strength
        || wind.gustiness != gustiness
        || wind.rate != rate
    {
        wind.direction = direction;
        wind.strength = strength;
        wind.gustiness = gustiness;
        wind.rate = rate;
    }
}

/// Exposure adaptation relative to the authored scene at the same time of day. Grass bounds its
/// lighting in exposed (display) units for a stylized, compressed look; scaling those bounds with
/// exposure keeps them fixed relative to the scene, as PBR surfaces are. Brighter overcast
/// ambient itself is a light change, which grass compresses by design.
fn ambient_gain(atmosphere: &AtmosphereState) -> f32 {
    if atmosphere.owner != AtmosphereOwner::Game
        || atmosphere.weather.is_none()
        || !atmosphere.phase.is_finite()
        || atmosphere.profile.validate().is_err()
    {
        return 1.0;
    }

    let ev = |profile: &AtmosphereProfile| evaluate(profile, atmosphere.phase).exposure_ev100;
    let gain = 2_f32.powf(ev(&atmosphere.profile) - ev(&atmosphere.effective_profile()));

    if gain.is_finite() {
        gain.clamp(0.0625, 16.0)
    } else {
        1.0
    }
}

/// Grass bounds its ambient fill; PBR surfaces do not. Keep both in step under weather.
fn apply_weather_ambient(
    atmosphere: Res<AtmosphereState>,
    gain: Option<ResMut<VegetationAmbientGain>>,
) {
    let Some(mut gain) = gain else {
        return;
    };
    let value = ambient_gain(&atmosphere);

    if gain.0 != value {
        gain.0 = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app(start: WeatherStart) -> App {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs(1));
        app.insert_resource(time)
            .insert_resource(GameWeather::new(start, 3))
            .init_resource::<SnowGround>()
            .init_resource::<AtmosphereState>()
            .init_resource::<VegetationWind>()
            .init_resource::<VegetationAmbientGain>()
            .add_systems(
                Update,
                (
                    advance_weather.before(ApplyAtmosphere),
                    apply_weather_wind,
                    apply_weather_ambient,
                )
                    .chain(),
            );
        app
    }

    #[test]
    fn snow_ground_brings_the_mountains_weather_and_keeps_the_ground_dry() {
        let mut app = test_app(WeatherStart::Authored);
        let profile = app.world().resource::<AtmosphereState>().profile.clone();
        app.world_mut()
            .resource_mut::<GameWeather>()
            .request(&profile, WeatherKind::Rain, 0.);
        app.update();
        let island = app.world().resource::<AtmosphereState>();
        assert_eq!(island.snow, 0.0);
        assert_eq!(island.weather, Some(WeatherKind::Rain.preset()));

        *app.world_mut().resource_mut::<SnowGround>() = SnowGround {
            share: 1.0,
            falling: 1.0,
            snowing: true,
        };
        app.update();
        let mountains = app.world().resource::<AtmosphereState>();
        assert_eq!(mountains.snow, 1.0);
        assert_eq!(
            mountains.weather,
            Some(WeatherSettings::mountains().preset(WeatherKind::Rain)),
            "held weather holds in the mountains, as snow"
        );
        assert_eq!(mountains.wetness, 0.0);
    }

    #[test]
    fn blizzards_frost_bare_ground_wherever_the_camera_is_and_steady_snow_clears_it() {
        let mut app = test_app(WeatherStart::Authored);
        let profile = app.world().resource::<AtmosphereState>().profile.clone();
        app.world_mut()
            .resource_mut::<GameWeather>()
            .request(&profile, WeatherKind::Storm, 0.);

        // The test clock steps a second an update; the camera stays off the snow.
        for _ in 0..180 {
            app.update();
        }

        let settled = app.world().resource::<AtmosphereState>().fresh_snow;
        assert!(
            (settled - (1.0 - (-2.0_f32).exp())).abs() < 0.01,
            "{settled}"
        );

        // In the mountains rain is steady snow: it lets the frost blow off.
        app.world_mut()
            .resource_mut::<GameWeather>()
            .request(&profile, WeatherKind::Rain, 0.);

        for _ in 0..240 {
            app.update();
        }

        let cleared = app.world().resource::<AtmosphereState>().fresh_snow;
        assert!(
            (cleared - settled * (-1.0_f32).exp()).abs() < 0.01,
            "{cleared}"
        );
    }

    #[test]
    fn rain_turns_to_snow_past_most_of_the_ground_under_snow_and_back_below_less() {
        let mut ground = SnowGround::default();
        let mut walk = |share: f32, seconds: f32| {
            ground.share = share;
            ground.change_falling(seconds);
            ground.falling
        };
        assert_eq!(
            walk(0.5, 10.0),
            0.0,
            "half the ground under snow still rains"
        );
        assert_eq!(walk(0.65, 2.0), 0.5, "past most of it, rain turns to snow");
        assert_eq!(walk(0.65, 2.0), 1.0);
        assert_eq!(walk(0.5, 10.0), 1.0, "and half of it keeps the snow");
        assert_eq!(walk(0.35, 4.0), 0.0);
    }

    #[test]
    fn authored_start_presents_the_profile_and_wind_unchanged() {
        let mut app = test_app(WeatherStart::Authored);
        app.update();
        assert!(app.world().resource::<AtmosphereState>().weather.is_none());
        let wind = app.world().resource::<VegetationWind>();
        assert_eq!(wind.strength, VegetationWind::default().strength);
        assert_eq!(wind.direction, VegetationWind::default().direction);
    }

    #[test]
    fn manual_weather_overlays_game_atmosphere_and_scales_shared_wind() {
        let mut app = test_app(WeatherStart::Authored);
        let profile = app.world().resource::<AtmosphereState>().profile.clone();
        app.world_mut()
            .resource_mut::<GameWeather>()
            .request(&profile, WeatherKind::Storm, 0.);
        app.update();
        let state = app.world().resource::<AtmosphereState>();
        assert_eq!(state.weather, Some(WeatherKind::Storm.preset()));
        let effective = state.effective_profile();
        assert!(effective.clouds.enabled);

        // Reduced visibility is fog in front of the unchanged clear-air sky model.
        assert_eq!(effective.visibility_metres, profile.visibility_metres);
        let fog = state.weather_fog().unwrap();
        assert!(fog.visibility_metres < profile.visibility_metres * 0.1);
        assert!(fog.extinction > 0.);
        let wind = *app.world().resource::<VegetationWind>();
        let base = VegetationWind::default();
        assert!((wind.strength - base.strength * 1.9).abs() < 1e-5);
        assert!((wind.rate - 1.5).abs() < 1e-5);
        let expected = profile.clouds.wind_degrees.to_radians();
        assert!((wind.direction - Vec2::new(expected.cos(), expected.sin())).length() < 1e-5);

        // Opened exposure raises the grass bounds by the same factor as every PBR surface.
        let gain = app.world().resource::<VegetationAmbientGain>().0;
        let expected = 2_f32.powf(-WeatherKind::Storm.preset().exposure_offset_ev);
        assert!((gain - expected).abs() < 1e-3, "storm ambient gain {gain}");

        // Returning to authored weather restores the baseline wind exactly.
        app.world_mut().resource_mut::<GameWeather>().clear();
        app.update();
        assert!(app.world().resource::<AtmosphereState>().weather.is_none());
        let wind = app.world().resource::<VegetationWind>();
        assert_eq!(wind.strength, base.strength);
        assert_eq!(wind.direction, base.direction);
        assert_eq!(wind.rate, base.rate);
        assert_eq!(app.world().resource::<VegetationAmbientGain>().0, 1.0);
    }

    #[test]
    fn editor_ownership_and_external_wind_are_never_overlaid() {
        let mut app = test_app(WeatherStart::Manual(WeatherKind::Rain));
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Editor;
        let profile = app.world().resource::<AtmosphereState>().profile.clone();
        app.world_mut()
            .resource_mut::<GameWeather>()
            .request(&profile, WeatherKind::Rain, 0.);
        app.update();
        let state = app.world().resource::<AtmosphereState>();
        assert!(state.weather.is_none());
        assert_eq!(*state.effective_profile(), profile);

        let mut app = test_app(WeatherStart::Authored);
        app.world_mut()
            .resource_mut::<VegetationWind>()
            .externally_driven = true;
        let profile = app.world().resource::<AtmosphereState>().profile.clone();
        app.world_mut()
            .resource_mut::<GameWeather>()
            .request(&profile, WeatherKind::Storm, 0.);
        app.update();
        assert_eq!(
            app.world().resource::<VegetationWind>().strength,
            VegetationWind::default().strength
        );
    }

    #[test]
    fn paused_and_accelerated_clocks() {
        let mut app = test_app(WeatherStart::Authored);
        let profile = app.world().resource::<AtmosphereState>().profile.clone();
        let mut weather = app.world_mut().resource_mut::<GameWeather>();
        weather.request(&profile, WeatherKind::Overcast, 10.);
        weather.paused = true;
        app.update();
        let progress = |app: &App| {
            app.world()
                .resource::<GameWeather>()
                .runtime()
                .unwrap()
                .progress()
        };
        assert_eq!(progress(&app), 0.);
        let mut weather = app.world_mut().resource_mut::<GameWeather>();
        weather.paused = false;
        weather.time_scale = 5.;
        app.update();
        assert!((progress(&app) - 0.5).abs() < 1e-5);
    }
}
