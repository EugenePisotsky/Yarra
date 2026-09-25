//! Game weather: advances the shared sequence and overlays it on the authored atmosphere and
//! vegetation wind. Editor workspaces and studies own the authored profile and never see it.
use crate::{ActiveWorldSpace, ApplyAtmosphere, AtmosphereOwner, AtmosphereState, TreeWindSystems};
use bevy::prelude::*;
use vegetation_render::{VegetationAmbientGain, VegetationWind};
use world::{
    atmosphere::{AtmosphereProfile, evaluate},
    weather::{WeatherKind, WeatherRuntime, WeatherSchedule},
};

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
    schedule: WeatherSchedule,
    runtime: Option<WeatherRuntime>,
    /// Freeze the weather clock, e.g. during an A/B capture.
    pub paused: bool,
    /// Weather clock multiplier for testing transitions.
    pub time_scale: f32,
}
impl GameWeather {
    pub fn new(start: WeatherStart, seed: u64) -> Self {
        Self {
            start,
            seed,
            schedule: default(),
            runtime: None,
            paused: false,
            time_scale: 1.0,
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
    pub fn runtime_mut(&mut self) -> Option<&mut WeatherRuntime> {
        self.runtime.as_mut()
    }
    /// The running weather, starting settled near the authored cloud layer if needed.
    fn running(&mut self, profile: &AtmosphereProfile) -> &mut WeatherRuntime {
        self.runtime.get_or_insert_with(|| {
            WeatherRuntime::new(
                self.schedule.clone(),
                self.seed,
                WeatherKind::nearest_to(profile),
                false,
            )
        })
    }
    /// Blend to `kind` and hold it, starting weather from the authored profile if needed.
    pub fn request(&mut self, profile: &AtmosphereProfile, kind: WeatherKind, seconds: f32) {
        let runtime = self.running(profile);
        runtime.automatic = false;
        runtime.request(kind, seconds);
    }
    /// Follow or stop the random sequence, starting weather from the authored profile if needed.
    pub fn set_automatic(&mut self, profile: &AtmosphereProfile, automatic: bool) {
        self.running(profile).automatic = automatic;
    }
    /// Start the next scheduled change now, keeping the automatic setting.
    pub fn next_random(&mut self, profile: &AtmosphereProfile) -> WeatherKind {
        self.running(profile).next_random()
    }
    /// Return to the authored profile.
    pub fn clear(&mut self) {
        self.runtime = None;
        self.start = WeatherStart::Authored;
    }
    fn initialize(&mut self, profile: &AtmosphereProfile) {
        if self.runtime.is_some() {
            return;
        }
        self.runtime = match self.start {
            WeatherStart::Authored => None,
            WeatherStart::Automatic => Some(WeatherRuntime::new(
                self.schedule.clone(),
                self.seed,
                WeatherKind::nearest_to(profile),
                true,
            )),
            WeatherStart::Manual(kind) => Some(WeatherRuntime::new(
                self.schedule.clone(),
                self.seed,
                kind,
                false,
            )),
        };
        if let Some(runtime) = &self.runtime {
            info!(
                "WEATHER start={:?} seed={} initial={:?}",
                self.start,
                self.seed,
                runtime.target()
            );
        }
    }
}

pub struct GameWeatherPlugin;
impl Plugin for GameWeatherPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GameWeather>() {
            app.insert_resource(GameWeather::new(WeatherStart::Automatic, 0));
        }
        app.add_systems(
            PostUpdate,
            (
                advance_weather
                    .after(crate::world_streaming::sync_world_atmosphere)
                    .before(ApplyAtmosphere),
                (
                    apply_weather_wind.before(TreeWindSystems),
                    apply_weather_ambient,
                ),
            )
                .chain(),
        );
    }
}

fn advance_weather(
    time: Res<Time>,
    active: Option<Res<ActiveWorldSpace>>,
    mut weather: ResMut<GameWeather>,
    mut atmosphere: ResMut<AtmosphereState>,
    mut previous_target: Local<Option<WeatherKind>>,
) {
    if atmosphere.owner != AtmosphereOwner::Game {
        if atmosphere.weather.is_some() || atmosphere.weather_transition.is_some() {
            atmosphere.weather = None;
            atmosphere.weather_transition = None;
            atmosphere.wetness = 0.0;
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
    let current = weather.runtime.as_mut().map(|runtime| {
        if !paused {
            runtime.advance(time.delta_secs() * scale);
        }
        runtime.current()
    });
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
    let transition = weather.runtime.as_ref().map(WeatherRuntime::transition);
    let wetness = weather
        .runtime
        .as_ref()
        .map_or(0.0, WeatherRuntime::wetness);
    if atmosphere.weather != current
        || atmosphere.weather_transition != transition
        || atmosphere.wetness != wetness
    {
        atmosphere.weather = current;
        atmosphere.weather_transition = transition;
        atmosphere.wetness = wetness;
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
/// trees and clouds share one wind. Studies and replays that drive wind externally are left alone.
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
