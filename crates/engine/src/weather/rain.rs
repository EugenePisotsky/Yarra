//! Rain on the ground around the camera: splashes where drops land and the shelter map of
//! what keeps the ground below dry.
use crate::{
    ActiveWorldView, AtmosphereOwner, AtmosphereState, ObjectFootprint, StreamedTerrainSurface,
    StreamedVisualObject, WorldOrigin, sample_resident_terrain_surface,
};
use atmosphere::{
    precipitation::{PrecipitationReference, RainSplashes},
    shelter::{METRES_PER_TEXEL, RainShelter, SIZE, ShelterDisc},
};
use bevy::prelude::*;

/// Impacts per square metre per second at full precipitation, around the ground the camera
/// looks at. The spawn disc is capped so the 512-entry buffer covers every live splash.
const SPLASHES_PER_SQUARE_METRE: f32 = 5.0;
const SPLASH_RADIUS: [f32; 2] = [6.0, 10.0];

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
pub(super) fn update_precipitation_reference(
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

/// Small generator for splash placement; variety, not statistical quality.
#[derive(Default)]
pub(super) struct SplashRandom(u64);
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
pub(super) fn spawn_rain_splashes(
    time: Res<Time>,
    atmosphere: Res<AtmosphereState>,
    shelter: Res<RainShelter>,
    origin: Option<Res<WorldOrigin>>,
    view: ActiveWorldView,
    target: Query<&Transform, With<crate::actor::CameraTarget>>,
    surfaces: Query<&StreamedTerrainSurface>,
    (splashes, sea): (
        Option<ResMut<RainSplashes>>,
        Option<Res<atmosphere::SeaSurface>>,
    ),
    mut state: Local<(SplashRandom, f32)>,
) {
    let camera = view.current().map(|view| view.transform);
    let (Some(mut splashes), Some(origin), Some(camera)) = (splashes, origin, camera) else {
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
        // Drops falling on the sea leave no splash on the seabed below it.
        if ground.normal[1] < 0.6 || sea.as_ref().and_then(|s| s.level) > Some(ground.height) {
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
pub(super) fn update_rain_shelter(
    time: Res<Time<Real>>,
    atmosphere: Res<AtmosphereState>,
    origin: Option<Res<WorldOrigin>>,
    view: ActiveWorldView,
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
    let Some(camera) = view.current().map(|view| view.transform).filter(|_| needed) else {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WeatherKind, WorldViewCamera};

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
        app.world_mut().spawn((
            WorldViewCamera,
            Camera::default(),
            GlobalTransform::from_xyz(0.0, 2.0, 0.0),
        ));
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
}
