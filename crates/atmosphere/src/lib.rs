//! Shared sky, sun and illumination. Applications supply profile/time inputs;
//! one ordered presentation system applies them before transform propagation.
pub mod adaptation;
pub mod ambient_particles;
pub mod clouds;
pub mod forest_shadow;
pub mod light_shafts;
pub mod lightning;
pub mod precipitation;
pub mod shelter;
pub mod shore;
pub mod sky;
pub mod sun_glare;
pub mod sunlight;
pub mod valley_mist;

use bevy::{
    camera::Exposure,
    core_pipeline::tonemapping::Tonemapping,
    light::{
        Atmosphere, CascadeShadowConfigBuilder, SunDisk,
        atmosphere::{Falloff, ScatteringMedium},
    },
    pbr::AtmosphereSettings,
    post_process::{auto_exposure::AutoExposure, bloom::Bloom},
    prelude::*,
    render::{
        extract_resource::ExtractResource,
        render_resource::{TextureUsages, TextureView},
        view::{ColorGrading, ViewDepthStencilTexture},
    },
};
use std::borrow::Cow;
use world::{
    atmosphere::{AtmosphereProfile, linear_rgb},
    weather::{WeatherFog, WeatherParams, WeatherTransition},
};

/// The world cameras' display transform. Khronos PBR Neutral keeps mid-tones as lit, so sunny
/// ground keeps its colour and contrast where Tony McMapface's compression read as milky haze
/// (docs/EXPERIMENTS.md, October 7).
pub const WORLD_TONEMAPPING: Tonemapping = Tonemapping::KhronosPbrNeutral;

/// Share of colour lost by full night: eyes adapted to moonlight see little colour, so moonlit
/// scenes stay readable without looking like a blue day.
pub const NIGHT_DESATURATION: f32 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtmosphereOwner {
    Game,
    Editor,
    Study,
}

/// Temporary presentation switches. Authored lighting and weather are preserved. Hiding the
/// sky and haze keeps Bevy's atmosphere tables, which also light surfaces; only the sky
/// composite stops drawing them.
#[derive(Resource, Clone, Copy, Debug, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub struct AtmospherePresentation {
    pub sky_and_haze: bool,
    pub bloom: bool,
    /// Eye adaptation around the profile's exposure ([`adaptation`]); game views only.
    pub auto_exposure: bool,
    /// Ground haze and valley mist ([`valley_mist`]); weather fog stays.
    pub low_air: bool,
    /// Dust motes, seed fluff and falling leaves ([`ambient_particles`]).
    pub particles: bool,
    /// Sunbeams through haze, mist and the air under crowns ([`light_shafts`]).
    pub light_shafts: bool,
    /// Sun and moon shadow maps (render quality). Each light renders them only while it is up
    /// and lights the scene: a dark sun's shadow passes cost the night forest about 1.2 ms.
    pub shadows: bool,
}
impl Default for AtmospherePresentation {
    fn default() -> Self {
        Self {
            sky_and_haze: true,
            bloom: true,
            auto_exposure: true,
            low_air: true,
            particles: true,
            light_shafts: true,
            shadows: true,
        }
    }
}

/// The open sea the world view shows, set by the engine: its level in render space, or `None`
/// without one. The sky composite shades the water's waves, reflection and glitter.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct SeaSurface {
    pub level: Option<f32>,
}

/// Scales over the authored ground haze and valley mist, for side-by-side comparisons (look
/// captures). The profile itself is restored from the world whenever it differs.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct FogTuning {
    /// Haze and mist extinction, and mist depth.
    pub haze: f32,
    pub mist: f32,
    pub mist_depth: f32,
    /// Extinction of the air under crowns that light shafts draw.
    pub canopy_air: f32,
}
impl Default for FogTuning {
    fn default() -> Self {
        Self {
            haze: 1.0,
            mist: 1.0,
            mist_depth: 1.0,
            canopy_air: 1.0,
        }
    }
}

#[derive(Resource)]
pub struct AtmosphereState {
    pub profile: AtmosphereProfile,
    pub phase: f32,
    /// Days since the game's first; they move the moon. Editors and studies stay on day 0.
    pub day: i32,
    pub owner: AtmosphereOwner,
    pub direction_override: Option<Vec3>,
    pub exposure_override: Option<f32>,
    /// Weather overlaid on the authored profile: the game's sequence or an editor preview.
    /// None presents the profile as authored; studies never apply it.
    pub weather: Option<WeatherParams>,
    /// Both ends of the current weather change, for the region-by-region cloud field.
    pub weather_transition: Option<WeatherTransition>,
    /// 0..1 surface wetness accumulated by game rain; lags precipitation.
    pub wetness: f32,
    /// A lightning strike in progress, set by the game's weather.
    pub lightning: Option<lightning::LightningFlash>,
}
impl Default for AtmosphereState {
    fn default() -> Self {
        let profile = AtmosphereProfile::default();
        Self {
            phase: profile.initial_phase,
            day: 0,
            profile,
            owner: AtmosphereOwner::Game,
            direction_override: None,
            exposure_override: None,
            weather: None,
            weather_transition: None,
            lightning: None,
            wetness: 0.0,
        }
    }
}
impl AtmosphereState {
    /// Moves the time of day by `days` (back when negative), carrying whole days into `day`.
    pub fn advance(&mut self, days: f32) {
        let time = self.phase + days;
        let mut day = self.day + time.floor() as i32;
        let mut phase = time - time.floor();
        if phase >= 1.0 {
            phase = 0.0;
            day += 1;
        }
        self.day = day;
        self.phase = phase;
    }
    /// Moves to the nearest day on which the moon is about `age` days old at the current time of
    /// day.
    pub fn set_moon_age(&mut self, age: f32) {
        let month = world::atmosphere::SYNODIC_MONTH_DAYS;
        let now = self.profile.night.age_days + (self.day as f32).rem_euclid(month) + self.phase;
        let ahead = (age - now).rem_euclid(month);
        let ahead = if ahead > 0.5 * month {
            ahead - month
        } else {
            ahead
        };
        self.day += ahead.round() as i32;
    }
    /// The atmosphere at the current day and time, for the presented profile.
    pub fn evaluate(&self, profile: &AtmosphereProfile) -> world::atmosphere::EvaluatedAtmosphere {
        world::atmosphere::evaluate_at(profile, self.day, self.phase)
    }
    /// The presented profile: authored, with any game weather overlaid.
    pub fn effective_profile(&self) -> Cow<'_, AtmosphereProfile> {
        match &self.weather {
            Some(weather) if self.owner != AtmosphereOwner::Study => {
                Cow::Owned(weather.apply(&self.profile))
            }
            _ => Cow::Borrowed(&self.profile),
        }
    }
    /// Reduced-visibility fog from game weather, in front of the authored clear-air haze.
    pub fn weather_fog(&self) -> Option<WeatherFog> {
        match &self.weather {
            Some(weather) if self.owner != AtmosphereOwner::Study => {
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
            .init_resource::<FogTuning>()
            .init_resource::<SeaSurface>()
            .add_plugins((
                clouds::CloudsPlugin,
                precipitation::PrecipitationPlugin,
                ambient_particles::AmbientParticlesPlugin,
                sky::SkyCompositePlugin,
                light_shafts::LightShaftsPlugin,
                sun_glare::SunGlarePlugin,
            ))
            .insert_resource(ClearColor(Color::BLACK))
            .insert_resource(AtmosphereState {
                owner: self.owner,
                ..default()
            })
            .insert_resource(ShadowCoverage(self.first_cascade, self.shadow_distance))
            .add_plugins(adaptation::plugin)
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
#[require(sky::SkyCompositeView)]
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
        (&mut Transform, &mut DirectionalLight, &mut SunDisk),
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
            Option<&mut Camera3d>,
            Option<&mut AutoExposure>,
            Option<&mut ColorGrading>,
        ),
        (Without<WorldSun>, Without<WorldMoon>),
    >,
    adaptation: Option<Res<adaptation::Adaptation>>,
    mut planets: Query<(&Atmosphere, &mut GlobalTransform), With<WorldAtmosphere>>,
    handle: Res<MediumHandle>,
    mut media: ResMut<Assets<ScatteringMedium>>,
    mut previous_medium: Local<Option<(f32, f32, [f32; 3])>>,
) {
    if state.owner == AtmosphereOwner::Study {
        // Studies own the shared sun and ambient resources. The world-only moon
        // must go dark as well, including its contribution to custom grass.
        for (_, mut light, _) in &mut moon {
            light.illuminance = 0.0;
            light.shadow_maps_enabled = false;
        }
        return;
    }
    let profile = state.effective_profile();
    let profile = profile.as_ref();
    if profile.validate().is_err() || !state.phase.is_finite() {
        return;
    }
    let value = state.evaluate(profile);
    let direction = state
        .direction_override
        .filter(|v| v.is_finite() && v.length_squared() > 0.01)
        .map(Vec3::normalize)
        .unwrap_or(Vec3::from_array(value.direction_to_sun));
    let presentation = presentation.as_deref().copied().unwrap_or_default();
    // Neither light is ever hidden: Bevy 0.19 loses a directional light's shadow casters once it
    // has been hidden and shown again, so a dark light only stops rendering shadow maps.
    let casts = |direction: Vec3, diameter_degrees: f32, lux: f32| {
        presentation.shadows
            && lux > 0.0
            && direction.y > -(0.5 * diameter_degrees).to_radians().sin()
    };
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
        // After sunset it still lights the sky, but nothing on the ground.
        let shadows = casts(direction, profile.sun_diameter_degrees, light.illuminance);
        if light.shadow_maps_enabled != shadows {
            light.shadow_maps_enabled = shadows;
        }
    }
    for (mut transform, mut light, mut disk) in &mut moon {
        let direction = Vec3::from_array(value.direction_to_moon);
        transform.rotation = Transform::from_translation(direction)
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation;
        light.color = rgb(value.moon_linear);
        light.illuminance = if profile.outdoor { value.moon_lux } else { 0.0 };
        let shadows = casts(direction, profile.night.diameter_degrees, light.illuminance);
        if light.shadow_maps_enabled != shadows {
            light.shadow_maps_enabled = shadows;
        }
        // The sky composite draws the moon's face and phase itself.
        disk.angular_size = profile.night.diameter_degrees.to_radians();
        disk.intensity = 0.0;
    }
    ambient.color = rgb(value.ambient_linear);
    // A flash lights everything around from the clouds, bluish white.
    let flash = state.lightning.map_or(0.0, |l| l.flash);
    ambient.brightness = value.ambient_lux + flash * lightning::FLASH_AMBIENT;
    if flash > 0.0 {
        let base = Vec3::from_array(value.ambient_linear) * value.ambient_lux;
        let lit = (base
            + Vec3::from_array(lightning::FLASH_COLOR) * flash * lightning::FLASH_AMBIENT)
            / ambient.brightness.max(1e-3);
        ambient.color = Color::linear_rgb(lit.x, lit.y, lit.z);
    }
    // Eye adaptation in the game only: authoring and studies judge the exposure as set.
    let adapt =
        presentation.auto_exposure && state.owner == AtmosphereOwner::Game && profile.outdoor;
    let saturation = if profile.outdoor {
        1.0 - NIGHT_DESATURATION * value.adaptation
    } else {
        1.0
    };
    for (entity, camera, view, mut exposure, settings, bloom, camera_3d, mut auto, grading) in
        &mut views
    {
        match grading {
            Some(mut grading) => {
                if grading.global.post_saturation != saturation {
                    grading.global.post_saturation = saturation;
                }
            }
            None => {
                let mut grading = ColorGrading::default();
                grading.global.post_saturation = saturation;
                commands.entity(entity).insert(grading);
            }
        }
        exposure.ev100 = state
            .exposure_override
            .filter(|v| v.is_finite())
            .unwrap_or(value.exposure_ev100);
        if profile.outdoor && settings.is_none() {
            commands
                .entity(entity)
                .insert(AtmosphereSettings::default());
        }
        if !profile.outdoor && settings.is_some() {
            commands.entity(entity).remove::<AtmosphereSettings>();
        }
        // The sky composite reads depth whether or not the atmosphere is present; Bevy only
        // requests sampling for atmosphere views.
        if let Some(mut camera_3d) = camera_3d {
            let usages = TextureUsages::from(camera_3d.depth_texture_usages);
            if !usages.contains(TextureUsages::TEXTURE_BINDING) {
                camera_3d.depth_texture_usages = (usages | TextureUsages::TEXTURE_BINDING).into();
            }
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
        if let Some(adaptation) = &adaptation {
            match auto.as_deref_mut() {
                Some(auto) if auto.compensation_curve != *adaptation.curve(adapt) => {
                    auto.compensation_curve = adaptation.curve(adapt).clone();
                }
                Some(_) => {}
                None if adapt => {
                    commands.entity(entity).insert(adaptation.settings(adapt));
                }
                None => {}
            }
        }
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

/// The view of a camera's depth texture that passes sample. Since Bevy 0.20 a depth texture may
/// carry a stencil aspect, which a binding cannot include.
pub fn sampled_depth(depth: &ViewDepthStencilTexture) -> &TextureView {
    depth
        .attachment
        .depth_stencil_views()
        .depth_only_view()
        .expect("view depth textures have a depth aspect")
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
        // At night only the moon renders shadow maps; neither light is ever hidden.
        let shadows = |app: &App, light: Entity| {
            app.world()
                .get::<DirectionalLight>(light)
                .unwrap()
                .shadow_maps_enabled
        };
        assert!(shadows(&app, moon) && !shadows(&app, sun));
        app.world_mut().resource_mut::<AtmosphereState>().phase = 0.5;
        app.update();
        assert!(shadows(&app, sun) && !shadows(&app, moon));
        app.world_mut().resource_mut::<AtmosphereState>().phase = 0.0;
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Study;
        app.update();
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(moon)
                .unwrap()
                .illuminance,
            0.0
        );
        assert!(!shadows(&app, moon));
        app.world_mut().resource_mut::<AtmosphereState>().owner = AtmosphereOwner::Editor;
        app.world_mut()
            .resource_mut::<AtmospherePresentation>()
            .shadows = false;
        app.update();
        assert!(!shadows(&app, moon) && !shadows(&app, sun));
        assert!(
            app.world()
                .get::<DirectionalLight>(moon)
                .unwrap()
                .illuminance
                > 0.0
        );
        app.world_mut()
            .resource_mut::<AtmosphereState>()
            .profile
            .outdoor = false;
        app.update();
        assert_eq!(
            app.world()
                .get::<DirectionalLight>(moon)
                .unwrap()
                .illuminance,
            0.0
        );
        for light in [sun, moon] {
            assert_ne!(
                app.world().get::<Visibility>(light),
                Some(&Visibility::Hidden)
            );
        }
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
            auto_exposure: false,
            low_air: false,
            particles: false,
            light_shafts: false,
            shadows: false,
        };
        app.update();
        // Surfaces keep atmosphere lighting; the composite alone stops drawing sky and haze.
        assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
        assert!(app.world().get::<Bloom>(camera).is_none());
        assert_eq!(app.world().resource::<AtmosphereState>().profile, profile);
        *app.world_mut().resource_mut::<AtmospherePresentation>() =
            AtmospherePresentation::default();
        app.update();
        assert!(app.world().get::<AtmosphereSettings>(camera).is_some());
        assert!(app.world().get::<Bloom>(camera).is_some());
    }

    #[test]
    fn study_owns_lighting_and_world_return_restores_profile_and_shadow_policy() {
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
            .resource_mut::<AtmospherePresentation>()
            .shadows = false;
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
