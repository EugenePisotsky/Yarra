//! Game presentation defaults, shared with the optional render-audit controls.
use bevy::{
    camera::{ImageRenderTarget, RenderTarget},
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureFormat},
    window::PrimaryWindow,
};
use engine::{GameplaySystems, WorldViewCamera};

pub(crate) struct GameRenderPlugin;

impl Plugin for GameRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameRenderSettings>()
            .add_plugins(upscaling::UpscalingPlugin)
            .add_systems(PostStartup, setup.in_set(GameRenderSetup))
            .add_systems(
                Update,
                apply_render_path
                    .in_set(GameRenderSystems)
                    .before(GameplaySystems::CameraInput),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GameRenderSetup;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GameRenderSystems;

pub(crate) const RESOLUTION_SCALES: [f32; 4] = [1.0, 0.75, 0.5, 1.0 / 3.0];

#[derive(Resource, Clone, PartialEq)]
pub(crate) struct GameRenderSettings {
    pub(crate) resolution_scale: f32,
    pub(crate) upscaler: upscaling::UpscaleMethod,
    pub(crate) temporal_debug: upscaling::temporal::TemporalDebug,
    /// Explicit internal pixel dimensions for controlled profiling, independent of Retina scaling.
    pub(crate) render_size: Option<UVec2>,
    pub(crate) msaa: Msaa,
    pub(crate) render_path: RenderPath,
    pub(crate) show_ui: bool,
    pub(crate) direct_temporal_output: bool,
}

impl Default for GameRenderSettings {
    fn default() -> Self {
        Self {
            resolution_scale: 0.5,
            temporal_debug: default(),
            upscaler: upscaling::UpscaleMethod::Auto,
            render_size: None,
            msaa: Msaa::Sample4,
            render_path: RenderPath::Composite,
            show_ui: true,
            direct_temporal_output: true,
        }
    }
}

impl GameRenderSettings {
    fn linear_composition(&self) -> bool {
        self.render_path == RenderPath::Composite
            && self.upscaler == upscaling::UpscaleMethod::Linear
    }

    fn target_size(&self, window: &Window) -> UVec2 {
        self.render_size.unwrap_or_else(|| {
            // Round up so odd output dimensions don't exceed the requested upscale ratio.
            (window.physical_size().as_vec2() * self.scale())
                .ceil()
                .as_uvec2()
                .max(UVec2::ONE)
        })
    }

    fn scale(&self) -> f32 {
        match self.render_path {
            RenderPath::Direct => 1.0,
            RenderPath::Composite => self.resolution_scale,
        }
    }
}

fn linear_status(input_size: UVec2, output_size: UVec2) -> upscaling::UpscaleStatus {
    upscaling::UpscaleStatus {
        requested: upscaling::UpscaleMethod::Linear,
        active: Some(upscaling::UpscaleMethod::Linear),
        reason: Some("Linear filtering during UI composition".into()),
        input_size,
        output_size,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RenderPath {
    #[default]
    Composite,
    Direct,
}

impl RenderPath {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Composite => "composite",
            Self::Direct => "direct",
        }
    }
}

#[derive(Component)]
struct GameUiCamera;

#[derive(Component)]
struct GameComposite;

#[derive(Component)]
struct SavedUiVisibility(Visibility);

#[derive(Resource)]
pub(crate) struct GameRenderAssets {
    pub(crate) target: Handle<Image>,
    pub(crate) upscaled: Handle<Image>,
    original_target: RenderTarget,
    original_camera_order: isize,
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(Entity, &mut Camera, &RenderTarget), With<WorldViewCamera>>,
    settings: Res<GameRenderSettings>,
) {
    // Scale only the world; the composite and gameplay UI use the native window.
    // Direct rendering keeps a tiny placeholder until scaling is requested.
    let initial_size = if settings.render_path == RenderPath::Composite {
        settings.target_size(&window)
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
    let linear = settings.linear_composition();
    let upscaled = images.add(upscaling::output_image(
        if settings.render_path == RenderPath::Composite && !linear {
            window.physical_size()
        } else {
            UVec2::ONE
        },
    ));
    let original_target = camera.2.clone();
    let original_camera_order = camera.1.order;
    camera.1.order = -1;
    commands
        .entity(camera.0)
        .insert(RenderTarget::Image(ImageRenderTarget {
            handle: target.clone(),
            scale_factor: window.scale_factor() * settings.scale(),
        }));
    if linear {
        commands
            .entity(camera.0)
            .insert(linear_status(initial_size, window.physical_size()));
    } else {
        commands.entity(camera.0).insert(upscaling::UpscaleView {
            input: target.clone(),
            output: upscaled.clone(),
            method: settings.upscaler,
            enabled: settings.render_path == RenderPath::Composite,
        });
    }
    commands.spawn((
        Camera2d,
        Msaa::Off,
        IsDefaultUiCamera,
        GameUiCamera,
        Name::new("Game UI camera"),
    ));
    commands.spawn((
        ImageNode::new(if linear {
            target.clone()
        } else {
            upscaled.clone()
        }),
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        GlobalZIndex(-100),
        GameComposite,
    ));
    commands.insert_resource(GameRenderAssets {
        target,
        upscaled,
        original_target,
        original_camera_order,
    });
}

#[allow(clippy::type_complexity)] // Disjoint camera/UI queries and saved root visibility.
fn apply_render_path(
    mut commands: Commands,
    s: Res<GameRenderSettings>,
    assets: Res<GameRenderAssets>,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<
        (
            Entity,
            &mut Camera,
            &RenderTarget,
            Option<&upscaling::temporal::TemporalView>,
            Option<&upscaling::UpscaleView>,
            Option<&upscaling::UpscaleStatus>,
        ),
        With<WorldViewCamera>,
    >,
    mut ui_camera: Single<(Entity, &mut Camera), (With<GameUiCamera>, Without<WorldViewCamera>)>,
    mut composite: Single<(&mut Visibility, &mut ImageNode), With<GameComposite>>,
    mut ui_roots: Query<
        (Entity, &mut Visibility, Option<&SavedUiVisibility>),
        (With<Node>, Without<ChildOf>, Without<GameComposite>),
    >,
) {
    let scale = s.scale();
    let size = s.target_size(&window);
    let linear = s.linear_composition();
    let temporal = s.render_path == RenderPath::Composite
        && s.upscaler == upscaling::UpscaleMethod::MetalFxTemporal;
    let target_scale = if temporal {
        window.scale_factor()
    } else if s.render_size.is_some() {
        size.y as f32 / window.height().max(1.0)
    } else {
        window.scale_factor() * scale
    };
    let composite_path = s.render_path == RenderPath::Composite;
    // Linear filtering is already performed by the composite's image sampler.
    // Avoid the redundant native-sized intermediate and fullscreen copy pass.
    let upscale_size = if linear {
        UVec2::ONE
    } else {
        window.physical_size().max(UVec2::ONE)
    };
    if composite_path
        && images.get(&assets.upscaled).expect("upscale target").size() != upscale_size
    {
        images.get_mut(&assets.upscaled).unwrap().resize(Extent3d {
            width: upscale_size.x,
            height: upscale_size.y,
            depth_or_array_layers: 1,
        });
    }
    let world_target = &assets.target;
    let image_size = if temporal {
        window.physical_size().max(UVec2::ONE)
    } else {
        size
    };
    let presentation = if temporal || linear {
        &assets.target
    } else {
        &assets.upscaled
    };
    if &composite.1.image != presentation {
        composite.1.image = presentation.clone();
    }
    if temporal {
        let request = upscaling::temporal::TemporalView {
            input_size: size,
            reset_epoch: 0,
            debug: s.temporal_debug,
        };
        if camera.3 != Some(&request) {
            commands
                .entity(camera.0)
                .insert((request, bevy::camera::MainPassResolutionOverride(size)));
            // The world camera has HDR effects only; UI is composited afterward.
            // Retain the original output path for matched performance comparisons.
            if s.direct_temporal_output {
                commands
                    .entity(camera.0)
                    .insert(upscaling::DirectTonemapOutput);
            }
        }
    } else if camera.3.is_some() {
        commands.entity(camera.0).remove::<(
            upscaling::temporal::TemporalView,
            upscaling::DirectTonemapOutput,
            bevy::camera::MainPassResolutionOverride,
            bevy::render::camera::TemporalJitter,
            bevy::render::camera::MipBias,
            bevy::core_pipeline::prepass::MotionVectorPrepass,
        )>();
    }
    let needs_image_target = !matches!(camera.2, RenderTarget::Image(target)
        if &target.handle == world_target && target.scale_factor == target_scale);
    if composite_path && needs_image_target {
        // Preserve logical viewport dimensions (and tree LOD) while varying physical pixels.
        commands
            .entity(camera.0)
            .insert(RenderTarget::Image(ImageRenderTarget {
                handle: world_target.clone(),
                scale_factor: target_scale,
            }));
    }
    if composite_path
        && images
            .get(&assets.target)
            .expect("game render target")
            .size()
            != image_size
    {
        let mut image = images.get_mut(&assets.target).expect("game render target");
        image.resize(Extent3d {
            width: image_size.x,
            height: image_size.y,
            depth_or_array_layers: 1,
        });
    }
    if linear {
        if camera.4.is_some() {
            commands.entity(camera.0).remove::<upscaling::UpscaleView>();
        }
        let status = linear_status(size, window.physical_size());
        if camera.5 != Some(&status) {
            commands.entity(camera.0).insert(status);
        }
    }
    if !s.is_changed() {
        return;
    }
    if !linear {
        commands.entity(camera.0).insert(upscaling::UpscaleView {
            input: assets.target.clone(),
            output: assets.upscaled.clone(),
            method: s.upscaler,
            enabled: composite_path,
        });
    }
    commands
        .entity(camera.0)
        .insert(if temporal { Msaa::Off } else { s.msaa });
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
    *composite.0 = if composite_path {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for (entity, mut visibility, saved) in &mut ui_roots {
        if s.show_ui {
            if let Some(saved) = saved {
                *visibility = saved.0;
                commands.entity(entity).remove::<SavedUiVisibility>();
            }
        } else {
            if saved.is_none() {
                commands
                    .entity(entity)
                    .insert(SavedUiVisibility(*visibility));
            }
            *visibility = Visibility::Hidden;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::WindowResolution;

    #[test]
    fn third_resolution_stays_within_three_times_upscaling_on_odd_dimensions() {
        let settings = GameRenderSettings {
            resolution_scale: 1.0 / 3.0,
            ..default()
        };
        for (output, input) in [
            (UVec2::new(3456, 1942), UVec2::new(1152, 648)),
            (UVec2::new(1921, 1081), UVec2::new(641, 361)),
            (UVec2::ONE, UVec2::ONE),
        ] {
            let window = Window {
                resolution: WindowResolution::new(output.x, output.y),
                ..default()
            };
            assert_eq!(settings.target_size(&window), input);
            assert!((input * 3).cmpge(output).all());
            assert_eq!(
                GameRenderSettings {
                    render_path: RenderPath::Direct,
                    ..settings.clone()
                }
                .target_size(&window),
                output
            );
        }
    }

    #[test]
    fn profile_target_uses_explicit_pixels_across_display_scales() {
        let settings = GameRenderSettings {
            render_size: Some(UVec2::new(2560, 1440)),
            ..default()
        };
        for factor in [1.0, 2.0, 3.0] {
            let window = Window {
                resolution: WindowResolution::new(1280, 720).with_scale_factor_override(factor),
                ..default()
            };
            assert_eq!(settings.target_size(&window), UVec2::new(2560, 1440));
            assert_eq!(
                GameRenderSettings::default().target_size(&window),
                (window.physical_size().as_vec2() * 0.5).as_uvec2()
            );
        }
    }

    #[test]
    fn normal_startup_scales_the_world_and_preserves_ui_across_render_path_changes() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .add_plugins((bevy::render::sync_world::SyncWorldPlugin, GameRenderPlugin));
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
        let main = app
            .world_mut()
            .spawn((Camera3d::default(), WorldViewCamera))
            .id();
        let label = app.world_mut().spawn(Node::default()).id();
        let hidden_label = app
            .world_mut()
            .spawn((Node::default(), Visibility::Hidden))
            .id();
        app.update();
        let target = app.world().resource::<GameRenderAssets>().target.clone();
        let upscaled = app.world().resource::<GameRenderAssets>().upscaled.clone();
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&upscaled)
                .unwrap()
                .size(),
            UVec2::new(1200, 900)
        );
        let request = app.world().get::<upscaling::UpscaleView>(main).unwrap();
        assert!(request.enabled);
        assert_eq!(request.input, target);
        assert_eq!(request.output, upscaled);
        let ui = app
            .world_mut()
            .query_filtered::<Entity, With<GameUiCamera>>()
            .single(app.world())
            .unwrap();
        let composite = app
            .world_mut()
            .query_filtered::<Entity, With<GameComposite>>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            app.world().get::<ImageNode>(composite).unwrap().image,
            upscaled
        );
        assert_eq!(app.world().get::<Msaa>(main), Some(&Msaa::Sample4));
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(600, 450)
        );
        let RenderTarget::Image(image) = app.world().get::<RenderTarget>(main).unwrap() else {
            panic!("normal startup must scale the world without audit controls");
        };
        assert_eq!(image.scale_factor, 1.5);
        assert_eq!(
            app.world().get::<Visibility>(label),
            Some(&Visibility::Inherited)
        );
        // Resizing must work even when quality settings have not changed.
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set_physical_resolution(1600, 1200);
        app.update();
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&upscaled)
                .unwrap()
                .size(),
            UVec2::new(1600, 1200)
        );
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(800, 600)
        );
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set_physical_resolution(1200, 900);
        app.update();

        assert_eq!(
            app.world().get::<RenderTarget>(main).unwrap().as_image(),
            Some(&target)
        );
        assert!(app.world().get::<Camera>(ui).unwrap().is_active);

        app.world_mut()
            .resource_mut::<GameRenderSettings>()
            .upscaler = upscaling::UpscaleMethod::MetalFxTemporal;
        app.update();
        assert_eq!(app.world().get::<Msaa>(main), Some(&Msaa::Off));
        assert!(
            app.world()
                .get::<upscaling::DirectTonemapOutput>(main)
                .is_some()
        );
        assert_eq!(
            app.world()
                .get::<upscaling::temporal::TemporalView>(main)
                .unwrap()
                .input_size,
            UVec2::new(600, 450)
        );
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(1200, 900)
        );
        assert_eq!(
            app.world().get::<ImageNode>(composite).unwrap().image,
            target
        );
        assert!(
            app.world()
                .get::<bevy::core_pipeline::prepass::MotionVectorPrepass>(main)
                .is_some()
        );
        // A resize updates the scene rectangle even without a settings change.
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set_physical_resolution(1400, 1000);
        app.update();
        assert_eq!(
            app.world()
                .get::<upscaling::temporal::TemporalView>(main)
                .unwrap()
                .input_size,
            UVec2::new(700, 500)
        );
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set_physical_resolution(1200, 900);
        app.update();

        {
            let mut settings = app.world_mut().resource_mut::<GameRenderSettings>();
            settings.render_path = RenderPath::Direct;
            settings.show_ui = false;
        }
        app.update();
        assert!(matches!(
            app.world().get::<RenderTarget>(main),
            Some(RenderTarget::Window(_))
        ));
        assert!(
            app.world()
                .get::<upscaling::temporal::TemporalView>(main)
                .is_none()
        );
        assert!(
            app.world()
                .get::<upscaling::DirectTonemapOutput>(main)
                .is_none()
        );
        assert!(
            app.world()
                .get::<bevy::render::camera::TemporalJitter>(main)
                .is_none()
        );
        assert!(
            app.world()
                .get::<bevy::core_pipeline::prepass::MotionVectorPrepass>(main)
                .is_none()
        );
        assert_eq!(app.world().get::<Msaa>(main), Some(&Msaa::Sample4));
        assert_eq!(app.world().get::<Camera>(main).unwrap().order, 0);
        assert!(
            !app.world()
                .get::<upscaling::UpscaleView>(main)
                .unwrap()
                .enabled
        );
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
            let mut settings = app.world_mut().resource_mut::<GameRenderSettings>();
            settings.render_path = RenderPath::Composite;
            settings.resolution_scale = 0.5;
            settings.upscaler = upscaling::UpscaleMethod::Linear;
            settings.show_ui = true;
        }
        app.update();
        assert!(app.world().get::<Camera>(ui).unwrap().is_active);
        assert!(app.world().get::<upscaling::UpscaleView>(main).is_none());
        assert_eq!(
            app.world().get::<upscaling::UpscaleStatus>(main),
            Some(&linear_status(UVec2::new(600, 450), UVec2::new(1200, 900)))
        );
        assert_eq!(
            app.world().get::<ImageNode>(composite).unwrap().image,
            target
        );
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&upscaled)
                .unwrap()
                .size(),
            UVec2::ONE
        );
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
        // Resize the fused path without touching settings, then restore a separate backend.
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set_physical_resolution(1600, 1200);
        app.update();
        assert_eq!(
            app.world().get::<upscaling::UpscaleStatus>(main),
            Some(&linear_status(UVec2::new(800, 600), UVec2::new(1600, 1200)))
        );
        app.world_mut()
            .resource_mut::<GameRenderSettings>()
            .upscaler = upscaling::UpscaleMethod::MetalFxSpatial;
        app.update();
        assert!(
            app.world()
                .get::<upscaling::UpscaleView>(main)
                .unwrap()
                .enabled
        );
        assert_eq!(
            app.world().get::<ImageNode>(composite).unwrap().image,
            upscaled
        );
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&upscaled)
                .unwrap()
                .size(),
            UVec2::new(1600, 1200)
        );
    }

    #[test]
    fn linear_startup_samples_the_scene_without_an_upscale_pass() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .insert_resource(GameRenderSettings {
                upscaler: upscaling::UpscaleMethod::Linear,
                ..default()
            })
            .add_plugins((bevy::render::sync_world::SyncWorldPlugin, GameRenderPlugin));
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(1200, 900),
                ..default()
            },
            PrimaryWindow,
        ));
        let camera = app
            .world_mut()
            .spawn((Camera3d::default(), WorldViewCamera))
            .id();
        app.update();
        let assets = app.world().resource::<GameRenderAssets>();
        let target = assets.target.clone();
        assert!(app.world().get::<upscaling::UpscaleView>(camera).is_none());
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&assets.upscaled)
                .unwrap()
                .size(),
            UVec2::ONE
        );
        assert_eq!(
            app.world().get::<upscaling::UpscaleStatus>(camera),
            Some(&linear_status(UVec2::new(600, 450), UVec2::new(1200, 900)))
        );
        let image = app
            .world_mut()
            .query_filtered::<&ImageNode, With<GameComposite>>()
            .single(app.world())
            .unwrap();
        assert_eq!(image.image, target);
    }
}
