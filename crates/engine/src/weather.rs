//! Game weather: advances the shared sequence and overlays it on the authored atmosphere and
//! vegetation wind. Editor workspaces and studies own the authored profile and never see it.
use crate::{
    ActiveWorldSpace, ApplyAtmosphere, AtmosphereOwner, AtmosphereState, ObjectFootprint,
    StreamedTerrainSurface, StreamedVisualObject, TreeWindSystems, WorldOrigin, WorldViewCamera,
    sample_resident_terrain_surface,
};
use atmosphere::{
    precipitation::{PrecipitationReference, RainSplashes},
    shelter::{METRES_PER_TEXEL, RainShelter, SIZE, ShelterDisc},
};
use bevy::prelude::*;
use vegetation_render::{VegetationAmbientGain, VegetationWind};
use world::{
    atmosphere::{AtmosphereProfile, evaluate},
    weather::{WeatherKind, WeatherRuntime},
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
                profile.weather.clone(),
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
                    update_rain_shelter.before(ApplyAtmosphere),
                ),
                spawn_rain_splashes,
                update_precipitation_reference.before(ApplyAtmosphere),
            )
                .chain(),
        );
    }
}

/// The ground the camera looks at: where its view ray meets the height of the followed
/// character (or 1.7 m below a free camera), clamped for low grazing views. The radius grows
/// with viewing distance so steep, distant views cover what they show.
fn splash_area(camera: &GlobalTransform, ground: Option<f32>) -> (Vec2, f32) {
    let position = camera.translation();
    let forward = camera.forward();
    let ground = ground.unwrap_or(position.y - 1.7);
    let height = (position.y - ground).max(0.5);
    let horizontal = if forward.y < -0.05 {
        height / -forward.y * forward.xz().length()
    } else {
        f32::INFINITY
    };
    let distance = horizontal.min(10.0);
    let centre = position.xz() + forward.xz().normalize_or(Vec2::Y) * distance;
    let [smallest, largest] = SPLASH_RADIUS;
    let radius = (0.8 * height.hypot(distance)).clamp(smallest, largest);
    (centre, radius)
}

/// Rain streaks stretch with the player's movement, not with the orbiting camera.
fn update_precipitation_reference(
    target: Query<&Transform, With<crate::actor::CameraTarget>>,
    reference: Option<ResMut<PrecipitationReference>>,
) {
    let Some(mut reference) = reference else {
        return;
    };
    let position = target.iter().next().map(|t| t.translation);
    if reference.0 != position {
        reference.0 = position;
    }
}

/// Impacts per square metre per second at full precipitation, around the ground the camera
/// looks at. The spawn disc is capped so the 512-entry buffer covers every live splash.
const SPLASHES_PER_SQUARE_METRE: f32 = 5.0;
const SPLASH_RADIUS: [f32; 2] = [6.0, 10.0];

/// Small generator for splash placement; variety, not statistical quality.
#[derive(Default)]
struct SplashRandom(u64);
impl SplashRandom {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Place drop impacts on resident terrain ahead of the camera. Steep ground and sheltered ground
/// get none; grass hides many of them, as it would.
#[allow(clippy::too_many_arguments)] // Weather, camera, terrain, shelter and the splash buffer.
fn spawn_rain_splashes(
    time: Res<Time>,
    atmosphere: Res<AtmosphereState>,
    shelter: Res<RainShelter>,
    origin: Option<Res<WorldOrigin>>,
    camera: Query<&GlobalTransform, With<WorldViewCamera>>,
    target: Query<&Transform, With<crate::actor::CameraTarget>>,
    surfaces: Query<&StreamedTerrainSurface>,
    splashes: Option<ResMut<RainSplashes>>,
    mut state: Local<(SplashRandom, f32)>,
) {
    let (Some(mut splashes), Some(origin), Some(camera)) = (splashes, origin, camera.iter().next())
    else {
        return;
    };
    let precipitation = atmosphere
        .weather
        .filter(|_| atmosphere.owner == AtmosphereOwner::Game && atmosphere.profile.outdoor)
        .map_or(0.0, |w| w.precipitation.clamp(0.0, 1.0));
    let (random, due) = &mut *state;
    if precipitation <= 0.0 {
        *due = 0.0;
        return;
    }
    let (centre, radius) = splash_area(camera, target.iter().next().map(|t| t.translation.y));
    *due += SPLASHES_PER_SQUARE_METRE
        * std::f32::consts::PI
        * radius
        * radius
        * precipitation
        * time.delta_secs().min(0.1);
    let count = due.floor().min(128.0);
    *due -= count;
    for _ in 0..count as u32 {
        // Uniform over the disc.
        let angle = random.next() * std::f32::consts::TAU;
        let point = centre + Vec2::from_angle(angle) * radius * random.next().sqrt();
        let Some(ground) =
            sample_resident_terrain_surface(&origin, surfaces.iter(), point.to_array())
        else {
            continue;
        };
        if ground.normal[1] < 0.6 {
            continue;
        }
        let impact = Vec3::new(point.x, ground.height, point.y);
        if random.next() > shelter.exposure(impact) {
            continue;
        }
        splashes.spawn(impact, Vec3::from_array(ground.normal), time.elapsed_secs());
    }
}

/// Rebuild after moving this far from the map centre, well inside its 64 m half size.
const SHELTER_RECENTRE_METRES: f32 = 16.0;
/// Streaming adds and removes objects over many frames; batch those rebuilds.
const SHELTER_MIN_INTERVAL: f32 = 0.5;
/// Crowns are narrower than their bounding boxes.
const CROWN_RADIUS_SCALE: f32 = 0.9;

/// Keep the shelter map around the camera while rain or wetness needs it. Any placed object
/// shelters the ground below its top, so trees, shrubs and rocks need no tagging.
#[allow(clippy::too_many_arguments)] // Weather state, camera, streamed objects and change sources.
fn update_rain_shelter(
    time: Res<Time<Real>>,
    atmosphere: Res<AtmosphereState>,
    origin: Option<Res<WorldOrigin>>,
    camera: Query<&GlobalTransform, With<WorldViewCamera>>,
    objects: Query<(&Transform, &ObjectFootprint), With<StreamedVisualObject>>,
    added: Query<(), Added<ObjectFootprint>>,
    mut removed: RemovedComponents<ObjectFootprint>,
    mut shelter: ResMut<RainShelter>,
    mut last: Local<Option<f32>>,
    mut pending: Local<bool>,
) {
    // Remember changes that arrive during the throttle interval.
    *pending |= !added.is_empty() | (removed.read().count() > 0);
    let needed = atmosphere.owner == AtmosphereOwner::Game
        && (atmosphere.wetness > 0.01 || atmosphere.weather.is_some_and(|w| w.precipitation > 0.0));
    let Some(camera) = camera.iter().next().filter(|_| needed) else {
        if shelter.enabled {
            shelter.disable();
        }
        *last = None;
        *pending = false;
        return;
    };
    let now = time.elapsed_secs();
    let centre = camera.translation().xz();
    let moved = centre.distance(shelter.centre()) > SHELTER_RECENTRE_METRES;
    let rebased = origin.is_some_and(|o| o.is_changed());
    let due = last.is_none_or(|t| now - t >= SHELTER_MIN_INTERVAL);
    if shelter.enabled && !moved && !rebased && (!*pending || !due) {
        return;
    }
    *pending = false;
    let reach = SIZE as f32 * METRES_PER_TEXEL * 0.5 * std::f32::consts::SQRT_2;
    shelter.rebuild(
        centre,
        objects.iter().filter_map(|(transform, footprint)| {
            let scale = transform.scale.x.abs();
            let disc = ShelterDisc {
                centre: transform.translation.xz(),
                radius: footprint.half_extent.max_element() * scale * CROWN_RADIUS_SCALE,
                top: transform.translation.y + footprint.height * scale,
            };
            (disc.centre.distance(centre) < reach + disc.radius).then_some(disc)
        }),
    );
    *last = Some(now);
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
        // Published or previewed edits apply live, without restarting the sequence.
        if runtime.settings() != &atmosphere.profile.weather {
            runtime.set_settings(atmosphere.profile.weather.clone());
        }
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
    fn rain_shelters_the_ground_below_placed_objects_only_while_needed() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<RainShelter>()
            .insert_resource(AtmosphereState {
                weather: Some(WeatherKind::Rain.preset()),
                ..default()
            })
            .add_systems(Update, update_rain_shelter);
        app.world_mut()
            .spawn((WorldViewCamera, GlobalTransform::from_xyz(0.0, 2.0, 0.0)));
        app.world_mut().spawn((
            StreamedVisualObject {
                id: world::StableObjectId([1; 16]),
            },
            ObjectFootprint {
                half_extent: Vec2::new(3.0, 2.0),
                height: 10.0,
            },
            Transform::from_xyz(8.0, 1.0, 0.0).with_scale(Vec3::splat(1.2)),
        ));
        app.update();
        let shelter = app.world().resource::<RainShelter>();
        assert!(shelter.enabled);
        assert!(
            shelter.exposure(Vec3::new(8.0, 1.0, 0.0)) < 0.1,
            "ground under the crown"
        );
        assert_eq!(
            shelter.exposure(Vec3::new(8.0, 14.0, 0.0)),
            1.0,
            "above the top"
        );
        assert_eq!(
            shelter.exposure(Vec3::new(-8.0, 1.0, 0.0)),
            1.0,
            "open ground"
        );

        // Dry, settled weather needs no shelter; shaders then treat everything as exposed.
        app.world_mut().resource_mut::<AtmosphereState>().weather =
            Some(WeatherKind::Clear.preset());
        app.update();
        assert!(!app.world().resource::<RainShelter>().enabled);
    }

    #[test]
    fn splashes_surround_what_the_camera_looks_at() {
        // Third-person camera looking down at a character 10 m ahead on flat ground.
        let camera = GlobalTransform::from(
            Transform::from_xyz(0.0, 12.0, 10.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        );
        let (centre, radius) = splash_area(&camera, Some(0.0));
        assert!(
            centre.distance(Vec2::ZERO) < 1.5,
            "centred near the character: {centre}"
        );
        assert_eq!(radius, SPLASH_RADIUS[1]);
        // A low grazing camera covers the ground ahead of it instead of the far horizon.
        let low = GlobalTransform::from(
            Transform::from_xyz(0.0, 1.6, 0.0).looking_to(Vec3::NEG_Z, Vec3::Y),
        );
        let (centre, radius) = splash_area(&low, Some(0.0));
        assert!((centre.y + 10.0).abs() < 1e-3);
        assert!(radius >= SPLASH_RADIUS[0]);
        let capacity =
            SPLASHES_PER_SQUARE_METRE * std::f32::consts::PI * SPLASH_RADIUS[1].powi(2) * 0.3;
        assert!(capacity < atmosphere::precipitation::SPLASH_CAPACITY as f32);
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
