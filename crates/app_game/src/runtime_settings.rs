//! Shared render/feature configuration and its application to the running game.
//! CLI setup, reproductions and F1 edit the same snapshot. This module owns no panel,
//! capture state, timing probes or argument parsing; it also works without diagnostics.
use crate::game_render::{
    GameRenderSettings, GameRenderSetup, GameRenderSystems, RESOLUTION_SCALES, scale_index,
};

use crate::launch::LaunchOptions;
use bevy::{light::ShadowFilteringMethod, prelude::*};
use engine::{GameInputEnabled, GameplaySystems, WorldViewCamera};
use vegetation_render::{VegetationProfileMode, VegetationSettings, VegetationWind};

/// Launch options applied once to the resources they configure, before [`RuntimeSettingsPlugin`]
/// reads them. Settings F1 also changes (counters) are written by
/// [`apply_settings`] alone.
pub(crate) fn apply_launch_options(app: &mut App) -> Result<(), String> {
    let clouds = app.world().resource::<LaunchOptions>().clouds;
    app.insert_resource(clouds);
    Ok(())
}

pub(crate) struct RuntimeSettingsPlugin;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeSettingsInit;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeSettingsApply;

impl Plugin for RuntimeSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LaunchOptions>()
            .init_resource::<engine::PlayerMovementSpeed>();
        let options = app.world().resource::<LaunchOptions>();
        let settings = RuntimeSettings {
            upscaler: options.upscaler,
            counters: options.counters,
            gpu_pass_timings: options.gpu_detail,
            scale_index: options.resolution_scale.map_or_else(
                || RuntimeSettings::default().scale_index,
                |scale| scale_index(scale).expect("launch validates the resolution scale"),
            ),
            ..default()
        };
        app.insert_resource(settings)
            .add_systems(
                PostStartup,
                (initialize, sync_render_settings)
                    .chain()
                    .in_set(RuntimeSettingsInit)
                    .before(GameRenderSetup),
            )
            .add_systems(
                Update,
                (
                    sync_render_settings,
                    apply_settings,
                    apply_appearance,
                    apply_input_lock,
                    apply_player_movement,
                )
                    .chain()
                    .in_set(RuntimeSettingsApply)
                    .before(GameRenderSystems)
                    .before(GameplaySystems::CameraInput),
            )
            .add_systems(
                PostUpdate,
                (apply_meshes, apply_trees)
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            );
    }
}

fn initialize(
    mut settings: ResMut<RuntimeSettings>,
    clouds: Res<engine::CloudQuality>,
    variation: Res<terrain_render::TerrainMacroVariation>,
) {
    settings.terrain_macro = *variation;
    settings.clouds = *clouds;
}

pub(crate) fn apply_input_lock(
    settings: Res<RuntimeSettings>,
    mut enabled: ResMut<GameInputEnabled>,
) {
    enabled.0 = !settings.controls_locked;
}

pub(crate) fn apply_player_movement(
    settings: Res<RuntimeSettings>,
    mut speed: ResMut<engine::PlayerMovementSpeed>,
) {
    speed.set_if_neq(if settings.fast_movement {
        engine::PlayerMovementSpeed::Fast
    } else {
        engine::PlayerMovementSpeed::Normal
    });
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scene {
    #[default]
    Current,
    Clear,
    Ground,
    Grass,
}

/// F1's object LOD size steps, as `VisualLodScale` factors.
pub(crate) const OBJECT_DETAIL_SCALES: [f32; 3] = [0.5, 1.0, 2.0];

/// Directional shadow map edge per cascade. Rendering the maps is most of the shadow cost; at
/// 2560×1440 with 4× MSAA, 1024 saved ~0.7 ms against 2048 on M2 Max.
pub(crate) const SHADOW_MAP_SIZES: [usize; 3] = [2048, 1536, 1024];

#[derive(Resource, Clone, Debug)]
pub(crate) struct RuntimeSettings {
    pub(crate) scene: Scene,
    pub(crate) grass: VegetationProfileMode,
    pub(crate) shadows: u8,
    /// Index into `SHADOW_MAP_SIZES`.
    pub(crate) shadow_map: usize,
    pub(crate) scale_index: usize,
    pub(crate) upscaler: upscaling::UpscaleMethod,
    pub(crate) temporal_debug: upscaling::temporal::TemporalDebug,
    pub(crate) counters: bool,
    pub(crate) gpu_pass_timings: bool,
    pub(crate) wind: bool,
    /// Characters pushing branches aside (`engine::TreeTouch`).
    pub(crate) tree_touch: bool,
    pub(crate) controls_locked: bool,
    pub(crate) fast_movement: bool,
    pub(crate) show_ui: bool,
    pub(crate) changed_at: f64,
    pub(crate) clouds: engine::CloudQuality,
    pub(crate) sky: bool,
    pub(crate) bloom: bool,
    pub(crate) auto_exposure: bool,
    /// Ground haze and valley mist.
    pub(crate) fog: bool,
    /// Dust motes, seed fluff and falling leaves.
    pub(crate) particles: bool,
    /// Sunbeams through haze, mist and the air under crowns.
    pub(crate) light_shafts: bool,
    pub(crate) hide_terrain: bool,
    pub(crate) hide_objects: bool,
    pub(crate) page_gizmos: bool,
    pub(crate) terrain_macro: terrain_render::TerrainMacroVariation,
    pub(crate) object_detail: usize,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        let render = GameRenderSettings::default();
        Self {
            scene: Scene::Current,
            grass: VegetationProfileMode::Full,
            shadows: 0,
            shadow_map: 0,
            scale_index: scale_index(render.resolution_scale)
                .expect("game scale is available in runtime settings"),
            upscaler: render.upscaler,
            temporal_debug: render.temporal_debug,
            // Statistics atomics are explicit on every platform, including audit startup.
            counters: false,
            gpu_pass_timings: false,
            wind: true,
            tree_touch: true,
            controls_locked: false,
            fast_movement: false,
            show_ui: render.show_ui,
            changed_at: 0.0,
            clouds: engine::CloudQuality::default(),
            sky: true,
            bloom: true,
            auto_exposure: true,
            fog: true,
            particles: true,
            light_shafts: true,
            hide_terrain: false,
            hide_objects: false,
            page_gizmos: false,
            terrain_macro: default(),
            object_detail: 1,
        }
    }
}

impl RuntimeSettings {
    pub(crate) fn scale(&self) -> f32 {
        RESOLUTION_SCALES[self.scale_index]
    }

    pub(crate) fn grass_mode(&self) -> VegetationProfileMode {
        if matches!(self.scene, Scene::Clear | Scene::Ground) {
            VegetationProfileMode::Disabled
        } else {
            self.grass
        }
    }
}

#[derive(Component)]
struct MeshOverrideState {
    original_visibility: Visibility,
    override_active: bool,
    /// One of terrain v2's patches.
    terrain: bool,
}

fn sync_render_settings(
    s: Res<RuntimeSettings>,
    profile: Option<Res<crate::dev::profile::ProfileSettings>>,
    mut render: ResMut<GameRenderSettings>,
) {
    if s.is_changed() {
        render.set_if_neq(GameRenderSettings {
            direct_temporal_output: render.direct_temporal_output,
            resolution_scale: profile.as_ref().map_or(s.scale(), |p| p.resolution_scale()),
            upscaler: s.upscaler,
            temporal_debug: s.temporal_debug,
            render_size: profile.as_ref().and_then(|p| p.size),
            show_ui: s.show_ui,
        });
    }
}

pub(crate) fn apply_appearance(
    settings: Res<RuntimeSettings>,
    mut variation: ResMut<terrain_render::TerrainMacroVariation>,
) {
    if settings.is_changed() {
        variation.set_if_neq(settings.terrain_macro);
    }
}

#[allow(clippy::too_many_arguments)] // One writer for every resource the settings drive.
fn apply_settings(
    mut commands: Commands,
    s: Res<RuntimeSettings>,
    camera: Single<Entity, With<WorldViewCamera>>,
    mut grass: ResMut<VegetationSettings>,
    mut wind: ResMut<VegetationWind>,
    mut clouds: ResMut<engine::CloudQuality>,
    mut atmosphere: ResMut<engine::AtmospherePresentation>,
    mut object_lod: ResMut<engine::VisualLodScale>,
    shadow_map: Option<ResMut<bevy::light::DirectionalLightShadowMap>>,
) {
    if !s.is_changed() {
        return;
    }

    let size = SHADOW_MAP_SIZES[s.shadow_map];

    if let Some(mut shadow_map) = shadow_map
        && shadow_map.size != size
    {
        shadow_map.size = size;
    }

    object_lod.0 = OBJECT_DETAIL_SCALES[s.object_detail];
    *clouds = s.clouds;
    atmosphere.sky_and_haze = s.sky;
    atmosphere.bloom = s.bloom;
    atmosphere.auto_exposure = s.auto_exposure;
    atmosphere.low_air = s.fog;
    atmosphere.particles = s.particles;
    atmosphere.light_shafts = s.light_shafts;
    grass.profile_mode = s.grass_mode();
    grass.gpu_counters_enabled = s.counters;
    wind.enabled = s.wind;
    atmosphere.shadows = s.shadows != 2;
    commands.entity(*camera).insert(if s.shadows == 1 {
        ShadowFilteringMethod::Hardware2x2
    } else {
        ShadowFilteringMethod::Gaussian
    });
}

/// Trees have no mesh entities to hide (`engine::TreeInstancing`): they hide with the other
/// objects, and in every scene but the current one.
fn apply_trees(
    s: Res<RuntimeSettings>,
    trees: Option<ResMut<engine::TreeInstancing>>,
    touch: Option<ResMut<engine::TreeTouch>>,
) {
    let hidden = s.hide_objects || s.scene != Scene::Current;

    if let Some(mut trees) = trees
        && trees.hidden != hidden
    {
        trees.hidden = hidden;
    }

    if let Some(mut touch) = touch {
        touch.set_if_neq(engine::TreeTouch {
            enabled: s.tree_touch,
        });
    }
}

#[allow(clippy::type_complexity)] // Keep the optional state query explicit.
fn apply_meshes(
    mut commands: Commands,
    s: Res<RuntimeSettings>,
    new_meshes: Query<(), Added<Mesh3d>>,
    mut meshes: Query<
        (
            Entity,
            &mut Visibility,
            Option<&mut MeshOverrideState>,
            Has<MeshMaterial3d<terrain_render::clipmap::ClipmapTerrainMaterial>>,
        ),
        With<Mesh3d>,
    >,
) {
    if !s.is_changed()
        && new_meshes.is_empty()
        && s.scene == Scene::Current
        && !s.hide_terrain
        && !s.hide_objects
    {
        return;
    }
    for (entity, mut visibility, saved, terrain) in &mut meshes {
        if !s.is_changed() && saved.as_ref().is_some_and(|state| !state.override_active) {
            continue;
        }

        let mut initial = MeshOverrideState {
            original_visibility: *visibility,
            override_active: false,
            terrain,
        };
        let is_new = saved.is_none();
        let mut saved = saved;
        let state = saved.as_deref_mut().unwrap_or(&mut initial);
        let forced = match s.scene {
            Scene::Current
                if (state.terrain && s.hide_terrain) || (!state.terrain && s.hide_objects) =>
            {
                Some(Visibility::Hidden)
            }
            Scene::Current => None,
            Scene::Ground if state.terrain => Some(Visibility::Visible),
            _ => Some(Visibility::Hidden),
        };

        if let Some(desired) = forced {
            if !state.override_active {
                state.original_visibility = *visibility;
            }

            state.override_active = true;

            if *visibility != desired {
                *visibility = desired;
            }
        } else if state.override_active {
            *visibility = state.original_visibility;
            state.override_active = false;
        }

        if is_new {
            commands.entity(entity).insert(initial);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_audit_includes_terrain_patches_and_restores_visibility() {
        let mut app = App::new();
        app.insert_resource(RuntimeSettings {
            scene: Scene::Ground,
            ..default()
        })
        .add_systems(Update, apply_meshes);
        let ground = app
            .world_mut()
            .spawn((
                Mesh3d::default(),
                MeshMaterial3d::<terrain_render::clipmap::ClipmapTerrainMaterial>::default(),
                Visibility::Inherited,
            ))
            .id();
        let object = app
            .world_mut()
            .spawn((Mesh3d::default(), Visibility::Inherited))
            .id();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(ground).unwrap(),
            Visibility::Visible
        );
        assert_eq!(
            *app.world().get::<Visibility>(object).unwrap(),
            Visibility::Hidden
        );
        app.world_mut().resource_mut::<RuntimeSettings>().scene = Scene::Clear;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(ground).unwrap(),
            Visibility::Hidden
        );
        *app.world_mut().resource_mut::<RuntimeSettings>() = RuntimeSettings::default();
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(ground).unwrap(),
            Visibility::Inherited
        );
        assert_eq!(
            *app.world().get::<Visibility>(object).unwrap(),
            Visibility::Inherited
        );
    }

    #[test]
    fn draw_switches_cover_new_meshes_and_restore_current_visibility() {
        let mut app = App::new();
        app.init_resource::<RuntimeSettings>()
            .add_systems(Update, apply_meshes);
        let object = app
            .world_mut()
            .spawn((Mesh3d::default(), Visibility::Inherited))
            .id();
        app.update();

        // Normal application visibility changes must survive unrelated settings edits.
        *app.world_mut().get_mut::<Visibility>(object).unwrap() = Visibility::Hidden;
        app.world_mut().resource_mut::<RuntimeSettings>().bloom = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(object),
            Some(&Visibility::Hidden)
        );
        app.world_mut()
            .resource_mut::<RuntimeSettings>()
            .hide_objects = true;
        app.update();
        let streamed = app
            .world_mut()
            .spawn((Mesh3d::default(), Visibility::Inherited))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(streamed),
            Some(&Visibility::Hidden)
        );
        app.world_mut()
            .resource_mut::<RuntimeSettings>()
            .hide_objects = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(streamed),
            Some(&Visibility::Inherited)
        );
        assert_eq!(
            app.world().get::<Visibility>(object),
            Some(&Visibility::Hidden)
        );
    }
}
