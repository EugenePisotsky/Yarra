//! Enter/leave ownership of shared renderer, lighting and frame pacing.
use crate::workspaces::{
    EditorFramePacing, FramePacingOwner,
    vegetation::{
        state::{RestoredWorld, StudyState},
        viewport::LAYER,
    },
};
use bevy::{camera::visibility::RenderLayers, prelude::*};
use engine::WorldSun;
use std::time::Instant;
use vegetation_render::{
    VegetationDebugSettings, VegetationLighting, VegetationProfileMode, VegetationSceneState,
    VegetationShapeInspection, VegetationWind,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn enter(
    mut commands: Commands,
    mut state: ResMut<StudyState>,
    scene: Res<VegetationSceneState>,
    mut settings: ResMut<VegetationDebugSettings>,
    mut lighting: ResMut<VegetationLighting>,
    mut wind: ResMut<VegetationWind>,
    mut pacing: ResMut<EditorFramePacing>,
    mut sun: Single<
        (
            Entity,
            &mut Transform,
            &mut DirectionalLight,
            Option<&RenderLayers>,
        ),
        With<WorldSun>,
    >,
    mut ambient: ResMut<GlobalAmbientLight>,
) {
    let shared_canopy = lighting.canopy;
    let (entity, transform, light, layers) = &mut *sun;
    state.restore = Some(RestoredWorld {
        scene: scene.clone(),
        settings: *settings,
        lighting: *lighting,
        wind: *wind,
        sun: (*entity, **transform, (**light).clone(), layers.cloned()),
        ambient: (*ambient).clone(),
    });
    commands.entity(*entity).insert(RenderLayers::layer(LAYER));
    if let Some(doc) = &state.launch.load {
        *settings = doc.settings;
        *lighting = doc.lighting;
        transform.rotation = Quat::from_array(doc.sun_rotation);
        light.color = Srgba::from_f32_array(doc.sun_color).into();
        light.illuminance = doc.sun_illuminance;
        ambient.color = Srgba::from_f32_array(doc.ambient_color).into();
        ambient.brightness = doc.ambient_brightness;
    } else {
        if let Some((s, l)) = state.own_settings {
            *settings = s;
            *lighting = l;
            lighting.canopy = shared_canopy;
        }
        if let Some((t, l, a)) = &state.own_environment {
            **transform = *t;
            **light = l.clone();
            *ambient = a.clone();
        } else {
            // A new study needs its own reproducible daylight, even when entered
            // from a night/interior world. Saved studies keep their authored lights.
            crate::workspaces::apply_study_daylight(transform, light, &mut ambient);
        }
    }
    lighting.canopy_origin = [0.0; 2];
    if let Some(shape) = state.launch.shape {
        settings.shape_inspection = shape;
    }
    if let Some(bands) = state.launch.blade_bands {
        settings.blade_bands = bands;
    }
    if settings.shape_inspection != VegetationShapeInspection::Off {
        state.field_size = state.field_size.min(16.0);
        settings.mode = vegetation_render::VegetationDebugMode::ProceduralGeometry;
    }
    if state.launch.no_opening {
        settings.inspection_disable_opening = true;
    }
    if state.launch.no_wind {
        state.wind.enabled = false;
    }
    settings.profile_mode = VegetationProfileMode::Full;
    settings.gpu_counters_enabled = true;
    state.wind.apply(&mut wind);
    state.signature = None;
    state.ready_frames = 0;
    state.started = Instant::now();
    pacing.request(FramePacingOwner::VegetationWorkspace);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn leave(
    mut commands: Commands,
    mut state: ResMut<StudyState>,
    mut scene: ResMut<VegetationSceneState>,
    mut settings: ResMut<VegetationDebugSettings>,
    mut lighting: ResMut<VegetationLighting>,
    mut wind: ResMut<VegetationWind>,
    mut pacing: ResMut<EditorFramePacing>,
    mut ambient: ResMut<GlobalAmbientLight>,
    sun: Single<(&Transform, &DirectionalLight), With<WorldSun>>,
) {
    let shared_canopy = lighting.canopy;
    state.own_settings = Some((*settings, *lighting));
    state.own_environment = Some((*sun.0, sun.1.clone(), (*ambient).clone()));
    if let Some(saved) = state.restore.take() {
        *scene = saved.scene;
        *settings = saved.settings;
        *lighting = saved.lighting;
        lighting.canopy = shared_canopy;
        *wind = saved.wind;
        *ambient = saved.ambient;
        let (entity, transform, light, layers) = saved.sun;
        let mut entity = commands.entity(entity);
        entity.insert((transform, light));
        if let Some(layers) = layers {
            entity.insert(layers);
        } else {
            entity.remove::<RenderLayers>();
        }
    }
    pacing.release(FramePacingOwner::VegetationWorkspace);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::{
        AnimationWorkspaceCamera, EditorFramePacing, EditorWorkspace, EditorWorkspacesPlugin,
        vegetation::{
            state::StudyState,
            study::{StudyLaunch, bounded_scene},
            viewport::{LAYER, VegetationWorkspaceCamera},
        },
    };
    use bevy::camera::visibility::RenderLayers;
    use engine::WorldSun;
    use vegetation_render::{
        VegetationDebugSettings, VegetationLighting, VegetationSceneState, VegetationWind,
    };

    #[test]
    fn repeated_workspace_switches_restore_world_resources_and_camera_ownership() {
        let mut app = App::new();
        let original_scene = VegetationSceneState::reference();
        let mut original_wind = VegetationWind::default();
        original_wind.set_phase_seconds(123.0);
        let sun_transform = Transform::from_xyz(12.0, -20.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y);
        app.add_plugins(bevy::state::app::StatesPlugin)
            .add_plugins(EditorWorkspacesPlugin)
            .insert_resource(StudyState::new(StudyLaunch::default()))
            .insert_resource(original_scene.clone())
            .insert_resource(original_wind)
            .init_resource::<VegetationDebugSettings>()
            .init_resource::<VegetationLighting>()
            .init_resource::<GlobalAmbientLight>()
            .add_systems(OnEnter(EditorWorkspace::Vegetation), enter)
            .add_systems(OnExit(EditorWorkspace::Vegetation), leave)
            .add_systems(Update, crate::shell::sync_workspace_cameras);
        let sun = app
            .world_mut()
            .spawn((
                WorldSun,
                sun_transform,
                DirectionalLight {
                    illuminance: 0.0,
                    ..default()
                },
            ))
            .id();
        let world_camera = app
            .world_mut()
            .spawn((Camera::default(), engine::WorldViewCamera))
            .id();
        let animation_camera = app
            .world_mut()
            .spawn((Camera::default(), AnimationWorkspaceCamera))
            .id();
        let study_camera = app
            .world_mut()
            .spawn((Camera::default(), VegetationWorkspaceCamera))
            .id();
        app.update();
        for iteration in 0..3 {
            let world_canopy = vegetation::CanopyShading {
                strength: 0.5 + iteration as f32 * 0.1,
                ..vegetation::CanopyShading::experiment()
            };
            app.world_mut().resource_mut::<VegetationLighting>().canopy = world_canopy;
            app.world_mut()
                .resource_mut::<NextState<EditorWorkspace>>()
                .set(EditorWorkspace::Vegetation);
            app.update();
            assert_eq!(
                app.world()
                    .get::<DirectionalLight>(sun)
                    .unwrap()
                    .illuminance,
                if iteration == 0 { 128_000.0 } else { 20_000.0 }
            );
            assert!(app.world().get::<Transform>(sun).unwrap().back().y > 0.0);
            app.world_mut()
                .get_mut::<DirectionalLight>(sun)
                .unwrap()
                .illuminance = 20_000.0;
            assert_eq!(
                app.world().resource::<VegetationLighting>().canopy,
                world_canopy
            );
            let study_canopy = vegetation::CanopyShading {
                height_metres: 0.23,
                ..world_canopy
            };
            app.world_mut().resource_mut::<VegetationLighting>().canopy = study_canopy;
            assert!(app.world().get::<Camera>(study_camera).unwrap().is_active);
            assert!(!app.world().get::<Camera>(world_camera).unwrap().is_active);
            assert!(
                !app.world()
                    .get::<Camera>(animation_camera)
                    .unwrap()
                    .is_active
            );
            assert!(app.world().resource::<VegetationWind>().externally_driven);
            assert!(
                app.world()
                    .resource::<EditorFramePacing>()
                    .full_rate_preview()
            );
            assert_eq!(
                app.world().get::<RenderLayers>(sun),
                Some(&RenderLayers::layer(LAYER))
            );
            let patch = bounded_scene(&vegetation::fixtures::reference_catalog(), 3, 42)
                .unwrap()
                .0;
            app.world_mut()
                .resource_mut::<VegetationSceneState>()
                .replace(patch)
                .unwrap();
            app.world_mut()
                .resource_mut::<NextState<EditorWorkspace>>()
                .set(EditorWorkspace::World);
            app.update();
            assert_eq!(
                app.world().resource::<VegetationLighting>().canopy,
                study_canopy
            );
            assert!(app.world().get::<Camera>(world_camera).unwrap().is_active);
            assert!(!app.world().get::<Camera>(study_camera).unwrap().is_active);
            assert_eq!(
                app.world().resource::<VegetationSceneState>().scene(),
                original_scene.scene()
            );
            assert_eq!(
                app.world().resource::<VegetationWind>().phase_seconds(),
                123.0
            );
            assert!(!app.world().resource::<VegetationWind>().externally_driven);
            assert!(
                !app.world()
                    .resource::<VegetationDebugSettings>()
                    .gpu_counters_enabled
            );
            assert!(
                !app.world()
                    .resource::<EditorFramePacing>()
                    .full_rate_preview()
            );
            assert!(app.world().get::<RenderLayers>(sun).is_none());
            assert_eq!(*app.world().get::<Transform>(sun).unwrap(), sun_transform);
            assert_eq!(
                app.world()
                    .get::<DirectionalLight>(sun)
                    .unwrap()
                    .illuminance,
                0.0
            );
        }
    }
}
