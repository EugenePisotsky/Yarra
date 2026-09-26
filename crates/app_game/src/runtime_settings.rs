//! Shared render/feature configuration and its application to the running game.
//! CLI setup, reproductions and F1 edit the same snapshot. This module owns no panel,
//! capture state, timing probes or argument parsing; it also works without diagnostics.
use crate::game_render::{
    GameRenderSettings, GameRenderSetup, GameRenderSystems, RESOLUTION_SCALES, RenderPath,
};
use bevy::{core_pipeline::prepass::DepthPrepass, light::ShadowFilteringMethod, prelude::*};
use engine::{
    GAME_DEPTH_PREPASS_ENABLED, GameInputEnabled, GameplaySystems, WorldSun, WorldViewCamera,
};
use terrain_render::{TerrainMaterial, TerrainShadingMode, composite::TerrainCompositeMaterial};
use vegetation_render::{
    VegetationDebugSettings, VegetationLightingMode, VegetationProfileMode, VegetationWind,
};

pub(crate) fn load_canopy(path: &std::path::Path) -> Result<vegetation::CanopyShading, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) struct RuntimeSettingsPlugin;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeSettingsInit;
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeSettingsApply;

impl Plugin for RuntimeSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<crate::launch::LaunchOptions>();
        let options = app.world().resource::<crate::launch::LaunchOptions>();
        let settings = RuntimeSettings {
            terrain_near_disabled: options.terrain_near_off,
            terrain_prepared: !options.terrain_reference,
            upscaler: options.upscaler,
            counters: options.counters,
            gpu_pass_timings: options.gpu_detail,
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
                )
                    .chain()
                    .in_set(RuntimeSettingsApply)
                    .before(GameRenderSystems)
                    .before(GameplaySystems::CameraInput),
            )
            .add_systems(
                PostUpdate,
                apply_meshes
                    .before(terrain_render::TerrainMaterialPreparation)
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            );
    }
}

fn initialize(
    mut settings: ResMut<RuntimeSettings>,
    clouds: Res<engine::CloudQuality>,
    grass: Res<VegetationDebugSettings>,
    lighting: Res<vegetation_render::VegetationLighting>,
    variation: Res<terrain_render::TerrainMacroVariation>,
) {
    settings.canopy = lighting.canopy;
    settings.terrain_macro = *variation;
    settings.clouds = *clouds;
    settings.density = grass.density_mode;
    settings.lighting = grass.lighting_mode;
}

pub(crate) fn apply_input_lock(
    settings: Res<RuntimeSettings>,
    mut enabled: ResMut<GameInputEnabled>,
) {
    enabled.0 = !settings.controls_locked;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scene {
    #[default]
    Current,
    Clear,
    Ground,
    Grass,
}

/// Directional shadow map edge per cascade. Rendering the maps is most of the shadow cost; at
/// 2560×1440 with 4× MSAA, 1024 saved ~0.7 ms against 2048 on M2 Max.
pub(crate) const SHADOW_MAP_SIZES: [usize; 3] = [2048, 1536, 1024];

#[derive(Resource, Clone, Debug)]
pub(crate) struct RuntimeSettings {
    pub(crate) scene: Scene,
    pub(crate) grass: VegetationProfileMode,
    pub(crate) unlit: bool,
    pub(crate) ground_shading: TerrainShadingMode,
    pub(crate) terrain_near_disabled: bool,
    pub(crate) terrain_prepared: bool,
    pub(crate) shadows: u8,
    /// Index into `SHADOW_MAP_SIZES`.
    pub(crate) shadow_map: usize,
    pub(crate) prepass: bool,
    pub(crate) scale_index: usize,
    pub(crate) upscaler: upscaling::UpscaleMethod,
    pub(crate) temporal_debug: upscaling::temporal::TemporalDebug,
    pub(crate) msaa: Msaa,
    pub(crate) counters: bool,
    pub(crate) gpu_pass_timings: bool,
    pub(crate) wind: bool,
    pub(crate) controls_locked: bool,
    pub(crate) render_path: RenderPath,
    pub(crate) show_ui: bool,
    pub(crate) changed_at: f64,
    pub(crate) clouds: engine::CloudQuality,
    pub(crate) sky: bool,
    pub(crate) bloom: bool,
    pub(crate) hide_terrain: bool,
    pub(crate) hide_objects: bool,
    pub(crate) density: vegetation_render::VegetationDensityMode,
    pub(crate) terrain_detail: usize,
    pub(crate) page_gizmos: bool,
    pub(crate) terrain_macro: terrain_render::TerrainMacroVariation,
    pub(crate) canopy: vegetation::CanopyShading,
    pub(crate) object_detail: usize,
    pub(crate) lighting: VegetationLightingMode,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        let render = GameRenderSettings::default();
        Self {
            scene: Scene::Current,
            grass: VegetationProfileMode::Full,
            unlit: false,
            ground_shading: TerrainShadingMode::Production,
            terrain_near_disabled: false,
            terrain_prepared: true,
            shadows: 0,
            shadow_map: 0,
            prepass: GAME_DEPTH_PREPASS_ENABLED,
            scale_index: RESOLUTION_SCALES
                .iter()
                .position(|&scale| scale == render.resolution_scale)
                .expect("game scale is available in runtime settings"),
            msaa: render.msaa,
            upscaler: render.upscaler,
            temporal_debug: render.temporal_debug,
            // Statistics atomics are explicit on every platform, including audit startup.
            counters: false,
            gpu_pass_timings: false,
            wind: true,
            controls_locked: false,
            render_path: render.render_path,
            show_ui: render.show_ui,
            changed_at: 0.0,
            clouds: engine::CloudQuality::default(),
            sky: true,
            bloom: true,
            hide_terrain: false,
            hide_objects: false,
            density: vegetation_render::VegetationDensityMode::Balanced,
            terrain_detail: 1,
            page_gizmos: false,
            terrain_macro: default(),
            canopy: default(),
            object_detail: 1,
            lighting: VegetationLightingMode::RoundedGloss,
        }
    }
}

impl RuntimeSettings {
    pub(crate) fn temporal_active(&self) -> bool {
        self.upscaler == upscaling::UpscaleMethod::MetalFxTemporal
            && self.render_path == RenderPath::Composite
    }
    pub(crate) fn scale(&self) -> f32 {
        match self.render_path {
            RenderPath::Direct => 1.0,
            RenderPath::Composite => RESOLUTION_SCALES[self.scale_index],
        }
    }

    pub(crate) fn terrain_shading(&self) -> TerrainShadingMode {
        if self.scene == Scene::Ground {
            self.ground_shading
        } else if self.unlit {
            TerrainShadingMode::SurfaceUnlit
        } else {
            TerrainShadingMode::Production
        }
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
    terrain: Option<Handle<TerrainMaterial>>,
    composite: Option<Handle<TerrainCompositeMaterial>>,
}

fn sync_render_settings(
    s: Res<RuntimeSettings>,
    profile: Option<Res<crate::profile::ProfileSettings>>,
    mut render: ResMut<GameRenderSettings>,
) {
    if s.is_changed() {
        render.set_if_neq(GameRenderSettings {
            direct_temporal_output: render.direct_temporal_output,
            resolution_scale: profile.as_ref().map_or(s.scale(), |p| p.resolution_scale()),
            upscaler: s.upscaler,
            temporal_debug: s.temporal_debug,
            render_size: profile.as_ref().and_then(|p| p.size),
            msaa: s.msaa,
            render_path: s.render_path,
            show_ui: s.show_ui,
        });
    }
}

pub(crate) fn page_gizmos_enabled(settings: Res<RuntimeSettings>) -> bool {
    settings.page_gizmos
}

pub(crate) fn apply_appearance(
    settings: Res<RuntimeSettings>,
    mut lighting: ResMut<vegetation_render::VegetationLighting>,
    mut variation: ResMut<terrain_render::TerrainMacroVariation>,
) {
    if settings.is_changed() {
        if lighting.canopy != settings.canopy {
            lighting.canopy = settings.canopy;
        }
        variation.set_if_neq(settings.terrain_macro);
    }
}

fn apply_settings(
    mut commands: Commands,
    s: Res<RuntimeSettings>,
    camera: Single<Entity, With<WorldViewCamera>>,
    mut sun: Single<&mut DirectionalLight, With<WorldSun>>,
    mut grass: ResMut<VegetationDebugSettings>,
    mut wind: ResMut<VegetationWind>,
    mut prepared: ResMut<terrain_render::TerrainPreparedSettings>,
    mut clouds: ResMut<engine::CloudQuality>,
    mut atmosphere: ResMut<engine::AtmospherePresentation>,
    mut lod: ResMut<engine::TerrainLodPreview>,
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
    object_lod.0 = [0.5, 1.0, 2.0][s.object_detail];
    *clouds = s.clouds;
    atmosphere.sky_and_haze = s.sky;
    atmosphere.bloom = s.bloom;
    lod.settings.refine_pixels = [1.0, 2.0, 4.0, 8.0][s.terrain_detail];
    lod.settings.collapse_pixels = lod.settings.refine_pixels * 0.5;
    grass.density_mode = s.density;
    grass.profile_mode = s.grass_mode();
    grass.lighting_mode = if s.unlit {
        VegetationLightingMode::UnlitDiagnostic
    } else {
        s.lighting
    };
    grass.gpu_counters_enabled = s.counters;
    wind.enabled = s.wind;
    prepared.enabled = s.terrain_prepared;
    sun.shadow_maps_enabled = s.shadows != 2;
    commands.entity(*camera).insert(if s.shadows == 1 {
        ShadowFilteringMethod::Hardware2x2
    } else {
        ShadowFilteringMethod::Gaussian
    });
    if s.prepass || s.temporal_active() {
        commands.entity(*camera).insert(DepthPrepass);
    } else {
        commands.entity(*camera).remove::<DepthPrepass>();
    }
}

#[allow(clippy::type_complexity)] // Keep the optional original-material/state query explicit.
fn apply_meshes(
    mut commands: Commands,
    s: Res<RuntimeSettings>,
    new_meshes: Query<(), Added<Mesh3d>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut composites: ResMut<Assets<TerrainCompositeMaterial>>,
    mut meshes: Query<
        (
            Entity,
            &mut Visibility,
            Option<&MeshMaterial3d<TerrainMaterial>>,
            Option<&MeshMaterial3d<TerrainCompositeMaterial>>,
            Option<&mut MeshOverrideState>,
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
    for (entity, mut visibility, material, composite, saved) in &mut meshes {
        if !s.is_changed() && saved.as_ref().is_some_and(|state| !state.override_active) {
            continue;
        }
        let initial = MeshOverrideState {
            original_visibility: *visibility,
            override_active: false,
            terrain: material.map(|m| m.0.clone()),
            composite: composite.map(|m| m.0.clone()),
        };
        let is_new = saved.is_none();
        let mut saved = saved;
        let mut initial = initial;
        let state = saved.as_deref_mut().unwrap_or(&mut initial);
        let terrain = state.terrain.is_some() || state.composite.is_some();
        let forced = match s.scene {
            Scene::Current if (terrain && s.hide_terrain) || (!terrain && s.hide_objects) => {
                Some(Visibility::Hidden)
            }
            Scene::Current => None,
            Scene::Ground if terrain => Some(Visibility::Visible),
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
        if !is_new && !s.is_changed() {
            continue;
        }
        if let Some(handle) = &state.terrain {
            let desired = s.terrain_shading();
            if materials
                .get(handle)
                .is_some_and(|m| m.shading_mode != desired)
            {
                materials.get_mut(handle).unwrap().shading_mode = desired;
            }
        }
        if let Some(handle) = &state.composite {
            let desired = s.terrain_shading();
            if composites.get(handle).is_some_and(|m| {
                m.shading_mode != desired || m.near_disabled != s.terrain_near_disabled
            }) {
                let mut m = composites.get_mut(handle).unwrap();
                m.shading_mode = desired;
                m.near_disabled = s.terrain_near_disabled;
            }
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
    fn ground_audit_includes_hierarchy_materials_and_restores_visibility() {
        let mut app = App::new();
        app.init_resource::<Assets<TerrainMaterial>>()
            .init_resource::<Assets<TerrainCompositeMaterial>>()
            .insert_resource(RuntimeSettings {
                scene: Scene::Ground,
                ground_shading: TerrainShadingMode::Flat,
                terrain_near_disabled: true,
                ..default()
            })
            .add_systems(Update, apply_meshes);
        let material = app
            .world_mut()
            .resource_mut::<Assets<TerrainCompositeMaterial>>()
            .add(TerrainCompositeMaterial::default());
        let ground = app
            .world_mut()
            .spawn((
                Mesh3d::default(),
                MeshMaterial3d(material.clone()),
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
        let m = app
            .world()
            .resource::<Assets<TerrainCompositeMaterial>>()
            .get(&material)
            .unwrap();
        assert_eq!(m.shading_mode, TerrainShadingMode::Flat);
        assert!(m.near_disabled);
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
        let m = app
            .world()
            .resource::<Assets<TerrainCompositeMaterial>>()
            .get(&material)
            .unwrap();
        assert_eq!(m.shading_mode, TerrainShadingMode::Production);
        assert!(!m.near_disabled);
    }

    #[test]
    fn draw_switches_cover_new_meshes_and_restore_current_visibility() {
        let mut app = App::new();
        app.init_resource::<RuntimeSettings>()
            .init_resource::<Assets<TerrainMaterial>>()
            .init_resource::<Assets<TerrainCompositeMaterial>>()
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
