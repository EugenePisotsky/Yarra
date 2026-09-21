//! Shared settings backend for the in-game Performance panel and CLI reproductions.
mod baseline;
mod logging;
mod performance;
mod repro;
mod timing;

use bevy::{
    core_pipeline::prepass::DepthPrepass, light::ShadowFilteringMethod, prelude::*,
    window::PrimaryWindow,
};
use engine::{
    GAME_DEPTH_PREPASS_ENABLED, GameInputEnabled, GameInputSystems, GamePointerInputBlocked,
    WorldSun, WorldViewCamera,
};
use terrain_render::composite::TerrainCompositeMaterial;
use terrain_render::{TerrainMaterial, TerrainShadingMode};
use vegetation_render::{
    VegetationDebugSettings, VegetationLightingMode, VegetationProfileMode, VegetationWind,
};

use crate::frame_pacing::{FramePacing, FrameRate};
use crate::game_render::{
    GameRenderAssets as AuditAssets, GameRenderSettings, GameRenderSetup, GameRenderSystems,
    RESOLUTION_SCALES, RenderPath as AuditRenderPath,
};

pub struct RenderAuditPlugin;

impl Plugin for RenderAuditPlugin {
    fn build(&self, app: &mut App) {
        let audit_logging =
            std::env::args_os().any(|a| a == "--render-audit" || a == "--render-repro");
        app.init_resource::<AuditSettings>()
            .init_resource::<baseline::BaselineRun>()
            .add_systems(
                PostStartup,
                (
                    performance::initialize,
                    sync_render_settings.before(GameRenderSetup),
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    buttons,
                    baseline::advance,
                    update_labels,
                    route_input,
                    sync_render_settings,
                    apply_settings,
                )
                    .chain()
                    .before(GameRenderSystems)
                    .before(GameInputSystems),
            )
            .add_systems(
                PostUpdate,
                apply_meshes
                    .before(terrain_render::TerrainMaterialPreparation)
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .add_systems(
                PostUpdate,
                logging::log_status
                    .after(TransformSystems::Propagate)
                    .run_if(move || audit_logging),
            );
        performance::install(app);
        app.add_plugins(timing::TimingPlugin);
        // Timed native-pacing observations can keep the normal gameplay camera
        // and controls. A reproduction route is optional, not a profiling prerequisite.
        crate::profile::install(app);
        repro::install(app);
        if let Some(profile) = app
            .world()
            .get_resource::<crate::profile::ProfileSettings>()
            .cloned()
        {
            let mut settings = app.world_mut().resource_mut::<AuditSettings>();
            settings.render_path = AuditRenderPath::Composite;
            settings.scale_index = if profile.size.is_some() { 0 } else { 2 };
            settings.msaa = profile.msaa;
            settings.bloom = profile.bloom;
            if profile.temporal_bypass {
                settings.temporal_debug = upscaling::temporal::TemporalDebug::Bypass;
            }
            settings.grass = if profile.grass {
                vegetation_render::VegetationProfileMode::Full
            } else {
                vegetation_render::VegetationProfileMode::Disabled
            };
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Scene {
    #[default]
    Current,
    Clear,
    Ground,
    Grass,
}

#[derive(Resource, Clone, Debug)]
struct AuditSettings {
    scene: Scene,
    grass: VegetationProfileMode,
    unlit: bool,
    ground_shading: TerrainShadingMode,
    terrain_near_disabled: bool,
    terrain_prepared: bool,
    shadows: u8,
    prepass: bool,
    scale_index: usize,
    upscaler: upscaling::UpscaleMethod,
    temporal_debug: upscaling::temporal::TemporalDebug,
    msaa: Msaa,
    counters: bool,
    gpu_pass_timings: bool,
    wind: bool,
    controls_locked: bool,
    render_path: AuditRenderPath,
    show_ui: bool,
    baseline_phase: Option<&'static str>,
    changed_at: f64,
    clouds: engine::CloudQuality,
    sky: bool,
    bloom: bool,
    hide_terrain: bool,
    hide_objects: bool,
    density: vegetation_render::VegetationDensityMode,
    terrain_detail: usize,
    overlays: bool,
    object_detail: usize,
    lighting: VegetationLightingMode,
}

impl Default for AuditSettings {
    fn default() -> Self {
        let render = GameRenderSettings::default();
        Self {
            scene: Scene::Current,
            grass: VegetationProfileMode::Full,
            unlit: false,
            ground_shading: TerrainShadingMode::Production,
            terrain_near_disabled: std::env::args_os().any(|arg| arg == "--terrain-near-off"),
            terrain_prepared: crate::terrain_prepared_enabled(),
            shadows: 0,
            prepass: GAME_DEPTH_PREPASS_ENABLED,
            scale_index: RESOLUTION_SCALES
                .iter()
                .position(|&scale| scale == render.resolution_scale)
                .expect("game scale is available in audit controls"),
            msaa: render.msaa,
            upscaler: render.upscaler,
            temporal_debug: render.temporal_debug,
            // Statistics atomics are explicit on every platform, including audit startup.
            counters: std::env::args_os().any(|arg| arg == "--grass-counters"),
            gpu_pass_timings: std::env::args_os().any(|arg| arg == "--gpu-timing-detail"),
            wind: true,
            controls_locked: false,
            render_path: render.render_path,
            show_ui: render.show_ui,
            baseline_phase: None,
            changed_at: 0.0,
            clouds: engine::CloudQuality::default(),
            sky: true,
            bloom: true,
            hide_terrain: false,
            hide_objects: false,
            density: vegetation_render::VegetationDensityMode::Balanced,
            terrain_detail: 1,
            overlays: false,
            object_detail: 1,
            lighting: VegetationLightingMode::RoundedGloss,
        }
    }
}

impl AuditSettings {
    fn temporal_active(&self) -> bool {
        self.upscaler == upscaling::UpscaleMethod::MetalFxTemporal
            && self.render_path == AuditRenderPath::Composite
    }
    fn scale(&self) -> f32 {
        match self.render_path {
            AuditRenderPath::Direct => 1.0,
            AuditRenderPath::Composite => RESOLUTION_SCALES[self.scale_index],
        }
    }

    fn terrain_shading(&self) -> TerrainShadingMode {
        if self.scene == Scene::Ground {
            self.ground_shading
        } else if self.unlit {
            TerrainShadingMode::SurfaceUnlit
        } else {
            TerrainShadingMode::Production
        }
    }

    fn grass_mode(&self) -> VegetationProfileMode {
        if matches!(self.scene, Scene::Clear | Scene::Ground) {
            VegetationProfileMode::Disabled
        } else {
            self.grass
        }
    }
}

#[derive(Component, Clone, Copy)]
enum Control {
    FrameRate,
    Scene,
    Grass,
    GrassEnabled,
    Shading,
    GroundMaterial,
    Shadows,
    Prepass,
    Scale,
    Upscaler,
    TemporalDebug,
    Counters,
    GpuPassTimings,
    Wind,
    Lock,
    Reset,
    RenderPath,
    Antialiasing,
    Clouds,
    Sky,
    Bloom,
    Terrain,
    Objects,
    Density,
    TerrainDetail,
    Near,
    Overlays,
    ObjectDetail,
}

impl Control {
    fn label(self, s: &AuditSettings, frame_rate: FrameRate) -> String {
        match self {
            Self::FrameRate => frame_rate.label(),
            Self::ObjectDetail => format!("Object LOD size: {}x", [0.5, 1.0, 2.0][s.object_detail]),
            Self::Clouds => format!("Clouds: {:?}", s.clouds),
            Self::Sky => format!("Sky + haze pass: {}", on_off(s.sky)),
            Self::Bloom => format!("Bloom pass: {}", on_off(s.bloom)),
            Self::Terrain => format!("Terrain draws: {}", on_off(!s.hide_terrain)),
            Self::Objects => format!("Object draws: {}", on_off(!s.hide_objects)),
            Self::Density => format!("Grass density: {}", s.density.label()),
            Self::TerrainDetail => format!("Terrain error: {} px", [1, 2, 4, 8][s.terrain_detail]),
            Self::Near => format!("Near terrain detail: {}", on_off(!s.terrain_near_disabled)),
            Self::Overlays => format!("Legacy overlays: {}", on_off(s.overlays)),
            Self::Scene => format!("Scene: {:?}", s.scene),
            Self::GrassEnabled => format!(
                "Grass vegetation: {}",
                on_off(s.grass_mode() != VegetationProfileMode::Disabled)
            ),
            Self::Grass => format!("Grass: {}", s.grass_mode().label()),
            Self::Shading if s.scene == Scene::Ground => {
                format!("Ground: {}", s.terrain_shading().label())
            }
            Self::Shading => format!("Shading: {}", if s.unlit { "unlit" } else { "production" }),
            Self::GroundMaterial => format!(
                "Ground material: {}",
                if s.terrain_prepared {
                    "prepared"
                } else {
                    "reference"
                }
            ),
            Self::Shadows => ["PBR shadows: Gaussian", "PBR shadows: 2x2", "Shadows: off"]
                [s.shadows as usize]
                .into(),
            Self::Prepass => {
                if s.temporal_active() {
                    "Depth prepass: required by temporal".into()
                } else {
                    format!("Depth prepass: {}", on_off(s.prepass))
                }
            }
            Self::Scale => format!(
                "Resolution: {}%",
                (100.0 * RESOLUTION_SCALES[s.scale_index]).round() as u32
            ),
            Self::TemporalDebug => format!("Temporal view: {}", s.temporal_debug.label()),
            Self::Upscaler => format!("Upscaler: {}", s.upscaler.label()),
            Self::Counters => format!("GPU counters: {}", on_off(s.counters)),
            Self::GpuPassTimings => format!("GPU pass timings: {}", on_off(s.gpu_pass_timings)),
            Self::Wind => format!("Grass wind: {}", on_off(s.wind)),
            Self::Lock => format!("Lock controls: {}", on_off(s.controls_locked)),
            Self::Reset => "Reset launch settings".into(),
            Self::RenderPath => format!("Render: {}", s.render_path.label()),
            Self::Antialiasing => {
                if s.temporal_active() {
                    "AA: temporal (MSAA off)".into()
                } else {
                    match s.msaa {
                        Msaa::Off => "AA: off".into(),
                        _ => format!("AA: {}x MSAA", s.msaa.samples()),
                    }
                }
            }
        }
    }
}

fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

#[derive(Component)]
struct AuditPanel;

#[derive(Component)]
struct AuditMesh {
    original_visibility: Visibility,
    override_active: bool,
    terrain: Option<Handle<TerrainMaterial>>,
    composite: Option<Handle<TerrainCompositeMaterial>>,
}

fn font(size: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(size),
        ..default()
    }
}

fn buttons(
    interactions: Query<(&Interaction, &Control), Changed<Interaction>>,
    mut s: ResMut<AuditSettings>,
    time: Res<Time>,
    panel: Res<performance::PanelState>,
    mut pacing: ResMut<FramePacing>,
) {
    if panel.recording() {
        return;
    }
    for (interaction, control) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if s.baseline_phase.is_some() {
            continue;
        }
        match control {
            Control::FrameRate => {
                if !pacing.profile_locked {
                    pacing.rate = pacing.rate.next();
                }
                // FPS alone must not reapply scene settings or disturb Temporal history.
                continue;
            }
            Control::ObjectDetail => s.object_detail = (s.object_detail + 1) % 3,
            Control::Clouds => {
                s.clouds = match s.clouds {
                    engine::CloudQuality::Off => engine::CloudQuality::Balanced,
                    engine::CloudQuality::Balanced => engine::CloudQuality::High,
                    engine::CloudQuality::High => engine::CloudQuality::Off,
                }
            }
            Control::Sky => s.sky = !s.sky,
            Control::Bloom => s.bloom = !s.bloom,
            Control::Terrain => s.hide_terrain = !s.hide_terrain,
            Control::Objects => s.hide_objects = !s.hide_objects,
            Control::Density => {
                s.density = match s.density {
                    vegetation_render::VegetationDensityMode::Balanced => {
                        vegetation_render::VegetationDensityMode::FullReference
                    }
                    vegetation_render::VegetationDensityMode::FullReference => {
                        vegetation_render::VegetationDensityMode::Authored
                    }
                    vegetation_render::VegetationDensityMode::Authored => {
                        vegetation_render::VegetationDensityMode::Balanced
                    }
                }
            }
            Control::TerrainDetail => s.terrain_detail = (s.terrain_detail + 1) % 4,
            Control::Near => s.terrain_near_disabled = !s.terrain_near_disabled,
            Control::Overlays => s.overlays = !s.overlays,
            Control::Scene => {
                s.scene = match s.scene {
                    Scene::Current => Scene::Clear,
                    Scene::Clear => Scene::Ground,
                    Scene::Ground => Scene::Grass,
                    Scene::Grass => Scene::Current,
                };
                s.grass = VegetationProfileMode::Full;
            }
            Control::GrassEnabled => {
                s.grass = if s.grass == VegetationProfileMode::Disabled {
                    VegetationProfileMode::Full
                } else {
                    VegetationProfileMode::Disabled
                }
            }
            Control::Grass => {
                s.grass = match s.grass {
                    VegetationProfileMode::Full => VegetationProfileMode::DrawFrozen,
                    VegetationProfileMode::DrawFrozen => VegetationProfileMode::ComputeOnly,
                    VegetationProfileMode::ComputeOnly => VegetationProfileMode::ScheduleOnly,
                    VegetationProfileMode::ScheduleOnly => VegetationProfileMode::Disabled,
                    VegetationProfileMode::Disabled => VegetationProfileMode::Full,
                };
                if s.grass == VegetationProfileMode::DrawFrozen {
                    s.controls_locked = true;
                }
            }
            Control::Shading if s.scene == Scene::Ground => {
                s.ground_shading = match s.ground_shading {
                    TerrainShadingMode::Production => TerrainShadingMode::SurfaceUnlit,
                    TerrainShadingMode::SurfaceUnlit => TerrainShadingMode::SingleTexture,
                    TerrainShadingMode::SingleTexture => TerrainShadingMode::Flat,
                    TerrainShadingMode::Flat => TerrainShadingMode::Production,
                };
            }
            Control::Shading => s.unlit = !s.unlit,
            Control::GroundMaterial => s.terrain_prepared = !s.terrain_prepared,
            Control::Shadows => s.shadows = (s.shadows + 1) % 3,
            Control::Prepass => s.prepass = !s.prepass,
            Control::Scale => {
                s.render_path = AuditRenderPath::Composite;
                s.scale_index = (s.scale_index + 1) % RESOLUTION_SCALES.len();
            }
            Control::TemporalDebug => {
                s.temporal_debug = s.temporal_debug.next();
            }
            Control::Upscaler => {
                let methods = upscaling::UpscaleMethod::ALL;
                let index = methods.iter().position(|m| *m == s.upscaler).unwrap_or(0);
                s.upscaler = methods[(index + 1) % methods.len()];
            }
            Control::Counters => s.counters = !s.counters,
            Control::GpuPassTimings => s.gpu_pass_timings = !s.gpu_pass_timings,
            Control::Wind => s.wind = !s.wind,
            Control::Lock => {
                s.controls_locked = !s.controls_locked;
                if !s.controls_locked && s.grass == VegetationProfileMode::DrawFrozen {
                    s.grass = VegetationProfileMode::Full;
                }
            }
            Control::Reset => {
                *s = panel.baseline.clone();
                pacing.rate = panel.baseline_rate;
            }
            Control::RenderPath => {
                s.render_path = match s.render_path {
                    AuditRenderPath::Composite => AuditRenderPath::Direct,
                    AuditRenderPath::Direct => AuditRenderPath::Composite,
                };
                s.scale_index = 0;
            }
            Control::Antialiasing => {
                s.msaa = match s.msaa {
                    Msaa::Off => Msaa::Sample2,
                    Msaa::Sample2 => Msaa::Sample4,
                    _ => Msaa::Off,
                };
            }
        }
        s.changed_at = time.elapsed_secs_f64();
    }
}

fn update_labels(
    s: Res<AuditSettings>,
    pacing: Res<FramePacing>,
    controls: Query<(&Control, &Children)>,
    mut labels: Query<&mut Text>,
) {
    if s.is_changed() || pacing.is_changed() {
        for (control, children) in &controls {
            for child in children {
                if let Ok(mut text) = labels.get_mut(*child) {
                    **text = if matches!(control, Control::FrameRate) {
                        pacing.control_label()
                    } else {
                        control.label(&s, pacing.rate)
                    };
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)] // Independent Bevy input resources and UI layout.
fn route_input(
    settings: Res<AuditSettings>,
    window: Single<&Window, With<PrimaryWindow>>,
    panels: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<AuditPanel>>,
    touches: Res<Touches>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut mouse_captured: Local<bool>,
    mut enabled: ResMut<GameInputEnabled>,
    mut pointer_blocked: ResMut<GamePointerInputBlocked>,
) {
    enabled.0 = !settings.controls_locked;
    let over_panel = |position: Vec2| {
        panels.iter().any(|(node, transform, visibility)| {
            visibility.get() && node.contains_point(*transform, position * window.scale_factor())
        })
    };
    let mouse_over_panel = window.cursor_position().is_some_and(over_panel);
    if mouse.any_just_pressed([MouseButton::Left, MouseButton::Right]) && mouse_over_panel {
        *mouse_captured = true;
    }
    // Include released touches and their initial positions: a HUD tap must never turn into a
    // world destination on release, even if the finger slid outside the panel in the meantime.
    let touch_over_panel = touches
        .iter()
        .chain(touches.iter_just_released())
        .chain(touches.iter_just_canceled())
        .any(|touch| over_panel(touch.start_position()) || over_panel(touch.position()));
    pointer_blocked.0 = mouse_over_panel || *mouse_captured || touch_over_panel;
    if !mouse.any_pressed([MouseButton::Left, MouseButton::Right]) {
        *mouse_captured = false;
    }
}

fn sync_render_settings(
    s: Res<AuditSettings>,
    profile: Option<Res<crate::profile::ProfileSettings>>,
    mut render: ResMut<GameRenderSettings>,
) {
    if s.is_changed() {
        render.set_if_neq(GameRenderSettings {
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

fn apply_settings(
    mut commands: Commands,
    s: Res<AuditSettings>,
    camera: Single<Entity, With<WorldViewCamera>>,
    mut sun: Single<&mut DirectionalLight, With<WorldSun>>,
    mut grass: ResMut<VegetationDebugSettings>,
    mut wind: ResMut<VegetationWind>,
    mut prepared: ResMut<terrain_render::TerrainPreparedSettings>,
    mut clouds: ResMut<engine::CloudQuality>,
    mut atmosphere: ResMut<engine::AtmospherePresentation>,
    mut lod: ResMut<engine::TerrainLodPreview>,
    mut object_lod: ResMut<engine::VisualLodScale>,
) {
    if !s.is_changed() {
        return;
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
    s: Res<AuditSettings>,
    new_meshes: Query<(), Added<Mesh3d>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut composites: ResMut<Assets<TerrainCompositeMaterial>>,
    mut meshes: Query<
        (
            Entity,
            &mut Visibility,
            Option<&MeshMaterial3d<TerrainMaterial>>,
            Option<&MeshMaterial3d<TerrainCompositeMaterial>>,
            Option<&mut AuditMesh>,
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
        let initial = AuditMesh {
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
    fn fps_button_changes_only_pacing_and_reset_restores_custom_launch_rate() {
        #[derive(Resource, Default)]
        struct SceneWrites(u32);
        let mut app = App::new();
        app.init_resource::<AuditSettings>()
            .init_resource::<FramePacing>()
            .init_resource::<Time>()
            .init_resource::<performance::PanelState>()
            .init_resource::<SceneWrites>()
            .add_systems(
                Update,
                (
                    buttons,
                    |settings: Res<AuditSettings>, mut count: ResMut<SceneWrites>| {
                        if settings.is_changed() {
                            count.0 += 1;
                        }
                    },
                )
                    .chain(),
            );
        app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(45);
        app.world_mut()
            .resource_mut::<performance::PanelState>()
            .baseline_rate = FrameRate::new(45);
        app.update();
        let before = app.world().resource::<SceneWrites>().0;
        let button = app
            .world_mut()
            .spawn((Interaction::Pressed, Control::FrameRate))
            .id();
        app.update();
        assert_eq!(app.world().resource::<FramePacing>().rate.fps(), 0);
        assert_eq!(app.world().resource::<SceneWrites>().0, before);
        assert_eq!(
            app.world().resource::<FramePacing>().control_label(),
            "FPS limit: Follow display"
        );
        app.world_mut().entity_mut(button).insert(Control::Reset);
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(app.world().resource::<FramePacing>().rate.fps(), 45);
        // Profiling must retain the launch rate even if a user presses the control.
        app.world_mut().resource_mut::<FramePacing>().profile_locked = true;
        app.world_mut()
            .entity_mut(button)
            .insert(Control::FrameRate);
        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(app.world().resource::<FramePacing>().rate.fps(), 45);
        assert!(
            app.world()
                .resource::<FramePacing>()
                .control_label()
                .contains("profile locked")
        );
    }
    use bevy::{
        input::{
            InputPlugin,
            touch::{TouchInput, TouchPhase},
        },
        window::WindowResolution,
    };

    #[test]
    fn ground_audit_includes_hierarchy_materials_and_restores_visibility() {
        let mut app = App::new();
        app.init_resource::<Assets<TerrainMaterial>>()
            .init_resource::<Assets<TerrainCompositeMaterial>>()
            .insert_resource(AuditSettings {
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
        app.world_mut().resource_mut::<AuditSettings>().scene = Scene::Clear;
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(ground).unwrap(),
            Visibility::Hidden
        );
        *app.world_mut().resource_mut::<AuditSettings>() = AuditSettings::default();
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
        app.init_resource::<AuditSettings>()
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
        app.world_mut().resource_mut::<AuditSettings>().bloom = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(object),
            Some(&Visibility::Hidden)
        );
        app.world_mut().resource_mut::<AuditSettings>().hide_objects = true;
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
        app.world_mut().resource_mut::<AuditSettings>().hide_objects = false;
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

    #[test]
    fn hud_captures_touch_through_release_but_leaves_world_input_enabled() {
        let mut app = App::new();
        app.add_plugins(InputPlugin)
            .init_resource::<AuditSettings>()
            .init_resource::<GameInputEnabled>()
            .init_resource::<GamePointerInputBlocked>()
            .add_systems(Update, route_input);
        let window = app
            .world_mut()
            .spawn((
                Window {
                    resolution: WindowResolution::new(1200, 900).with_scale_factor_override(3.0),
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        // Physical UI rectangle corresponds to logical x=200..400, y=200..300 at 3x DPI.
        app.world_mut().spawn((
            AuditPanel,
            ComputedNode {
                size: Vec2::new(600.0, 300.0),
                ..default()
            },
            UiGlobalTransform::from_translation(Vec2::new(900.0, 750.0)),
            InheritedVisibility::VISIBLE,
        ));
        app.update();
        assert!(app.world().resource::<GameInputEnabled>().0);
        assert!(!app.world().resource::<GamePointerInputBlocked>().0);

        let touch = |app: &mut App, phase, position| {
            app.world_mut().write_message(TouchInput {
                phase,
                position,
                window,
                force: None,
                id: 1,
            });
            app.update();
        };
        touch(&mut app, TouchPhase::Started, Vec2::new(300.0, 250.0));
        assert!(app.world().resource::<GamePointerInputBlocked>().0);
        assert!(
            app.world().resource::<GameInputEnabled>().0,
            "HUD capture must not globally freeze gameplay"
        );
        touch(&mut app, TouchPhase::Moved, Vec2::new(100.0, 100.0));
        assert!(app.world().resource::<GamePointerInputBlocked>().0);
        touch(&mut app, TouchPhase::Ended, Vec2::new(100.0, 100.0));
        assert!(
            app.world().resource::<GamePointerInputBlocked>().0,
            "release outside HUD must stay captured"
        );
        app.update();
        assert!(!app.world().resource::<GamePointerInputBlocked>().0);
        touch(&mut app, TouchPhase::Started, Vec2::new(100.0, 100.0));
        assert!(!app.world().resource::<GamePointerInputBlocked>().0);
        touch(&mut app, TouchPhase::Ended, Vec2::new(100.0, 100.0));
        assert!(!app.world().resource::<GamePointerInputBlocked>().0);

        app.world_mut()
            .resource_mut::<AuditSettings>()
            .controls_locked = true;
        app.update();
        assert!(!app.world().resource::<GameInputEnabled>().0);
        app.world_mut()
            .resource_mut::<AuditSettings>()
            .controls_locked = false;
        app.update();
        assert!(app.world().resource::<GameInputEnabled>().0);
    }
}
