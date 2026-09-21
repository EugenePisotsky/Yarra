//! Optional F1 controls and diagnostics. Game configuration lives in runtime_settings.
mod logging;
mod performance;
mod timing;

use crate::frame_pacing::{FramePacing, FrameRate};
use crate::game_render::{
    GameRenderAssets as AuditAssets, RESOLUTION_SCALES, RenderPath as AuditRenderPath,
};
use crate::runtime_settings::{RuntimeSettings, RuntimeSettingsApply, Scene};
use bevy::{prelude::*, window::PrimaryWindow};
use engine::{GamePointerInputBlocked, GameplaySystems};
use terrain_render::TerrainShadingMode;
use vegetation_render::VegetationProfileMode;

/// Composition is chosen at launch. Hiding F1 does not uninstall instrumentation.
pub(crate) fn install(app: &mut App) {
    let options = app
        .world()
        .resource::<crate::launch::LaunchOptions>()
        .clone();
    info!(
        "DIAGNOSTICS mode={:?} gpu_timing_off={} grass_counters={}",
        options.diagnostics, options.gpu_off, options.counters
    );
    if options.audit_log {
        app.add_systems(
            PostUpdate,
            logging::log_status.after(TransformSystems::Propagate),
        );
    }
    if options.diagnostics == crate::launch::DiagnosticsMode::Off {
        return;
    }
    app.add_plugins(PerformancePanelPlugin);
    if options.diagnostics == crate::launch::DiagnosticsMode::Full {
        // Bevy's recorder is separate from our frame timestamps. Keep both optional.
        #[cfg(not(target_os = "ios"))]
        if !options.gpu_off {
            app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
        }
        // Finish last: the recorder adds systems in finish(), and our CPU wrappers
        // must see those systems too, as they did before diagnostics were optional.
        app.add_plugins(timing::TimingPlugin {
            log: options.timing_log,
            gpu_off: options.gpu_off,
        });
    }
}

pub(crate) struct PerformancePanelPlugin;
impl Plugin for PerformancePanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<timing::History>()
            .init_resource::<timing::Stamp>()
            .add_systems(
                Update,
                (buttons, update_labels, route_input)
                    .chain()
                    .before(RuntimeSettingsApply)
                    .before(GameplaySystems::CameraInput),
            )
            .add_systems(Last, update_stamp.before(timing::CollectTimings));
        performance::install(app);
    }
}

fn update_stamp(
    frame: Res<bevy::diagnostic::FrameCount>,
    settings: Res<RuntimeSettings>,
    mut stamp: ResMut<timing::Stamp>,
    pacing: Res<FramePacing>,
) {
    stamp.frame = frame.0;
    stamp.detailed = settings.gpu_pass_timings;
    if settings.is_changed() || pacing.is_changed() {
        stamp.epoch = stamp.epoch.wrapping_add(1);
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
    PageGizmos,
    TerrainMacro,
    ReloadCanopy,
    ObjectDetail,
}

impl Control {
    fn label(self, s: &RuntimeSettings, frame_rate: FrameRate) -> String {
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
            Self::PageGizmos => format!("Terrain page gizmos: {}", on_off(s.page_gizmos)),
            Self::TerrainMacro => format!("Terrain macro: {}", s.terrain_macro.label()),
            Self::ReloadCanopy => "Reload canopy look".into(),
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

fn font(size: f32) -> TextFont {
    TextFont {
        font_size: FontSize::Px(size),
        ..default()
    }
}

fn buttons(
    options: Option<Res<crate::launch::LaunchOptions>>,
    interactions: Query<(&Interaction, &Control), Changed<Interaction>>,
    mut s: ResMut<RuntimeSettings>,
    time: Res<Time>,
    mut session: ResMut<performance::CaptureSession>,
    mut pacing: ResMut<FramePacing>,
) {
    if session.recording() {
        return;
    }
    for (interaction, control) in &interactions {
        if *interaction != Interaction::Pressed {
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
            Control::PageGizmos => s.page_gizmos = !s.page_gizmos,
            Control::TerrainMacro => s.terrain_macro = s.terrain_macro.toggled(),
            Control::ReloadCanopy => {
                let path = options
                    .as_deref()
                    .cloned()
                    .unwrap_or_default()
                    .canopy_path();
                match crate::runtime_settings::load_canopy(&path) {
                    Ok(look) => {
                        s.canopy = look;
                        session.message =
                            "Canopy look reloaded; Reset restores the launch values.".into();
                    }
                    Err(error) => {
                        session.message = format!("Canopy reload failed: {error}");
                        warn!("Canopy reload failed: {error}");
                        continue;
                    }
                }
            }
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
            Control::GpuPassTimings => {
                if options.as_ref().is_some_and(|o| {
                    o.diagnostics != crate::launch::DiagnosticsMode::Full || o.gpu_off
                }) {
                    session.message =
                        "GPU probes disabled at launch; restart with --diagnostics full.".into();
                    continue;
                }
                s.gpu_pass_timings = !s.gpu_pass_timings;
            }
            Control::Wind => s.wind = !s.wind,
            Control::Lock => {
                s.controls_locked = !s.controls_locked;
                if !s.controls_locked && s.grass == VegetationProfileMode::DrawFrozen {
                    s.grass = VegetationProfileMode::Full;
                }
            }
            Control::Reset => {
                session.reset(&mut s, &mut pacing);
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
    s: Res<RuntimeSettings>,
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
    window: Single<&Window, With<PrimaryWindow>>,
    panels: Query<(&ComputedNode, &UiGlobalTransform, &InheritedVisibility), With<AuditPanel>>,
    touches: Res<Touches>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut mouse_captured: Local<bool>,
    mut pointer_blocked: ResMut<GamePointerInputBlocked>,
) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_settings::{apply_appearance, apply_input_lock};
    use engine::{GameInputEnabled, WorldViewCamera};

    #[test]
    fn composition_omits_panel_and_probe_plugins_when_disabled() {
        use crate::launch::{DiagnosticsMode, LaunchOptions};
        use bevy::render::{RenderApp, diagnostic::RenderDiagnosticsPlugin};
        for mode in [
            DiagnosticsMode::Off,
            DiagnosticsMode::Panel,
            DiagnosticsMode::Full,
        ] {
            for gpu_off in [false, true] {
                let mut app = App::new();
                app.add_plugins(vegetation_render::VegetationRenderPlugin);
                app.insert_sub_app(RenderApp, SubApp::new());
                app.insert_resource(LaunchOptions {
                    diagnostics: mode,
                    gpu_off,
                    ..default()
                });
                install(&mut app);
                assert_eq!(
                    app.is_plugin_added::<PerformancePanelPlugin>(),
                    mode != DiagnosticsMode::Off
                );
                assert_eq!(
                    app.is_plugin_added::<timing::TimingPlugin>(),
                    mode == DiagnosticsMode::Full
                );
                assert_eq!(
                    app.is_plugin_added::<RenderDiagnosticsPlugin>(),
                    mode == DiagnosticsMode::Full && !gpu_off && !cfg!(target_os = "ios")
                );
                assert_eq!(
                    app.world().contains_resource::<performance::PanelState>(),
                    mode != DiagnosticsMode::Off
                );
                assert_eq!(
                    app.world()
                        .contains_resource::<performance::CaptureSession>(),
                    mode != DiagnosticsMode::Off
                );
                assert_eq!(
                    app.world().contains_resource::<timing::History>(),
                    mode != DiagnosticsMode::Off
                );
            }
        }
    }

    #[test]
    fn runtime_startup_and_changes_work_with_or_without_the_panel() {
        use crate::game_render::GameRenderSettings;
        use crate::launch::{DiagnosticsMode, LaunchOptions};
        use crate::runtime_settings::RuntimeSettingsPlugin;
        use vegetation_render::{VegetationDebugSettings, VegetationLighting, VegetationWind};
        let canopy = vegetation::CanopyShading {
            strength: 0.27,
            ..default()
        };
        for mode in [DiagnosticsMode::Off, DiagnosticsMode::Panel] {
            let mut app = App::new();
            app.add_plugins(bevy::input::InputPlugin)
                .insert_resource(LaunchOptions {
                    diagnostics: mode,
                    upscaler: upscaling::UpscaleMethod::Linear,
                    ..default()
                })
                .init_resource::<Time>()
                .init_resource::<Time<Real>>()
                .init_resource::<bevy::diagnostic::FrameCount>()
                .init_resource::<FramePacing>()
                .init_resource::<GameRenderSettings>()
                .init_resource::<GameInputEnabled>()
                .init_resource::<GamePointerInputBlocked>()
                .init_resource::<engine::AtmospherePresentation>()
                .init_resource::<engine::AtmosphereState>()
                .init_resource::<engine::TerrainLodPreview>()
                .init_resource::<engine::VisualLodScale>()
                .init_resource::<engine::TerrainLodStats>()
                .init_resource::<engine::StreamingStats>()
                .init_resource::<VegetationWind>()
                .init_resource::<vegetation_render::VegetationDiagnostics>()
                .init_resource::<terrain_render::TerrainPreparedSettings>()
                .init_resource::<terrain_render::TerrainMacroVariation>()
                .init_resource::<Assets<terrain_render::TerrainMaterial>>()
                .init_resource::<Assets<terrain_render::composite::TerrainCompositeMaterial>>()
                .insert_resource(engine::CloudQuality::Off)
                .insert_resource(VegetationLighting {
                    canopy,
                    ..default()
                })
                .insert_resource(VegetationDebugSettings {
                    density_mode: vegetation_render::VegetationDensityMode::Authored,
                    ..default()
                })
                .add_plugins(RuntimeSettingsPlugin);
            app.world_mut().spawn((Window::default(), PrimaryWindow));
            let camera = app
                .world_mut()
                .spawn((
                    Camera::default(),
                    GlobalTransform::default(),
                    WorldViewCamera,
                ))
                .id();
            app.world_mut()
                .spawn((DirectionalLight::default(), engine::WorldSun));
            install(&mut app);
            app.update();
            let settings = app.world().resource::<RuntimeSettings>();
            assert_eq!(settings.canopy, canopy);
            assert_eq!(settings.clouds, engine::CloudQuality::Off);
            assert_eq!(
                settings.density,
                vegetation_render::VegetationDensityMode::Authored
            );
            assert_eq!(
                app.world().resource::<GameRenderSettings>().upscaler,
                upscaling::UpscaleMethod::Linear
            );
            assert_eq!(app.world().resource::<VegetationLighting>().canopy, canopy);
            assert_eq!(
                app.world().contains_resource::<performance::PanelState>(),
                mode == DiagnosticsMode::Panel
            );
            {
                let mut settings = app.world_mut().resource_mut::<RuntimeSettings>();
                settings.controls_locked = true;
                settings.prepass = false;
                settings.grass = VegetationProfileMode::Disabled;
                settings.clouds = engine::CloudQuality::High;
            }
            app.update();
            assert!(!app.world().resource::<GameInputEnabled>().0);
            assert!(
                !app.world()
                    .entity(camera)
                    .contains::<bevy::core_pipeline::prepass::DepthPrepass>()
            );
            assert_eq!(
                app.world()
                    .resource::<VegetationDebugSettings>()
                    .profile_mode,
                VegetationProfileMode::Disabled
            );
            assert_eq!(
                *app.world().resource::<engine::CloudQuality>(),
                engine::CloudQuality::High
            );
            if mode == DiagnosticsMode::Panel {
                // Baseline must be captured after runtime initialization, not before it.
                app.world_mut()
                    .spawn((Interaction::Pressed, Control::Reset));
                app.update();
                assert!(app.world().resource::<GameInputEnabled>().0);
                assert_eq!(
                    *app.world().resource::<engine::CloudQuality>(),
                    engine::CloudQuality::Off
                );
                assert_eq!(
                    app.world()
                        .resource::<VegetationDebugSettings>()
                        .density_mode,
                    vegetation_render::VegetationDensityMode::Authored
                );
                assert_eq!(app.world().resource::<VegetationLighting>().canopy, canopy);
            }
        }
    }

    #[test]
    fn canopy_reload_and_reset_restore_values_without_rereading_the_file() {
        let path =
            std::env::temp_dir().join(format!("yarra-canopy-reset-{}.ron", std::process::id()));
        let initial = vegetation::CanopyShading {
            strength: 0.25,
            ..default()
        };
        let edited = vegetation::CanopyShading {
            strength: 0.85,
            enabled: true,
            ..default()
        };
        std::fs::write(&path, ron::to_string(&edited).unwrap()).unwrap();
        let mut app = App::new();
        app.insert_resource(RuntimeSettings {
            canopy: initial,
            ..default()
        })
        .insert_resource(crate::launch::LaunchOptions {
            canopy_path: Some(path.clone()),
            ..default()
        })
        .init_resource::<FramePacing>()
        .init_resource::<Time>()
        .init_resource::<performance::CaptureSession>()
        .init_resource::<vegetation_render::VegetationLighting>()
        .init_resource::<terrain_render::TerrainMacroVariation>()
        .add_systems(Update, (buttons, apply_appearance).chain());
        app.world_mut()
            .resource_mut::<performance::CaptureSession>()
            .remember_launch(
                &RuntimeSettings {
                    canopy: initial,
                    ..default()
                },
                FrameRate::default(),
            );
        let button = app
            .world_mut()
            .spawn((Interaction::Pressed, Control::ReloadCanopy))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .resource::<vegetation_render::VegetationLighting>()
                .canopy,
            edited
        );
        std::fs::remove_file(path).unwrap();
        app.world_mut()
            .entity_mut(button)
            .insert((Interaction::Pressed, Control::TerrainMacro));
        app.update();
        assert_eq!(
            *app.world()
                .resource::<terrain_render::TerrainMacroVariation>(),
            terrain_render::TerrainMacroVariation::default().toggled()
        );
        app.world_mut()
            .entity_mut(button)
            .insert((Interaction::Pressed, Control::Reset));
        app.update();
        assert_eq!(
            app.world()
                .resource::<vegetation_render::VegetationLighting>()
                .canopy,
            initial
        );
        assert_eq!(
            *app.world()
                .resource::<terrain_render::TerrainMacroVariation>(),
            terrain_render::TerrainMacroVariation::default()
        );
    }

    #[test]
    fn fps_button_changes_only_pacing_and_reset_restores_custom_launch_rate() {
        #[derive(Resource, Default)]
        struct SceneWrites(u32);
        let mut app = App::new();
        app.init_resource::<RuntimeSettings>()
            .init_resource::<FramePacing>()
            .init_resource::<Time>()
            .init_resource::<performance::CaptureSession>()
            .init_resource::<SceneWrites>()
            .add_systems(
                Update,
                (
                    buttons,
                    |settings: Res<RuntimeSettings>, mut count: ResMut<SceneWrites>| {
                        if settings.is_changed() {
                            count.0 += 1;
                        }
                    },
                )
                    .chain(),
            );
        app.world_mut().resource_mut::<FramePacing>().rate = FrameRate::new(45);
        app.world_mut()
            .resource_mut::<performance::CaptureSession>()
            .remember_launch(&RuntimeSettings::default(), FrameRate::new(45));
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
    fn hud_captures_touch_through_release_but_leaves_world_input_enabled() {
        let mut app = App::new();
        app.add_plugins(InputPlugin)
            .init_resource::<RuntimeSettings>()
            .init_resource::<GameInputEnabled>()
            .init_resource::<GamePointerInputBlocked>()
            .add_systems(Update, (route_input, apply_input_lock));
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
            .resource_mut::<RuntimeSettings>()
            .controls_locked = true;
        app.update();
        assert!(!app.world().resource::<GameInputEnabled>().0);
        app.world_mut()
            .resource_mut::<RuntimeSettings>()
            .controls_locked = false;
        app.update();
        assert!(app.world().resource::<GameInputEnabled>().0);
    }
}
