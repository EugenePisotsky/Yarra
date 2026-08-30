use bevy::{
    camera::Exposure,
    light::{CascadeShadowConfigBuilder, NotShadowCaster, NotShadowReceiver, light_consts::lux},
    prelude::*,
};

// A low but not horizon-grazing sun remains visible in the near camera while keeping character and
// tree shadows within useful cascade coverage.
const SUN_POSITION: Vec3 = Vec3::new(7.878_527_6, 3.0, -12.131_867);
const SUN_VISUAL_DISTANCE: f32 = 80.0;
const SUN_VISUAL_RADIUS: f32 = 1.30;
const FOG_VISIBILITY_DISTANCE: f32 = 420.0;
const SKY_COLOR: Color = Color::srgb(0.43, 0.56, 0.68);

/// Installs the shared low-cost sky, distance fog, ambient fill, and world sun.
///
/// Game and editor use the same lighting model; only their directional-shadow coverage differs.
pub struct WorldEnvironmentPlugin {
    first_cascade_far_bound: f32,
    maximum_shadow_distance: f32,
}

impl WorldEnvironmentPlugin {
    pub const fn game() -> Self {
        Self {
            first_cascade_far_bound: 20.0,
            maximum_shadow_distance: 80.0,
        }
    }

    pub const fn editor() -> Self {
        Self {
            first_cascade_far_bound: 60.0,
            maximum_shadow_distance: 300.0,
        }
    }
}

impl Plugin for WorldEnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(SKY_COLOR))
            .insert_resource(WorldEnvironmentConfig {
                first_cascade_far_bound: self.first_cascade_far_bound,
                maximum_shadow_distance: self.maximum_shadow_distance,
            })
            .add_systems(Startup, setup_world_environment)
            .add_systems(PostUpdate, update_sun_visual);
    }
}

#[derive(Resource)]
struct WorldEnvironmentConfig {
    first_cascade_far_bound: f32,
    maximum_shadow_distance: f32,
}

/// Marker for the directional light that drives the sky and foliage lighting.
#[derive(Component)]
pub struct WorldSun;

/// Marks the outdoor camera whose view anchors the effectively infinite sun visual.
#[derive(Component, Default)]
struct WorldEnvironmentView;

/// Cheap visible proxy for the directional light; it does not participate in lighting or shadows.
#[derive(Component)]
struct WorldSunVisual;

/// Camera components required for the shared low-cost outdoor atmosphere.
#[derive(Bundle)]
pub struct WorldEnvironmentCamera {
    fog: DistanceFog,
    exposure: Exposure,
    view: WorldEnvironmentView,
}

impl Default for WorldEnvironmentCamera {
    fn default() -> Self {
        Self {
            fog: DistanceFog {
                color: SKY_COLOR,
                // This is the broad atmospheric cue around the projected sun. Grass reuses its
                // existing receiver visibility for the same lobe; Bevy handles ordinary PBR.
                directional_light_color: Color::srgba(1.0, 0.90, 0.72, 0.14),
                directional_light_exponent: 32.0,
                falloff: FogFalloff::from_visibility_colors(
                    FOG_VISIBILITY_DISTANCE,
                    Color::srgb(0.40, 0.52, 0.64),
                    Color::srgb(0.47, 0.58, 0.70),
                ),
            },
            // Direct sunlight and all custom foliage radiance are exposure-aware.
            exposure: Exposure { ev100: 13.0 },
            view: WorldEnvironmentView,
        }
    }
}

fn setup_world_environment(
    mut commands: Commands,
    config: Res<WorldEnvironmentConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(GlobalAmbientLight {
        // Outdoor sky fill without a dynamically generated environment cubemap.
        color: Color::srgb(0.58, 0.68, 0.82),
        brightness: 5_000.0,
        ..default()
    });

    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.93, 0.82),
            illuminance: lux::DIRECT_SUNLIGHT,
            shadow_maps_enabled: true,
            ..default()
        },
        CascadeShadowConfigBuilder {
            num_cascades: 3,
            first_cascade_far_bound: config.first_cascade_far_bound,
            maximum_distance: config.maximum_shadow_distance,
            ..default()
        }
        .build(),
        Transform::from_translation(SUN_POSITION).looking_at(Vec3::ZERO, Vec3::Y),
        WorldSun,
        Name::new("Sun"),
    ));

    // The light itself is directional and therefore has no world-space source to look at. This
    // camera-relative unlit sphere gives its projected direction a stable, inexpensive visual
    // anchor without another light, shadow map, atmosphere pass, or fullscreen effect.
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(SUN_VISUAL_RADIUS))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.82, 0.54),
            emissive: LinearRgba::rgb(30.0, 16.0, 5.0),
            unlit: true,
            ..default()
        })),
        Transform::default(),
        NotShadowCaster,
        NotShadowReceiver,
        WorldSunVisual,
        Name::new("Sun visual"),
    ));
}

fn update_sun_visual(
    camera: Single<
        &Transform,
        (
            With<WorldEnvironmentView>,
            Without<WorldSun>,
            Without<WorldSunVisual>,
        ),
    >,
    sun: Single<
        &Transform,
        (
            With<WorldSun>,
            Without<WorldEnvironmentView>,
            Without<WorldSunVisual>,
        ),
    >,
    mut visual: Single<
        &mut Transform,
        (
            With<WorldSunVisual>,
            Without<WorldEnvironmentView>,
            Without<WorldSun>,
        ),
    >,
) {
    let direction_to_sun: Vec3 = sun.back().into();
    visual.translation = camera.translation + direction_to_sun * SUN_VISUAL_DISTANCE;
}
