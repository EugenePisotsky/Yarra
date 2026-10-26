//! Game presentation defaults, shared with the optional render-audit controls.
use bevy::{
    anti_alias::fxaa::Fxaa,
    camera::{ImageRenderTarget, RenderTarget},
    ecs::system::SystemParam,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureFormat},
    window::{PrimaryWindow, WindowRef},
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
                apply_settings
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

/// Index of a launch scale in `RESOLUTION_SCALES`.
pub(crate) fn scale_index(scale: f32) -> Option<usize> {
    RESOLUTION_SCALES
        .iter()
        .position(|&s| (s - scale).abs() < 1e-3)
}

#[derive(Resource, Clone, PartialEq)]
pub(crate) struct GameRenderSettings {
    pub(crate) resolution_scale: f32,
    pub(crate) upscaler: upscaling::UpscaleMethod,
    pub(crate) temporal_debug: upscaling::temporal::TemporalDebug,
    /// Explicit internal pixel dimensions for controlled profiling, independent of Retina scaling.
    pub(crate) render_size: Option<UVec2>,
    pub(crate) show_ui: bool,
    pub(crate) direct_temporal_output: bool,
}

impl Default for GameRenderSettings {
    fn default() -> Self {
        Self {
            resolution_scale: 0.5,
            temporal_debug: default(),
            upscaler: upscaling::UpscaleMethod::MetalFxTemporal,
            render_size: None,
            show_ui: true,
            direct_temporal_output: true,
        }
    }
}

impl GameRenderSettings {
    fn linear_composition(&self) -> bool {
        self.upscaler == upscaling::UpscaleMethod::Linear
    }

    fn temporal(&self) -> bool {
        self.upscaler == upscaling::UpscaleMethod::MetalFxTemporal
    }

    /// The world camera owns the window: one pass tone-maps its image there and draws the UI
    /// over it, so the UI camera and its composite of the world image stand down.
    fn direct_output(&self) -> bool {
        self.temporal() && self.direct_temporal_output
    }

    /// The world's rendered pixels.
    pub(crate) fn target_size(&self, window: &Window) -> UVec2 {
        self.render_size.unwrap_or_else(|| {
            // Round up so odd output dimensions don't exceed the requested upscale ratio.
            (window.physical_size().as_vec2() * self.resolution_scale)
                .ceil()
                .as_uvec2()
                .max(UVec2::ONE)
        })
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

#[derive(Component)]
pub(crate) struct GameUiCamera;

#[derive(Component)]
struct GameComposite;

#[derive(Component)]
struct SavedUiVisibility(Visibility);

#[derive(Resource)]
struct GameRenderAssets {
    target: Handle<Image>,
    upscaled: Handle<Image>,
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<(Entity, &mut Camera), With<WorldViewCamera>>,
    settings: Res<GameRenderSettings>,
) {
    // Scale only the world; the composite and gameplay UI use the native window.
    let initial_size = settings.target_size(&window);
    let mut image = Image::new_target_texture(
        initial_size.x,
        initial_size.y,
        TextureFormat::Bgra8UnormSrgb,
        None,
    );
    image.sampler = ImageSampler::linear();
    let target = images.add(image);
    let linear = settings.linear_composition();
    let upscaled = images.add(upscaling::output_image(if linear {
        UVec2::ONE
    } else {
        window.physical_size()
    }));
    camera.1.order = -1;
    commands
        .entity(camera.0)
        .insert(RenderTarget::Image(ImageRenderTarget {
            handle: target.clone(),
            scale_factor: window.scale_factor() * settings.resolution_scale,
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
            enabled: true,
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
    commands.insert_resource(GameRenderAssets { target, upscaled });
}

/// What the output setup reads of the world camera, and its projection, which a new target
/// marks changed (`target_world`).
#[derive(bevy::ecs::query::QueryData)]
#[query_data(mutable)]
struct WorldCamera {
    entity: Entity,
    target: &'static RenderTarget,
    projection: &'static mut Projection,
    temporal: Option<&'static upscaling::temporal::TemporalView>,
    upscale: Option<&'static upscaling::UpscaleView>,
    status: Option<&'static upscaling::UpscaleStatus>,
}

/// What shows the world besides its camera: the UI camera, which draws the world image
/// full-screen under the UI, and the UI roots that `show_ui` hides.
#[derive(SystemParam)]
struct Presentation<'w, 's> {
    ui_camera: Single<'w, 's, (Entity, &'static mut Camera), With<GameUiCamera>>,
    composite:
        Single<'w, 's, (&'static mut ImageNode, &'static mut Visibility), With<GameComposite>>,
    ui_roots: Query<'w, 's, UiRoot, UiRoots>,
}

/// The UI's roots, not the composite under them.
type UiRoots = (With<Node>, Without<ChildOf>, Without<GameComposite>);

#[derive(bevy::ecs::query::QueryData)]
#[query_data(mutable)]
struct UiRoot {
    entity: Entity,
    visibility: &'static mut Visibility,
    saved: Option<&'static SavedUiVisibility>,
}

fn apply_settings(
    mut commands: Commands,
    s: Res<GameRenderSettings>,
    assets: Res<GameRenderAssets>,
    mut images: ResMut<Assets<Image>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut camera: Single<WorldCamera, With<WorldViewCamera>>,
    mut presentation: Presentation,
) {
    let size = s.target_size(&window);
    let linear = s.linear_composition();
    let temporal = s.temporal();
    let direct = s.direct_output();
    let output_size = window.physical_size().max(UVec2::ONE);

    // Window-sized images only where a pass writes them: the spatial upscaler's output, and the
    // world image the composite shows. The linear path's composite samples the scaled image.
    let upscaled_size = if linear || temporal {
        UVec2::ONE
    } else {
        output_size
    };
    let image_size = if direct {
        UVec2::ONE
    } else if temporal {
        output_size
    } else {
        size
    };
    resize(&mut images, &assets.upscaled, upscaled_size);
    resize(&mut images, &assets.target, image_size);
    target_world(&mut commands, &s, &assets, &window, &mut camera);

    let presented = if temporal || linear {
        &assets.target
    } else {
        &assets.upscaled
    };

    if presentation.composite.0.image != *presented {
        presentation.composite.0.image = presented.clone();
    }

    if temporal {
        let request = upscaling::temporal::TemporalView {
            input_size: size,
            reset_epoch: 0,
            debug: s.temporal_debug,
        };
        if camera.temporal != Some(&request) {
            commands
                .entity(camera.entity)
                .insert((request, bevy::camera::MainPassResolutionOverride(size)));
            // Retain the original output path for matched performance comparisons.
            if s.direct_temporal_output {
                commands
                    .entity(camera.entity)
                    .insert(upscaling::DirectTonemapOutput);
            }
        }
    } else if camera.temporal.is_some() {
        commands.entity(camera.entity).remove::<(
            upscaling::temporal::TemporalView,
            upscaling::DirectTonemapOutput,
            bevy::camera::MainPassResolutionOverride,
            bevy::render::camera::TemporalJitter,
            bevy::render::camera::MipBias,
            bevy::core_pipeline::prepass::MotionVectorPrepass,
        )>();
    }

    if linear {
        if camera.upscale.is_some() {
            commands
                .entity(camera.entity)
                .remove::<upscaling::UpscaleView>();
        }

        let status = linear_status(size, window.physical_size());

        if camera.status != Some(&status) {
            commands.entity(camera.entity).insert(status);
        }
    }

    if !s.is_changed() {
        return;
    }

    if !linear {
        commands
            .entity(camera.entity)
            .insert(upscaling::UpscaleView {
                input: assets.target.clone(),
                output: assets.upscaled.clone(),
                method: s.upscaler,
                enabled: true,
            });
    }

    // MetalFX Temporal resolves edges itself; the other paths take FXAA. World cameras do not
    // multisample (docs/DECISIONS.md).
    if temporal {
        commands.entity(camera.entity).remove::<Fxaa>();
    } else {
        commands.entity(camera.entity).insert(Fxaa::default());
    }

    presentation.assign_ui(&mut commands, direct, camera.entity);
    presentation.show_ui(&mut commands, s.show_ui);
}

fn resize(images: &mut Assets<Image>, image: &Handle<Image>, size: UVec2) {
    if images.get(image).expect("game render image").size() != size {
        images.get_mut(image).unwrap().resize(Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        });
    }
}

/// The world camera draws onto the window on the direct output, else into the world image.
fn target_world(
    commands: &mut Commands,
    s: &GameRenderSettings,
    assets: &GameRenderAssets,
    window: &Window,
    camera: &mut WorldCameraItem,
) {
    // Bevy 0.20's camera_system sizes a camera for its target on window and image events or a
    // changed projection, not on a new target: back on the window from the world image, the
    // camera kept the image's size and scale, and the UI drew at half resolution.
    if s.direct_output() {
        if !matches!(camera.target, RenderTarget::Window(WindowRef::Primary)) {
            commands
                .entity(camera.entity)
                .insert(RenderTarget::Window(WindowRef::Primary));
            camera.projection.set_changed();
        }

        return;
    }

    let scale = if s.temporal() {
        window.scale_factor()
    } else if s.render_size.is_some() {
        s.target_size(window).y as f32 / window.height().max(1.0)
    } else {
        window.scale_factor() * s.resolution_scale
    };
    let current = matches!(camera.target, RenderTarget::Image(target)
        if target.handle == assets.target && target.scale_factor == scale);

    if !current {
        // Preserve logical viewport dimensions (and tree LOD) while varying physical pixels.
        commands
            .entity(camera.entity)
            .insert(RenderTarget::Image(ImageRenderTarget {
                handle: assets.target.clone(),
                scale_factor: scale,
            }));
        camera.projection.set_changed();
    }
}

impl Presentation<'_, '_> {
    /// The camera that draws the UI: the world camera on the direct output, else the UI camera
    /// over its composite of the world image.
    fn assign_ui(&mut self, commands: &mut Commands, direct: bool, world_camera: Entity) {
        let (ui_camera, camera) = &mut *self.ui_camera;
        camera.is_active = !direct;
        *self.composite.1 = if direct {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let (drawing, idle) = if direct {
            (world_camera, *ui_camera)
        } else {
            (*ui_camera, world_camera)
        };
        commands.entity(drawing).insert(IsDefaultUiCamera);
        commands.entity(idle).remove::<IsDefaultUiCamera>();
    }

    fn show_ui(&mut self, commands: &mut Commands, show: bool) {
        for mut root in &mut self.ui_roots {
            if show {
                if let Some(saved) = root.saved {
                    *root.visibility = saved.0;
                    commands.entity(root.entity).remove::<SavedUiVisibility>();
                }
            } else {
                if root.saved.is_none() {
                    commands
                        .entity(root.entity)
                        .insert(SavedUiVisibility(*root.visibility));
                }

                *root.visibility = Visibility::Hidden;
            }
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
    fn the_world_camera_takes_each_new_targets_size() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<bevy::render::texture::ManualTextureViews>()
            .add_message::<bevy::window::WindowResized>()
            .add_message::<bevy::window::WindowCreated>()
            .add_message::<bevy::window::WindowScaleFactorChanged>()
            .add_message::<AssetEvent<Image>>()
            .insert_resource(GameRenderSettings {
                upscaler: upscaling::UpscaleMethod::MetalFxTemporal,
                ..default()
            })
            .add_plugins((
                bevy::extract::sync_world::SyncWorldPlugin::<bevy::render::RenderApp>::default(),
                GameRenderPlugin,
            ))
            .add_systems(PostUpdate, bevy::render::camera::camera_system);
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(1200, 900).with_scale_factor_override(2.0),
                ..default()
            },
            PrimaryWindow,
        ));
        let main = app
            .world_mut()
            .spawn((Camera3d::default(), WorldViewCamera))
            .id();
        let sized = |app: &App| {
            let camera = app.world().get::<Camera>(main).unwrap();
            (
                camera.physical_target_size(),
                camera.target_scaling_factor(),
            )
        };

        // On the window, then in the half-sized world image, then on the window again: the UI
        // the camera draws on the window lays out by its size and scale.
        app.update();
        assert_eq!(sized(&app), (Some(UVec2::new(1200, 900)), Some(2.0)));

        for (upscaler, size, scale) in [
            (
                upscaling::UpscaleMethod::MetalFxSpatial,
                UVec2::new(600, 450),
                1.0,
            ),
            (
                upscaling::UpscaleMethod::MetalFxTemporal,
                UVec2::new(1200, 900),
                2.0,
            ),
        ] {
            app.world_mut()
                .resource_mut::<GameRenderSettings>()
                .upscaler = upscaler;
            app.update();
            assert_eq!(sized(&app), (Some(size), Some(scale)), "{upscaler:?}");
        }
    }

    #[test]
    fn spatial_startup_scales_the_world_and_preserves_ui_across_settings_changes() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .insert_resource(GameRenderSettings {
                upscaler: upscaling::UpscaleMethod::MetalFxSpatial,
                ..default()
            })
            .add_plugins((
                bevy::extract::sync_world::SyncWorldPlugin::<bevy::render::RenderApp>::default(),
                GameRenderPlugin,
            ));
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
        assert!(app.world().get::<Fxaa>(main).is_some());
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
        assert!(app.world().get::<Fxaa>(main).is_none());
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
        assert!(
            app.world()
                .get::<bevy::core_pipeline::prepass::MotionVectorPrepass>(main)
                .is_some()
        );

        // The world camera owns the window and draws the UI; the UI camera and composite rest.
        assert!(matches!(
            app.world().get::<RenderTarget>(main).unwrap(),
            RenderTarget::Window(WindowRef::Primary)
        ));
        assert!(!app.world().get::<Camera>(ui).unwrap().is_active);
        assert!(app.world().get::<IsDefaultUiCamera>(main).is_some());
        assert!(app.world().get::<IsDefaultUiCamera>(ui).is_none());
        assert_eq!(
            app.world().get::<Visibility>(composite),
            Some(&Visibility::Hidden)
        );

        for image in [&target, &upscaled] {
            assert_eq!(
                app.world()
                    .resource::<Assets<Image>>()
                    .get(image)
                    .unwrap()
                    .size(),
                UVec2::ONE
            );
        }

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
            settings.upscaler = upscaling::UpscaleMethod::MetalFxSpatial;
            settings.show_ui = false;
        }

        app.update();
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
        assert!(app.world().get::<Fxaa>(main).is_some());
        assert_eq!(
            app.world().get::<ImageNode>(composite).unwrap().image,
            upscaled
        );
        assert_eq!(
            app.world().get::<Visibility>(label),
            Some(&Visibility::Hidden)
        );
        assert_eq!(
            app.world().get::<RenderTarget>(main).unwrap().as_image(),
            Some(&target)
        );
        assert!(app.world().get::<Camera>(ui).unwrap().is_active);
        assert!(app.world().get::<IsDefaultUiCamera>(ui).is_some());
        assert!(app.world().get::<IsDefaultUiCamera>(main).is_none());
        assert_eq!(
            app.world().get::<Visibility>(composite),
            Some(&Visibility::Inherited)
        );

        {
            let mut settings = app.world_mut().resource_mut::<GameRenderSettings>();
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
    fn standard_temporal_output_composites_the_world_image_on_the_ui_camera() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .insert_resource(GameRenderSettings {
                direct_temporal_output: false,
                ..default()
            })
            .add_plugins((
                bevy::extract::sync_world::SyncWorldPlugin::<bevy::render::RenderApp>::default(),
                GameRenderPlugin,
            ));
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(1200, 900),
                ..default()
            },
            PrimaryWindow,
        ));
        let main = app
            .world_mut()
            .spawn((Camera3d::default(), WorldViewCamera))
            .id();
        app.update();
        let target = app.world().resource::<GameRenderAssets>().target.clone();
        let ui = app
            .world_mut()
            .query_filtered::<Entity, With<GameUiCamera>>()
            .single(app.world())
            .unwrap();
        let composite = app
            .world_mut()
            .query_filtered::<&ImageNode, With<GameComposite>>()
            .single(app.world())
            .unwrap();
        assert_eq!(composite.image, target);
        assert!(
            app.world()
                .get::<upscaling::DirectTonemapOutput>(main)
                .is_none()
        );
        assert_eq!(
            app.world().get::<RenderTarget>(main).unwrap().as_image(),
            Some(&target)
        );
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(1200, 900)
        );
        assert!(app.world().get::<Camera>(ui).unwrap().is_active);
        assert!(app.world().get::<IsDefaultUiCamera>(ui).is_some());
    }

    #[test]
    fn linear_startup_samples_the_scene_without_an_upscale_pass() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .insert_resource(GameRenderSettings {
                upscaler: upscaling::UpscaleMethod::Linear,
                ..default()
            })
            .add_plugins((
                bevy::extract::sync_world::SyncWorldPlugin::<bevy::render::RenderApp>::default(),
                GameRenderPlugin,
            ));
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
