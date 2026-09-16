//! Opt-in A/B controls with normal gameplay input and an explicit profiling lock.
mod baseline;
mod logging;
mod repro;

use bevy::{
    core_pipeline::prepass::DepthPrepass,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    light::ShadowFilteringMethod,
    prelude::*,
    window::PrimaryWindow,
};
use engine::{
    GAME_DEPTH_PREPASS_ENABLED, GameInputEnabled, GameInputSystems, GamePointerInputBlocked,
    WorldSun, WorldViewCamera,
};
use terrain_render::{TerrainMaterial, TerrainShadingMode};
use vegetation_render::{
    VegetationDebugSettings, VegetationLightingMode, VegetationProfileMode, VegetationWind,
};

use crate::game_render::{
    GameRenderAssets as AuditAssets, GameRenderSettings, GameRenderSetup, GameRenderSystems,
    RenderPath as AuditRenderPath,
};

pub struct RenderAuditPlugin;

impl Plugin for RenderAuditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuditSettings>()
            .init_resource::<baseline::BaselineRun>()
            .add_systems(
                PostStartup,
                (sync_render_settings.before(GameRenderSetup), setup),
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
                    status,
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
                logging::log_status.after(TransformSystems::Propagate),
            );
        repro::install(app);
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

#[derive(Resource, Clone)]
struct AuditSettings {
    scene: Scene,
    grass: VegetationProfileMode,
    unlit: bool,
    ground_shading: TerrainShadingMode,
    terrain_prepared: bool,
    shadows: u8,
    prepass: bool,
    scale_index: usize,
    msaa: Msaa,
    counters: bool,
    wind: bool,
    controls_locked: bool,
    render_path: AuditRenderPath,
    show_ui: bool,
    baseline_phase: Option<&'static str>,
    changed_at: f64,
}

impl Default for AuditSettings {
    fn default() -> Self {
        let render = GameRenderSettings::default();
        Self {
            scene: Scene::Current,
            grass: VegetationProfileMode::Full,
            unlit: false,
            ground_shading: TerrainShadingMode::Production,
            terrain_prepared: crate::terrain_prepared_enabled(),
            shadows: 0,
            prepass: GAME_DEPTH_PREPASS_ENABLED,
            scale_index: [1.0, 0.75, 0.5]
                .iter()
                .position(|&scale| scale == render.resolution_scale)
                .expect("game scale is available in audit controls"),
            msaa: render.msaa,
            // Statistics atomics are explicit on every platform, including audit startup.
            counters: std::env::args_os().any(|arg| arg == "--grass-counters"),
            wind: true,
            controls_locked: false,
            render_path: render.render_path,
            show_ui: render.show_ui,
            baseline_phase: None,
            changed_at: 0.0,
        }
    }
}

impl AuditSettings {
    fn scale(&self) -> f32 {
        match self.render_path {
            AuditRenderPath::Direct => 1.0,
            AuditRenderPath::Composite => [1.0, 0.75, 0.5][self.scale_index],
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
    Scene,
    Grass,
    Shading,
    GroundMaterial,
    Shadows,
    Prepass,
    Scale,
    Counters,
    Wind,
    Lock,
    Reset,
    RenderPath,
    Antialiasing,
    Baseline,
}

impl Control {
    fn label(self, s: &AuditSettings) -> String {
        match self {
            Self::Scene => format!("Scene: {:?}", s.scene),
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
            Self::Prepass => format!("Depth prepass: {}", on_off(s.prepass)),
            Self::Scale => format!("Resolution: {}%", [100, 75, 50][s.scale_index]),
            Self::Counters => format!("GPU counters: {}", on_off(s.counters)),
            Self::Wind => format!("Wind: {}", on_off(s.wind)),
            Self::Lock => format!("Lock controls: {}", on_off(s.controls_locked)),
            Self::Reset => "Reset baseline".into(),
            Self::RenderPath => format!("Render: {}", s.render_path.label()),
            Self::Antialiasing => match s.msaa {
                Msaa::Off => "AA: off".into(),
                _ => format!("AA: {}x MSAA", s.msaa.samples()),
            },
            Self::Baseline => if s.baseline_phase.is_some() {
                "Stop baseline test"
            } else {
                "Baseline test (160s)"
            }
            .into(),
        }
    }
}

fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

#[derive(Component)]
struct AuditStatus;

#[derive(Component)]
struct AuditPanel;

#[derive(Component)]
struct AuditMesh {
    original_visibility: Visibility,
    terrain: Option<Handle<TerrainMaterial>>,
}

fn setup(mut commands: Commands, settings: Res<AuditSettings>) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: px(12),
                bottom: px(12),
                width: px(432),
                padding: UiRect::all(px(8)),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.03, 0.04, 0.96)),
            GlobalZIndex(100),
            AuditPanel,
        ))
        .with_children(|panel| {
            let title = if cfg!(target_os = "ios") {
                "Render audit | target 60 FPS"
            } else {
                "Render audit | VSync"
            };
            panel.spawn((Text::new(title), font(13.0)));
            panel.spawn((Text::new("Starting..."), font(12.0), AuditStatus));
            panel
                .spawn(Node {
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: px(6),
                    row_gap: px(6),
                    ..default()
                })
                .with_children(|row| {
                    for control in [
                        Control::Scene,
                        Control::Grass,
                        Control::Shading,
                        Control::GroundMaterial,
                        Control::Shadows,
                        Control::Prepass,
                        Control::Scale,
                        Control::Counters,
                        Control::Wind,
                        Control::Lock,
                        Control::Reset,
                        Control::RenderPath,
                        Control::Baseline,
                        Control::Antialiasing,
                    ] {
                        row.spawn((
                            Button,
                            control,
                            Node {
                                width: px(204),
                                min_height: px(34),
                                padding: UiRect::all(px(6)),
                                align_items: AlignItems::Center,
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.13, 0.19, 0.23)),
                        ))
                        .with_child((Text::new(control.label(&settings)), font(12.0)));
                    }
                });
        });
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
    mut baseline: ResMut<baseline::BaselineRun>,
    time: Res<Time>,
) {
    for (interaction, control) in &interactions {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if matches!(control, Control::Baseline) {
            baseline.toggle_requested = true;
            continue;
        }
        if s.baseline_phase.is_some() {
            continue;
        }
        match control {
            Control::Scene => {
                s.scene = match s.scene {
                    Scene::Current => Scene::Clear,
                    Scene::Clear => Scene::Ground,
                    Scene::Ground => Scene::Grass,
                    Scene::Grass => Scene::Current,
                };
                s.grass = VegetationProfileMode::Full;
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
                s.scale_index = (s.scale_index + 1) % 3;
            }
            Control::Counters => s.counters = !s.counters,
            Control::Wind => s.wind = !s.wind,
            Control::Lock => {
                s.controls_locked = !s.controls_locked;
                if !s.controls_locked && s.grass == VegetationProfileMode::DrawFrozen {
                    s.grass = VegetationProfileMode::Full;
                }
            }
            Control::Reset => *s = AuditSettings::default(),
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
            Control::Baseline => unreachable!(),
        }
        s.changed_at = time.elapsed_secs_f64();
    }
}

fn update_labels(
    s: Res<AuditSettings>,
    controls: Query<(&Control, &Children)>,
    mut labels: Query<&mut Text>,
) {
    if s.is_changed() {
        for (control, children) in &controls {
            for child in children {
                if let Ok(mut text) = labels.get_mut(*child) {
                    **text = control.label(&s);
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
) {
    if !s.is_changed() {
        return;
    }
    grass.profile_mode = s.grass_mode();
    grass.lighting_mode = if s.unlit {
        VegetationLightingMode::UnlitDiagnostic
    } else {
        VegetationLightingMode::RoundedGloss
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
    if s.prepass {
        commands.entity(*camera).insert(DepthPrepass);
    } else {
        commands.entity(*camera).remove::<DepthPrepass>();
    }
}

#[allow(clippy::type_complexity)] // Keep the optional original-material/state query explicit.
fn apply_meshes(
    mut commands: Commands,
    s: Res<AuditSettings>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut meshes: Query<
        (
            Entity,
            &mut Visibility,
            Option<&MeshMaterial3d<TerrainMaterial>>,
            Option<&mut AuditMesh>,
        ),
        With<Mesh3d>,
    >,
) {
    for (entity, mut visibility, material, saved) in &mut meshes {
        if saved.is_some() && !s.is_changed() {
            continue;
        }
        let initial = AuditMesh {
            original_visibility: *visibility,
            terrain: material.map(|m| m.0.clone()),
        };
        let is_new = saved.is_none();
        let mut saved = saved;
        let mut initial = initial;
        let state = saved.as_deref_mut().unwrap_or(&mut initial);
        let terrain = state.terrain.is_some();
        let desired = match s.scene {
            Scene::Current => state.original_visibility,
            Scene::Ground if terrain => Visibility::Visible,
            _ => Visibility::Hidden,
        };
        if *visibility != desired {
            *visibility = desired;
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
        if is_new {
            commands.entity(entity).insert(initial);
        }
    }
}

fn status(
    settings: Res<AuditSettings>,
    assets: Res<AuditAssets>,
    images: Res<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    diagnostics: Res<DiagnosticsStore>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
    mut text: Single<&mut Text, With<AuditStatus>>,
) {
    *elapsed += time.delta_secs();
    if *elapsed < 0.5 {
        return;
    }
    *elapsed = 0.0;
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or_default();
    let size = match settings.render_path {
        AuditRenderPath::Composite => images.get(&assets.target).unwrap().size(),
        AuditRenderPath::Direct => window.physical_size(),
    };
    let phase = settings.baseline_phase.unwrap_or("manual");
    **text = Text::new(format!(
        "{fps:.1} FPS | 3D {}x{} | {:.0}s since change\n{phase} | Metal HUD: GPU time / thermal state",
        size.x,
        size.y,
        time.elapsed_secs_f64() - settings.changed_at
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
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
