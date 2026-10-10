//! The light and wind every vegetation species responds to: the world's directional light and
//! ambient fill, the shared lighting response, the analytic wind field and exposure gain.
use bevy::{
    color::LinearRgba, light::SunDisk, prelude::*, render::extract_resource::ExtractResource,
};

/// Global environment response shared by every vegetation species.
///
/// Species keep their own colors, roughness, transmission, and AO. These values tune how strongly
/// the renderer applies the world's directional light and its received shadows to that authored
/// material response.
#[derive(Resource, ExtractResource, Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationLighting {
    pub diffuse_strength: f32,
    pub specular_strength: f32,
    pub transmission_strength: f32,
    pub received_shadow_strength: f32,
    #[serde(default)]
    pub canopy: vegetation::CanopyShading,
    #[serde(skip)]
    pub canopy_origin: [f32; 2],
}

impl Default for VegetationLighting {
    fn default() -> Self {
        Self {
            diffuse_strength: 0.82,
            specular_strength: 0.28,
            transmission_strength: 0.34,
            received_shadow_strength: 0.78,
            canopy: Default::default(),
            canopy_origin: [0.0; 2],
        }
    }
}

/// Shared low-frequency wind field used by procedural vegetation.
///
/// The field is intentionally analytic so CPU gameplay and GPU rendering can sample the same
/// travelling wave without a texture dependency. Ribbons use bounded rotations of their resting
/// curves; broad leaves retain longitudinal detail. This resource supplies the shared field.
#[derive(Resource, ExtractResource, Debug, Clone, Copy)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationWind {
    /// An external transport (editor previews, tests) owns phase and disables diagnostic
    /// keyboard shortcuts.
    /// Default false preserves the game clock and controls.
    pub externally_driven: bool,
    pub enabled: bool,
    /// Horizontal direction in world XZ coordinates.
    pub direction: Vec2,
    /// Maximum horizontal tip displacement as a fraction of blade height.
    pub strength: f32,
    /// Broad-wave spatial frequency in radians per world unit.
    pub spatial_frequency: f32,
    /// Travelling-wave angular speed in radians per second.
    pub speed: f32,
    /// Relative strength of the slower gust layer.
    pub gustiness: f32,
    /// Grass-only hashed flutter amplitude as a fraction of blade height.
    pub flutter: f32,
    /// Clock multiplier for every travelling wave and flutter. Weather changes this rather than
    /// `speed`: phase is `time * speed`, so varying `speed` would jump the whole field.
    pub rate: f32,
    pub(crate) phase_seconds: f32,
}

impl Default for VegetationWind {
    fn default() -> Self {
        Self {
            externally_driven: false,
            enabled: true,
            direction: Vec2::new(0.92, 0.38).normalize(),
            strength: 0.82,
            spatial_frequency: 0.12,
            speed: 2.4,
            gustiness: 0.95,
            flutter: 0.28,
            rate: 1.0,
            phase_seconds: 0.0,
        }
    }
}

impl VegetationWind {
    /// Synchronize procedural wind for a deterministic replay or shared weather clock.
    pub fn set_phase_seconds(&mut self, seconds: f32) {
        assert!(seconds.is_finite(), "wind phase must be finite");
        self.phase_seconds = seconds.rem_euclid(4096.0);
    }

    /// Samples the coherent scalar push used by non-rendering systems.
    pub fn sample_force(&self, world_xz: Vec2) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let direction = self.direction.try_normalize().unwrap_or(Vec2::X);
        let cross_direction = Vec2::new(-direction.y, direction.x);
        let frequency = self.spatial_frequency.max(0.001);
        let speed = self.speed.max(0.0);
        let broad_phase = world_xz.dot(direction) * frequency - self.phase_seconds * speed;
        let cross_phase =
            world_xz.dot(cross_direction) * frequency * 0.71 + self.phase_seconds * speed * 0.37;
        let broad_wave = (broad_phase + cross_phase.sin() * 0.85).sin();
        let gust_wave = (broad_phase * 0.43 - cross_phase * 0.61).sin();
        let broad_amount = broad_wave * 0.5 + 0.5;
        let gust_coordinate = (gust_wave * 0.5 + 0.5).clamp(0.0, 1.0);
        let gust_rise = ((gust_coordinate - 0.28) / 0.72).clamp(0.0, 1.0);
        let gust_pulse = gust_rise * gust_rise * (3.0 - 2.0 * gust_rise);
        self.strength.max(0.0)
            * (0.25 + broad_amount * 0.18 + gust_pulse * self.gustiness.clamp(0.0, 1.0) * 0.90)
                .clamp(0.12, 1.30)
    }

    /// Current procedural wind phase, in seconds.
    pub fn phase_seconds(self) -> f32 {
        self.phase_seconds
    }
}

pub(crate) fn advance_vegetation_wind(time: Res<Time>, mut wind: ResMut<VegetationWind>) {
    if wind.externally_driven {
        return;
    }
    // Bound hitch recovery so a paused debugger does not produce a single violent deformation.
    // The rate bound keeps one frame's advance below the grass history-reset threshold.
    let rate = if wind.rate.is_finite() {
        wind.rate.clamp(0.0, 2.0)
    } else {
        1.0
    };
    wind.phase_seconds =
        (wind.phase_seconds + time.delta_secs().min(0.1) * rate).rem_euclid(4096.0);
}

/// Exposure adaptation relative to the authored scene. Grass bounds its ambient fill in exposed
/// units for the stylized clear-day look; weather that opens exposure under cloud raises that
/// bound by the same factor, so grass keeps pace with PBR terrain, trees and characters.
#[derive(Resource, ExtractResource, Debug, Clone, Copy, PartialEq)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationAmbientGain(pub f32);
impl Default for VegetationAmbientGain {
    fn default() -> Self {
        Self(1.0)
    }
}

/// Render-facing snapshot of the strongest directional light and the global ambient fill.
#[derive(Resource, ExtractResource, Debug, Clone, Copy)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct VegetationSun {
    pub(crate) direction_to_light: Vec3,
    pub(crate) radiance: Vec3,
    pub(crate) ambient_radiance: Vec3,
    pub(crate) active: bool,
}

impl Default for VegetationSun {
    fn default() -> Self {
        Self {
            direction_to_light: Vec3::Y,
            radiance: Vec3::ONE,
            ambient_radiance: Vec3::ONE,
            active: false,
        }
    }
}

pub(crate) fn sync_vegetation_sun(
    directional_lights: Query<(
        &DirectionalLight,
        &GlobalTransform,
        Option<&SunDisk>,
        Option<&Visibility>,
    )>,
    ambient: Option<Res<GlobalAmbientLight>>,
    mut vegetation_sun: ResMut<VegetationSun>,
) {
    if let Some(ambient) = ambient {
        let color = LinearRgba::from(ambient.color);
        vegetation_sun.ambient_radiance =
            Vec3::new(color.red, color.green, color.blue) * ambient.brightness.max(0.0);
    } else {
        vegetation_sun.ambient_radiance = Vec3::ONE;
    }

    let Some((light, transform, _, _)) = directional_lights
        .iter()
        .filter(|(light, transform, disk, visibility)| {
            light.illuminance.is_finite()
                && light.illuminance > 0.0
                && visibility.is_none_or(|v| *v != Visibility::Hidden)
                // Celestial lights below the ground still illuminate the sky at twilight,
                // but must not steal surface lighting from the moon above the horizon.
                && disk.is_none_or(|d| transform.back().y > -(d.angular_size * 0.5).sin())
        })
        .max_by(|(left, ..), (right, ..)| left.illuminance.total_cmp(&right.illuminance))
    else {
        vegetation_sun.active = false;
        vegetation_sun.radiance = Vec3::ZERO;
        return;
    };
    let color = LinearRgba::from(light.color);
    vegetation_sun.direction_to_light = transform.back().into();
    vegetation_sun.radiance =
        Vec3::new(color.red, color.green, color.blue) * light.illuminance.max(0.0);
    vegetation_sun.active = light.illuminance > 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VegetationDensityMode, VegetationSettings};

    #[test]
    fn external_transport_keeps_exact_wind_phase() {
        let mut app = App::new();
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        let mut wind = VegetationWind {
            externally_driven: true,
            ..default()
        };
        wind.set_phase_seconds(7.125);
        app.insert_resource(time)
            .insert_resource(wind)
            .init_resource::<VegetationSettings>()
            .add_systems(Update, advance_vegetation_wind);
        app.update();
        let wind = app.world().resource::<VegetationWind>();
        assert_eq!(wind.phase_seconds(), 7.125);
        assert!(wind.enabled);
        assert_eq!(
            app.world().resource::<VegetationSettings>().density_mode,
            VegetationDensityMode::Balanced
        );
        app.world_mut()
            .resource_mut::<VegetationWind>()
            .externally_driven = false;
        app.update();
        assert!(app.world().resource::<VegetationWind>().phase_seconds() > 7.125);
    }

    #[test]
    fn wind_rate_scales_the_clock_and_is_bounded() {
        let advance = |rate: f32, delta: f32| {
            let mut app = App::new();
            let mut time: Time = Time::default();
            time.advance_by(std::time::Duration::from_secs_f32(delta));
            let mut wind = VegetationWind { rate, ..default() };
            wind.set_phase_seconds(10.0);
            app.insert_resource(time)
                .insert_resource(wind)
                .add_systems(Update, advance_vegetation_wind);
            app.update();
            app.world().resource::<VegetationWind>().phase_seconds() - 10.0
        };
        assert!((advance(1.0, 0.05) - 0.05).abs() < 1e-5);
        assert!((advance(1.5, 0.05) - 0.075).abs() < 1e-5);
        assert_eq!(advance(0.0, 0.05), 0.0);
        // Hitches and invalid rates stay below the 0.25 s grass history-reset threshold.
        assert!((advance(5.0, 1.0) - 0.2).abs() < 1e-5);
        assert!((advance(f32::NAN, 0.05) - 0.05).abs() < 1e-5);
    }

    #[test]
    fn default_wind_is_strong_coherent_and_cpu_sampleable() {
        let wind = VegetationWind::default();
        assert!(wind.enabled);
        assert!(wind.strength >= 0.8);
        assert!(wind.gustiness >= 0.9);
        assert!(wind.flutter >= 0.25);
        assert!((wind.direction.length() - 1.0).abs() < 1e-5);
        assert!(wind.sample_force(Vec2::new(12.0, -8.0)).is_finite());

        let mut disabled = wind;
        disabled.enabled = false;
        assert_eq!(disabled.sample_force(Vec2::ZERO), 0.0);
    }

    #[test]
    fn moon_above_horizon_wins_over_twilight_sun_and_hidden_lights_are_excluded() {
        let mut app = App::new();
        app.init_resource::<VegetationSun>()
            .add_systems(Update, sync_vegetation_sun);
        app.world_mut().spawn((
            DirectionalLight {
                illuminance: 100_000.0,
                ..default()
            },
            GlobalTransform::from(
                Transform::from_xyz(1.0, -0.5, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
            ),
            SunDisk::EARTH,
        ));
        let moon = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    illuminance: 1800.0,
                    ..default()
                },
                GlobalTransform::from(
                    Transform::from_xyz(-1.0, 0.5, 0.0).looking_at(Vec3::ZERO, Vec3::Y),
                ),
                SunDisk::EARTH,
            ))
            .id();
        app.update();
        let light = app.world().resource::<VegetationSun>();
        assert!(light.active);
        assert!(light.direction_to_light.y > 0.0);
        assert_eq!(light.radiance, Vec3::splat(1800.0));
        app.world_mut().entity_mut(moon).insert(Visibility::Hidden);
        app.update();
        assert!(!app.world().resource::<VegetationSun>().active);
        assert_eq!(app.world().resource::<VegetationSun>().radiance, Vec3::ZERO);
    }
}
