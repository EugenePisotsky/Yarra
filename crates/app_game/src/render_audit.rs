//! Opt-in A/B controls with normal gameplay input and an explicit profiling lock.
mod baseline;
mod logging;
mod repro;

use bevy::{
    camera::{ImageRenderTarget, RenderTarget},
    core_pipeline::prepass::DepthPrepass,
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    image::ImageSampler,
    light::ShadowFilteringMethod,
    prelude::*,
    render::render_resource::{Extent3d, TextureFormat},
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

pub struct RenderAuditPlugin;

impl Plugin for RenderAuditPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuditSettings>()
            .init_resource::<baseline::BaselineRun>()
            .add_systems(PostStartup, setup)
            .add_systems(
                Update,
                (
                    buttons,
                    baseline::advance,
                    update_labels,
                    route_input,
                    apply_render_path,
                    apply_settings,
                    status,
                )
                    .chain()
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AuditRenderPath {
    Composite,
    #[default]
    Direct,
}

impl AuditRenderPath {
    fn label(self) -> &'static str {
        match self {
            Self::Composite => "composite",
            Self::Direct => "direct",
        }
    }
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
        Self {
            scene: Scene::Current,
            grass: VegetationProfileMode::Full,
            unlit: false,
            ground_shading: TerrainShadingMode::Production,
            terrain_prepared: std::env::args_os().any(|arg| arg == "--terrain-prepared"),
            shadows: 0,
            prepass: GAME_DEPTH_PREPASS_ENABLED,
            scale_index: 0,
            msaa: Msaa::Sample4,
            // iOS cannot currently read these statistics back. Do not pay a global atomic
            // for every candidate during normal device runs; Metal HUD is independent.
            counters: !cfg!(target_os = "ios"),
            wind: true,
            controls_locked: false,
            render_path: AuditRenderPath::Direct,
            show_ui: true,
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
struct AuditUiCamera;

#[derive(Component)]
struct AuditComposite;

#[derive(Component)]
struct SavedAuditUiVisibility(Visibility);

#[derive(Component)]
struct AuditMesh {
    original_visibility: Visibility,
    terrain: Option<Handle<TerrainMaterial>>,
}

#[derive(Resource)]
struct AuditAssets {
    target: Handle<Image>,
    original_target: RenderTarget,
    original_camera_order: isize,
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(Entity, &mut Camera, &RenderTarget), With<WorldViewCamera>>,
    settings: Res<AuditSettings>,
) {
    // Composite preserves native UI when scaling 3D. Direct can bypass this extra path at
    // 100% so the audit's own rendering overhead can be measured.
    // Direct does not need a native-sized offscreen allocation. The composite path resizes
    // this placeholder on demand before drawing into it.
    let initial_size = if settings.render_path == AuditRenderPath::Composite {
        window.physical_size().max(UVec2::ONE)
    } else {
        UVec2::ONE
    };
    let mut image = Image::new_target_texture(
        initial_size.x,
        initial_size.y,
        TextureFormat::Bgra8UnormSrgb,
        None,
    );
    image.sampler = ImageSampler::linear();
    let target = images.add(image);
    let original_target = camera.2.clone();
    let original_camera_order = camera.1.order;
    camera.1.order = -1;
    commands
        .entity(camera.0)
        .insert(RenderTarget::Image(ImageRenderTarget {
            handle: target.clone(),
            scale_factor: window.scale_factor(),
        }));
    commands.spawn((
        Camera2d,
        Msaa::Off,
        IsDefaultUiCamera,
        AuditUiCamera,
        Name::new("Render audit UI camera"),
    ));
    commands.spawn((
        ImageNode::new(target.clone()),
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        GlobalZIndex(-100),
        AuditComposite,
    ));
    commands.insert_resource(AuditAssets {
        target,
        original_target,
        original_camera_order,
    });
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

#[allow(clippy::type_complexity)] // Disjoint camera/UI queries and saved root visibility.
fn apply_render_path(
    mut commands: Commands,
    s: Res<AuditSettings>,
    assets: Res<AuditAssets>,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(Entity, &mut Camera, &RenderTarget), With<WorldViewCamera>>,
    mut ui_camera: Single<(Entity, &mut Camera), (With<AuditUiCamera>, Without<WorldViewCamera>)>,
    mut composite: Single<&mut Visibility, With<AuditComposite>>,
    mut ui_roots: Query<
        (Entity, &mut Visibility, Option<&SavedAuditUiVisibility>),
        (With<Node>, Without<ChildOf>, Without<AuditComposite>),
    >,
) {
    let scale = s.scale();
    let size = (window.physical_size().as_vec2() * scale)
        .as_uvec2()
        .max(UVec2::ONE);
    let target_scale = window.scale_factor() * scale;
    let composite_path = s.render_path == AuditRenderPath::Composite;
    let needs_image_target = !matches!(camera.2, RenderTarget::Image(target)
        if target.handle == assets.target && target.scale_factor == target_scale);
    if composite_path && needs_image_target {
        // Preserve logical viewport dimensions (and tree LOD) while varying physical pixels.
        commands
            .entity(camera.0)
            .insert(RenderTarget::Image(ImageRenderTarget {
                handle: assets.target.clone(),
                scale_factor: target_scale,
            }));
    }
    if composite_path
        && images
            .get(&assets.target)
            .expect("audit render target")
            .size()
            != size
    {
        let mut image = images.get_mut(&assets.target).expect("audit render target");
        image.resize(Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        });
    }
    if !s.is_changed() {
        return;
    }
    camera.1.order = if composite_path {
        -1
    } else {
        assets.original_camera_order
    };
    ui_camera.1.is_active = composite_path;
    if composite_path {
        commands.entity(camera.0).remove::<IsDefaultUiCamera>();
        commands.entity(ui_camera.0).insert(IsDefaultUiCamera);
    } else {
        commands
            .entity(camera.0)
            .insert((assets.original_target.clone(), IsDefaultUiCamera));
        commands.entity(ui_camera.0).remove::<IsDefaultUiCamera>();
    }
    **composite = if composite_path {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for (entity, mut visibility, saved) in &mut ui_roots {
        if s.show_ui {
            if let Some(saved) = saved {
                *visibility = saved.0;
                commands.entity(entity).remove::<SavedAuditUiVisibility>();
            }
        } else {
            if saved.is_none() {
                commands
                    .entity(entity)
                    .insert(SavedAuditUiVisibility(*visibility));
            }
            *visibility = Visibility::Hidden;
        }
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
    commands.entity(*camera).insert(s.msaa);
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
    fn direct_rendering_bypasses_the_audit_camera_and_restores_ui_and_scaled_target() {
        let mut app = App::new();
        app.insert_resource(AuditSettings {
            render_path: AuditRenderPath::Composite,
            ..default()
        })
        .init_resource::<Assets<Image>>()
        .add_systems(Update, apply_render_path);
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(1200, 900).with_scale_factor_override(3.0),
                ..default()
            },
            PrimaryWindow,
        ));
        let target =
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::new_target_texture(
                    1200,
                    900,
                    TextureFormat::Bgra8UnormSrgb,
                    None,
                ));
        app.insert_resource(AuditAssets {
            target: target.clone(),
            original_target: RenderTarget::default(),
            original_camera_order: 0,
        });
        let main = app
            .world_mut()
            .spawn((Camera3d::default(), WorldViewCamera))
            .id();
        let ui = app
            .world_mut()
            .spawn((Camera2d, AuditUiCamera, IsDefaultUiCamera))
            .id();
        let composite = app
            .world_mut()
            .spawn((Node::default(), AuditComposite))
            .id();
        let label = app.world_mut().spawn(Node::default()).id();
        let hidden_label = app
            .world_mut()
            .spawn((Node::default(), Visibility::Hidden))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<RenderTarget>(main).unwrap().as_image(),
            Some(&target)
        );
        assert!(app.world().get::<Camera>(ui).unwrap().is_active);

        {
            let mut settings = app.world_mut().resource_mut::<AuditSettings>();
            settings.render_path = AuditRenderPath::Direct;
            settings.show_ui = false;
        }
        app.update();
        assert!(matches!(
            app.world().get::<RenderTarget>(main),
            Some(RenderTarget::Window(_))
        ));
        assert_eq!(app.world().get::<Camera>(main).unwrap().order, 0);
        assert!(!app.world().get::<Camera>(ui).unwrap().is_active);
        assert!(app.world().get::<IsDefaultUiCamera>(main).is_some());
        assert!(app.world().get::<IsDefaultUiCamera>(ui).is_none());
        assert_eq!(
            app.world().get::<Visibility>(composite),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().get::<Visibility>(label),
            Some(&Visibility::Hidden)
        );

        {
            let mut settings = app.world_mut().resource_mut::<AuditSettings>();
            settings.render_path = AuditRenderPath::Composite;
            settings.scale_index = 2;
            settings.show_ui = true;
        }
        app.update();
        assert!(app.world().get::<Camera>(ui).unwrap().is_active);
        assert_eq!(app.world().get::<Camera>(main).unwrap().order, -1);
        assert!(app.world().get::<IsDefaultUiCamera>(main).is_none());
        assert!(app.world().get::<IsDefaultUiCamera>(ui).is_some());
        assert_eq!(
            app.world().get::<Visibility>(label),
            Some(&Visibility::Inherited)
        );
        assert_eq!(
            app.world().get::<Visibility>(hidden_label),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().get::<Visibility>(composite),
            Some(&Visibility::Inherited)
        );
        let RenderTarget::Image(image) = app.world().get::<RenderTarget>(main).unwrap() else {
            panic!("scaled image target must be restored");
        };
        assert_eq!(image.scale_factor, 1.5);
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(600, 450)
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
