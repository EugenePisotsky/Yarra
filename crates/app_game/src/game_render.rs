//! Game presentation defaults, shared with the optional render-audit controls.
use bevy::{
    camera::{ImageRenderTarget, RenderTarget},
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureFormat},
    window::PrimaryWindow,
};
use engine::{GameInputSystems, WorldViewCamera};

pub(crate) struct GameRenderPlugin;

impl Plugin for GameRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameRenderSettings>()
            .add_systems(PostStartup, setup.in_set(GameRenderSetup))
            .add_systems(
                Update,
                apply_render_path
                    .in_set(GameRenderSystems)
                    .before(GameInputSystems),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GameRenderSetup;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct GameRenderSystems;

#[derive(Resource, Clone, PartialEq)]
pub(crate) struct GameRenderSettings {
    pub(crate) resolution_scale: f32,
    /// Explicit internal pixel dimensions for controlled profiling, independent of Retina scaling.
    pub(crate) render_size: Option<UVec2>,
    pub(crate) msaa: Msaa,
    pub(crate) render_path: RenderPath,
    pub(crate) show_ui: bool,
}

impl Default for GameRenderSettings {
    fn default() -> Self {
        Self {
            resolution_scale: 0.75,
            render_size: None,
            msaa: Msaa::Sample4,
            render_path: RenderPath::Composite,
            show_ui: true,
        }
    }
}

impl GameRenderSettings {
    fn target_size(&self, window: &Window) -> UVec2 {
        self.render_size.unwrap_or_else(|| {
            (window.physical_size().as_vec2() * self.scale())
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
    let original_target = camera.2.clone();
    let original_camera_order = camera.1.order;
    camera.1.order = -1;
    commands
        .entity(camera.0)
        .insert(RenderTarget::Image(ImageRenderTarget {
            handle: target.clone(),
            scale_factor: window.scale_factor() * settings.scale(),
        }));
    commands.spawn((
        Camera2d,
        Msaa::Off,
        IsDefaultUiCamera,
        GameUiCamera,
        Name::new("Game UI camera"),
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
        GameComposite,
    ));
    commands.insert_resource(GameRenderAssets {
        target,
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
    mut camera: Single<(Entity, &mut Camera, &RenderTarget), With<WorldViewCamera>>,
    mut ui_camera: Single<(Entity, &mut Camera), (With<GameUiCamera>, Without<WorldViewCamera>)>,
    mut composite: Single<&mut Visibility, With<GameComposite>>,
    mut ui_roots: Query<
        (Entity, &mut Visibility, Option<&SavedUiVisibility>),
        (With<Node>, Without<ChildOf>, Without<GameComposite>),
    >,
) {
    let scale = s.scale();
    let size = s.target_size(&window);
    let target_scale = if s.render_size.is_some() {
        size.y as f32 / window.height().max(1.0)
    } else {
        window.scale_factor() * scale
    };
    let composite_path = s.render_path == RenderPath::Composite;
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
            .expect("game render target")
            .size()
            != size
    {
        let mut image = images.get_mut(&assets.target).expect("game render target");
        image.resize(Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        });
    }
    if !s.is_changed() {
        return;
    }
    commands.entity(camera.0).insert(s.msaa);
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
                (window.physical_size().as_vec2() * 0.75).as_uvec2()
            );
        }
    }

    #[test]
    fn normal_startup_scales_the_world_and_preserves_ui_across_render_path_changes() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .add_plugins(GameRenderPlugin);
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
        assert_eq!(app.world().get::<Msaa>(main), Some(&Msaa::Sample4));
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(900, 675)
        );
        let RenderTarget::Image(image) = app.world().get::<RenderTarget>(main).unwrap() else {
            panic!("normal startup must scale the world without audit controls");
        };
        assert_eq!(image.scale_factor, 2.25);
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
                .get(&target)
                .unwrap()
                .size(),
            UVec2::new(1200, 900)
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
            let mut settings = app.world_mut().resource_mut::<GameRenderSettings>();
            settings.render_path = RenderPath::Composite;
            settings.resolution_scale = 0.5;
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
}
