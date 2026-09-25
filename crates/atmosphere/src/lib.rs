//! Shared sky, sun and illumination. Applications supply profile/time inputs;
//! one ordered presentation system applies them before transform propagation.
pub mod clouds;
pub mod precipitation;
pub mod shelter;

use bevy::{
    camera::Exposure,
    light::{
        Atmosphere, CascadeShadowConfigBuilder, SunDisk,
        atmosphere::{Falloff, ScatteringMedium},
    },
    pbr::AtmosphereSettings,
    post_process::bloom::Bloom,
    prelude::*,
};
use std::borrow::Cow;
use world::{
    atmosphere::{AtmosphereProfile, evaluate, linear_rgb},
    weather::{WeatherFog, WeatherParams, WeatherTransition},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmosphereOwner {
    Game,
    Editor,
    Study,
}

/// Temporary presentation switches. Authored lighting and weather are preserved.
#[derive(Resource, Clone, Copy, Debug)]
pub struct AtmospherePresentation {
    pub sky_and_haze: bool,
    pub bloom: bool,
}
impl Default for AtmospherePresentation {
    fn default() -> Self {
        Self {
            sky_and_haze: true,
            bloom: true,
        }
    }
}

#[derive(Resource)]
pub struct AtmosphereState {
    pub profile: AtmosphereProfile,
    pub phase: f32,
    pub owner: AtmosphereOwner,
    pub direction_override: Option<Vec3>,
    pub exposure_override: Option<f32>,
    /// Game weather overlaid on the authored profile. None presents the profile as authored.
    pub weather: Option<WeatherParams>,
    /// Both ends of the current weather change, for the region-by-region cloud field.
    pub weather_transition: Option<WeatherTransition>,
    /// 0..1 surface wetness accumulated by game rain; lags precipitation.
    pub wetness: f32,
}
impl Default for AtmosphereState {
    fn default() -> Self {
        let profile = AtmosphereProfile::default();
        Self {
            phase: profile.initial_phase,
            profile,
            owner: AtmosphereOwner::Game,
            direction_override: None,
            exposure_override: None,
            weather: None,
            weather_transition: None,
            wetness: 0.0,
        }
    }
}
impl AtmosphereState {
    /// The presented profile: authored, with any game weather overlaid.
    pub fn effective_profile(&self) -> Cow<'_, AtmosphereProfile> {
        match &self.weather {
            Some(weather) if self.owner == AtmosphereOwner::Game => {
                Cow::Owned(weather.apply(&self.profile))
            }
            _ => Cow::Borrowed(&self.profile),
        }
    }
    /// Reduced-visibility fog from game weather, in front of the authored clear-air haze.
    pub fn weather_fog(&self) -> Option<WeatherFog> {
        match &self.weather {
            Some(weather) if self.owner == AtmosphereOwner::Game => {
                Some(weather.fog(&self.profile))
            }
            _ => None,
        }
    }
}
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApplyAtmosphere;

pub struct WorldEnvironmentPlugin {
    first_cascade: f32,
    shadow_distance: f32,
    owner: AtmosphereOwner,
}
impl WorldEnvironmentPlugin {
    pub const fn game() -> Self {
        Self {
            first_cascade: 20.0,
            shadow_distance: 80.0,
            owner: AtmosphereOwner::Game,
        }
    }
    pub const fn editor() -> Self {
        Self {
            first_cascade: 60.0,
            shadow_distance: 300.0,
            owner: AtmosphereOwner::Editor,
        }
    }
}
impl Plugin for WorldEnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AtmospherePresentation>()
            .add_plugins((clouds::CloudsPlugin, precipitation::PrecipitationPlugin))
            .insert_resource(ClearColor(Color::BLACK))
            .insert_resource(AtmosphereState {
                owner: self.owner,
                ..default()
            })
            .insert_resource(ShadowCoverage(self.first_cascade, self.shadow_distance))
            .add_systems(Startup, setup)
            .configure_sets(
                PostUpdate,
                ApplyAtmosphere.before(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(PostUpdate, apply.in_set(ApplyAtmosphere));
    }
}
#[derive(Resource)]
struct ShadowCoverage(f32, f32);
#[derive(Component)]
pub struct WorldSun;
#[derive(Component)]
pub struct WorldMoon;
#[derive(Component, Default)]
pub struct WorldEnvironmentView {
    /// Explicit launch/bookmark override, displayed and clearable by the editor.
    pub visibility_override: Option<f32>,
}
#[derive(Bundle)]
pub struct WorldEnvironmentCamera {
    exposure: Exposure,
    view: WorldEnvironmentView,
}
impl Default for WorldEnvironmentCamera {
    fn default() -> Self {
        Self {
            exposure: Exposure { ev100: 13.0 },
            view: default(),
        }
    }
}
impl WorldEnvironmentCamera {
    pub fn with_visibility(visibility: f32) -> Self {
        Self {
            view: WorldEnvironmentView {
                visibility_override: Some(visibility),
            },
            ..default()
        }
    }
}
#[derive(Component)]
struct WorldAtmosphere;
#[derive(Resource)]
struct MediumHandle(Handle<ScatteringMedium>);

fn setup(
    mut commands: Commands,
    coverage: Res<ShadowCoverage>,
    mut media: ResMut<Assets<ScatteringMedium>>,
) {
    commands.insert_resource(GlobalAmbientLight::default());
    let medium = media.add(ScatteringMedium::earth(256, 128));
    commands.spawn((Atmosphere::earth(medium.clone()), WorldAtmosphere));
    commands.insert_resource(MediumHandle(medium));
    commands.spawn((
        DirectionalLight {
            illuminance: 128_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        SunDisk::EARTH,
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            first_cascade_far_bound: coverage.0,
            maximum_distance: coverage.1,
            ..default()
        }
        .build(),
        Transform::from_xyz(1.0, 1.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
        WorldSun,
        Name::new("Sun"),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 0.0,
            shadow_maps_enabled: true,
            ..default()
        },
        SunDisk::EARTH,
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            first_cascade_far_bound: coverage.0,
            maximum_distance: coverage.1,
            ..default()
        }
        .build(),
        Transform::default(),
        Visibility::Hidden,
        WorldMoon,
        Name::new("Moon"),
    ));
}

fn rgb(c: [f32; 3]) -> Color {
    Color::linear_rgb(c[0], c[1], c[2])
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn apply(
    mut commands: Commands,
    state: Res<AtmosphereState>,
    presentation: Option<Res<AtmospherePresentation>>,
    mut sun: Query<
        (&mut Transform, &mut DirectionalLight, &mut SunDisk),
        (With<WorldSun>, Without<WorldMoon>),
    >,
    mut moon: Query<
        (
            &mut Transform,
            &mut DirectionalLight,
            &mut SunDisk,
            &mut Visibility,
        ),
        (With<WorldMoon>, Without<WorldSun>),
    >,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut views: Query<
        (
            Entity,
            &Transform,
            &WorldEnvironmentView,
            &mut Exposure,
            Option<&AtmosphereSettings>,
            Option<&mut Bloom>,
        ),
        (Without<WorldSun>, Without<WorldMoon>),
    >,
    mut planets: Query<(&Atmosphere, &mut GlobalTransform), With<WorldAtmosphere>>,
    handle: Res<MediumHandle>,
    mut media: ResMut<Assets<ScatteringMedium>>,
    mut previous_medium: Local<Option<(f32, f32, [f32; 3])>>,
) {
    if state.owner == AtmosphereOwner::Study {
        // Studies own the shared sun and ambient resources. The world-only moon
        // must disappear as well, including its contribution to custom grass.
        for (_, mut light, _, mut visibility) in &mut moon {
            light.illuminance = 0.0;
            *visibility = Visibility::Hidden;
        }
        return;
    }
    let profile = state.effective_profile();
    let profile = profile.as_ref();
    if profile.validate().is_err() || !state.phase.is_finite() {
        return;
    }
    let value = evaluate(profile, state.phase);
    let direction = state
        .direction_override
        .filter(|v| v.is_finite() && v.length_squared() > 0.01)
        .map(Vec3::normalize)
        .unwrap_or(Vec3::from_array(value.direction_to_sun));
    let mut shadows_enabled = false;
    for (mut transform, mut light, mut disk) in &mut sun {
        let up = if direction.y.abs() > 0.999 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        transform.rotation = Transform::from_translation(direction)
            .looking_at(Vec3::ZERO, up)
            .rotation;
        light.color = rgb(value.sun_linear);
        light.illuminance = if profile.outdoor { value.sun_lux } else { 0.0 };
        disk.angular_size = profile.sun_diameter_degrees.to_radians();
        shadows_enabled = light.shadow_maps_enabled;
        // Shadow enablement belongs to render quality/audits, not the atmosphere profile.
    }
    for (mut transform, mut light, mut disk, mut visibility) in &mut moon {
        transform.rotation = Transform::from_translation(Vec3::from_array(value.direction_to_moon))
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation;
        light.color = rgb(value.moon_linear);
        light.illuminance = if profile.outdoor { value.moon_lux } else { 0.0 };
        light.shadow_maps_enabled = shadows_enabled;
        disk.angular_size = profile.night.diameter_degrees.to_radians();
        *visibility = if light.illuminance > 0.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    ambient.color = rgb(value.ambient_linear);
    ambient.brightness = value.ambient_lux;
    let presentation = presentation.as_deref().copied().unwrap_or_default();
    let sky_enabled = profile.outdoor && presentation.sky_and_haze;
    for (entity, camera, view, mut exposure, settings, bloom) in &mut views {
        exposure.ev100 = state
            .exposure_override
            .filter(|v| v.is_finite())
            .unwrap_or(value.exposure_ev100);
        if sky_enabled && settings.is_none() {
            commands
                .entity(entity)
                .insert(AtmosphereSettings::default());
        }
        if !sky_enabled && settings.is_some() {
            commands.entity(entity).remove::<AtmosphereSettings>();
        }
        if !presentation.bloom {
            if bloom.is_some() {
                commands.entity(entity).remove::<Bloom>();
            }
        } else if let Some(mut bloom) = bloom {
            bloom.intensity = profile.bloom_intensity;
        } else {
            commands.entity(entity).insert(Bloom {
                intensity: profile.bloom_intensity,
                ..Bloom::NATURAL
            });
        }
        // The atmosphere handles both sky and aerial perspective. No second DistanceFog pass.
        commands.entity(entity).remove::<DistanceFog>();
        let visibility = view
            .visibility_override
            .filter(|v| v.is_finite())
            .unwrap_or(profile.visibility_metres)
            .clamp(10.0, 100_000.0);
        let signature = (profile.molecular_density, visibility, profile.haze_srgb);
        if previous_medium.as_ref() != Some(&signature) {
            let mut medium = ScatteringMedium::earth(256, 128);
            medium.terms[0].scattering *= profile.molecular_density;
            // Additional near-ground aerosol layer; extinction corresponds to 2% contrast
            // at the authored visibility. This is separate from high-altitude molecular air.
            let extinction = 3.912 / visibility;
            let tint = Vec3::from_array(linear_rgb(profile.haze_srgb));
            medium.terms[1].scattering = tint * extinction * 0.95;
            medium.terms[1].absorption = Vec3::splat(extinction) - medium.terms[1].scattering;
            medium.terms[1].falloff = Falloff::Exponential {
                scale: 1200.0 / 100_000.0,
            };
            if let Some(mut asset) = media.get_mut(&handle.0) {
                *asset = medium;
            }
            *previous_medium = Some(signature);
        }
        // A local tangent atmosphere follows horizontal origin shifts. Absolute Y is
        // unchanged by the engine's XZ rebasing, preserving observer altitude and horizon.
        for (planet, mut transform) in &mut planets {
            *transform = GlobalTransform::from_translation(Vec3::new(
                camera.translation.x,
                -planet.inner_radius,
                camera.translation.z,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn night_lights_surfaces_and_studies_and_interiors_disable_the_moon() {
        let mut app = App::new();
        app.init_resource::<Assets<ScatteringMedium>>()
            .add_plugins(WorldEnvironmentPlugin::editor());
        let camera = app
            .world_mut()
            .spawn((Transform::default(), WorldEnvironmentCamera::default()))
            .id();
        app.world_mut().resource_mut::<AtmosphereState>().phase = 0.0;
        app.update();
        let moon = app
            .world_mut()
            .query_filtered::<Entity, With<WorldMoon>>()
            .single(app.world())
            .unwrap();
        let sun = app
            .world_mut()
            .query_filtered::<Entity, With<WorldSun>>()
            .single(app.world())
            .unwrap();
        assert!(
            app.world()
                .get::<DirectionalLight>(moon)
                .unwrap()
                .illuminance
                > 0.0
        );
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .illuminance,
            0.0
        );
        assert_eq!(
            app.world().get::<Exposure>(camera).unwrap().ev100,
            world::atmosphere::NightLighting::default().exposure_ev100
        );
        app.world_mut()
            .resource_mut::<AtmosphereState>()
            .exposure_override = Some(13.0);
        app.update();
        assert_eq!(app.world().get::<Exposure>(camera).unwrap().ev100, 13.0);
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Study;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(moon),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(moon)
                .unwrap()
                .illuminance,
            0.0
        );
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Editor;
        app.world_mut()
            .get_mut::<DirectionalLight>(sun)
            .unwrap()
            .shadow_maps_enabled = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(moon),
            Some(&Visibility::Visible)
        );
        assert!(
            !app.world()
                .get::<DirectionalLight>(moon)
                .unwrap()
                .shadow_maps_enabled
        );
        app.world_mut()
            .resource_mut::<AtmosphereState>()
            .profile
            .outdoor = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(moon),
            Some(&Visibility::Hidden)
        );
    }
    #[test]
    fn presentation_switches_remove_passes_and_restore_without_editing_profile() {
        let mut app = App::new();
        app.init_resource::<Assets<ScatteringMedium>>()
            .add_plugins(WorldEnvironmentPlugin::game());
        let camera = app
            .world_mut()
            .spawn((Transform::default(), WorldEnvironmentCamera::default()))
            .id();
        app.update();
        assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
        assert!(app.world().get::<Bloom>(camera).is_some());
        let profile = app.world().resource::<AtmosphereState>().profile.clone();
        *app.world_mut().resource_mut::<AtmospherePresentation>() = AtmospherePresentation {
            sky_and_haze: false,
            bloom: false,
        };
        app.update();
        assert!(app.world().get::<AtmosphereSettings>(camera).is_none());
        assert!(app.world().get::<Bloom>(camera).is_none());
        assert_eq!(app.world().resource::<AtmosphereState>().profile, profile);
        *app.world_mut().resource_mut::<AtmospherePresentation>() =
            AtmospherePresentation::default();
        app.update();
        assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
        assert!(app.world().get::<Bloom>(camera).is_some());
    }

    #[test]
    fn study_owns_lighting_and_world_return_restores_profile_without_changing_shadow_policy() {
        let mut app = App::new();
        app.init_resource::<Assets<ScatteringMedium>>()
            .add_plugins(WorldEnvironmentPlugin::editor());
        let camera = app
            .world_mut()
            .spawn((
                Transform::from_xyz(20.0, 3.0, 40.0),
                WorldEnvironmentCamera::with_visibility(1200.0),
            ))
            .id();
        app.update();
        let sun = app
            .world_mut()
            .query_filtered::<Entity, With<WorldSun>>()
            .single(app.world())
            .unwrap();
        assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Study;
        app.world_mut()
            .get_mut::<DirectionalLight>(sun)
            .unwrap()
            .illuminance = 1234.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .illuminance,
            1234.0
        );
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Editor;
        app.world_mut()
            .get_mut::<DirectionalLight>(sun)
            .unwrap()
            .shadow_maps_enabled = false;
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .illuminance
                > 100_000.0
        );
        assert!(
            !app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadow_maps_enabled
        );
        app.world_mut()
            .resource_mut::<AtmosphereState>()
            .profile
            .outdoor = false;
        app.update();
        assert!(app.world().get::<AtmosphereSettings>(camera).is_none());
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .illuminance,
            0.0
        );
    }
}
