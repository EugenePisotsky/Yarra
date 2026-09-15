//! A single, bounded production grass view with an independent editor camera and transport.
mod comparison;
mod ground;
mod ground_treatment;
mod references;
mod stage;
mod study;
mod ui;

use super::{EditorFramePacing, EditorWorkspace, FramePacingOwner};
use crate::{shell::EditorUiSet, vegetation_authoring::VegetationAuthoringState};
use bevy::{
    camera::{Exposure, RenderTarget, visibility::RenderLayers},
    prelude::*,
    render::{
        render_resource::TextureFormat,
        view::{
            Msaa,
            screenshot::{Screenshot, ScreenshotCaptured},
        },
    },
};
use bevy_egui::{EguiPrimaryContextPass, EguiTextureHandle, EguiUserTextures, egui};
use comparison::ComparisonView;
use engine::{CharacterPresentationPreview, DEFAULT_CHARACTER_PRESENTATION_ID, WorldSun};
use references::References;
use stage::{CharacterPlacement, GrassEdge, GroundMode, ReferenceSetup};
use std::{path::PathBuf, time::Instant};
use study::*;
use ui::ui;
use vegetation_render::{
    VegetationBladeBands, VegetationDebugScene, VegetationDebugSettings, VegetationDiagnostics,
    VegetationLighting, VegetationProfileMode, VegetationShapeInspection, VegetationWind,
};

const LAYER: usize = 31;
const GROUND_COLOR: [f32; 3] = [0.105, 0.12, 0.065];

#[derive(Component)]
pub(crate) struct VegetationWorkspaceCamera;
#[derive(Component)]
struct StudyGround;
#[derive(Component)]
struct StudyCharacter;
#[derive(Component)]
struct StudyScaleRuler;

pub(crate) struct VegetationWorkspacePlugin {
    pub runtime_database: PathBuf,
}
impl Plugin for VegetationWorkspacePlugin {
    fn build(&self, app: &mut App) {
        ground_treatment::register(app);
        let launch = StudyLaunch::parse(std::env::args().skip(1)).unwrap_or_else(|e| {
            eprintln!("Vegetation study: {e}");
            std::process::exit(2);
        });
        app.insert_resource(StudyState::new(launch))
            .insert_resource(ground::GroundDatabase(self.runtime_database.clone()))
            .init_resource::<ground::StudyGroundAssets>()
            .add_systems(Startup, setup)
            .add_systems(Startup, ground::setup.after(setup))
            .add_systems(
                PreUpdate,
                references::configure_texture_limit
                    .after(bevy_egui::EguiPreUpdateSet::ProcessInput)
                    .before(bevy_egui::EguiPreUpdateSet::BeginPass),
            )
            .add_systems(
                Update,
                ground::sync.run_if(in_state(EditorWorkspace::Vegetation)),
            )
            .add_systems(OnEnter(EditorWorkspace::Vegetation), enter)
            .add_systems(OnExit(EditorWorkspace::Vegetation), leave)
            .add_systems(Update, sync.run_if(in_state(EditorWorkspace::Vegetation)))
            .add_systems(
                PostUpdate,
                sync_scale_figure
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
            )
            .add_systems(
                PostUpdate,
                capture
                    .after(sync_scale_figure)
                    .before(bevy_egui::EguiPostUpdateSet::EndPass)
                    .run_if(in_state(EditorWorkspace::Vegetation)),
            )
            .add_systems(
                EguiPrimaryContextPass,
                ui.run_if(in_state(EditorWorkspace::Vegetation))
                    .in_set(EditorUiSet::Workspace),
            );
    }
}

struct RestoredWorld {
    scene: VegetationDebugScene,
    settings: VegetationDebugSettings,
    lighting: VegetationLighting,
    wind: VegetationWind,
    sun: (Entity, Transform, DirectionalLight, Option<RenderLayers>),
    ambient: GlobalAmbientLight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SceneSignature {
    revision: u64,
    population: usize,
    seed: u32,
    field_size: u32,
    edge: Option<[u32; 3]>,
}

#[derive(Resource)]
struct StudyState {
    launch: StudyLaunch,
    refs: References,
    camera: StudyCamera,
    camera_label: String,
    comparison: ComparisonView,
    render_size: [u32; 2],
    msaa_samples: u32,
    seed: u32,
    wind: WindStudy,
    playing: bool,
    playback_speed: f32,
    restore: Option<RestoredWorld>,
    own_settings: Option<(VegetationDebugSettings, VegetationLighting)>,
    own_environment: Option<(Transform, DirectionalLight, GlobalAmbientLight)>,
    image: Handle<Image>,
    texture: egui::TextureId,
    signature: Option<SceneSignature>,
    candidates: u32,
    ready_frames: u32,
    error: Option<String>,
    message: String,
    capture_dir: Option<PathBuf>,
    pending: u32,
    capture_finished: bool,
    capture_failed: bool,
    started: Instant,
    show_reference: bool,
    show_inspector: bool,
    show_colors: bool,
    show_canopy: bool,
    canopy_message: Option<String>,
    show_picker: bool,
    save_study: bool,
    show_character: bool,
    show_ruler: bool,
    character: CharacterPlacement,
    field_size: f32,
    ground: GroundMode,
    edge: Option<GrassEdge>,
    character_ready: bool,
}

impl StudyState {
    fn new(launch: StudyLaunch) -> Self {
        let refs = References::default();
        let camera = launch
            .camera
            .clone()
            .or_else(|| launch.load.as_ref().map(|d| d.camera.clone()))
            .unwrap_or_default();
        let mut wind = launch
            .load
            .as_ref()
            .map(|d| d.wind.clone())
            .unwrap_or_else(|| VegetationWind::default().into());
        if let Some(time) = launch.time {
            wind.time = time;
        }
        let seed = launch.load.as_ref().map_or(0x5a37_c19d, |d| d.seed);
        let capture_dir = launch.capture.clone();
        let show_character =
            launch.character || launch.load.as_ref().is_none_or(|d| d.show_character);
        let show_ruler = launch.ruler
            || launch.load.as_ref().is_some_and(|d| {
                if d.version == 1 {
                    d.show_character
                } else {
                    d.show_ruler
                }
            });
        let character = launch
            .load
            .as_ref()
            .map(|d| d.character.clone())
            .unwrap_or_default();
        let field_size = launch.load.as_ref().map_or(16.0, |d| d.patch_size);
        let ground = launch
            .load
            .as_ref()
            .map_or(GroundMode::Meadow, |d| d.ground);
        let edge = launch.load.as_ref().and_then(|d| d.edge);
        let comparison = launch
            .load
            .as_ref()
            .map(|d| d.comparison.clone())
            .unwrap_or_default();
        let render_size = launch
            .load
            .as_ref()
            .map_or([WIDTH, HEIGHT], |d| d.render_size);
        let msaa_samples = launch.load.as_ref().map_or(1, |d| d.msaa_samples);
        let show_inspector = launch.show_inspector;
        let show_colors = launch.show_colors;
        let show_canopy = launch.show_canopy;
        let show_picker = launch.show_picker;
        let playing = launch.play;
        let camera_label = if launch.load.is_some() {
            "Study replay"
        } else {
            "Approximate match"
        }
        .into();
        Self {
            launch,
            refs,
            camera,
            camera_label,
            comparison,
            render_size,
            msaa_samples,
            seed,
            wind,
            playing,
            playback_speed: 1.0,
            restore: None,
            own_settings: None,
            own_environment: None,
            image: default(),
            texture: egui::TextureId::default(),
            signature: None,
            candidates: 0,
            ready_frames: 0,
            error: None,
            message: String::new(),
            capture_dir,
            pending: 0,
            capture_finished: false,
            capture_failed: false,
            started: Instant::now(),
            show_reference: !show_canopy,
            show_inspector,
            show_colors,
            show_canopy,
            canopy_message: None,
            show_picker,
            save_study: false,
            show_character,
            show_ruler,
            character,
            field_size,
            ground,
            edge,
            character_ready: false,
        }
    }

    fn match_reference(&mut self, index: usize) {
        if index >= self.refs.items.len() {
            return;
        }
        self.refs.selected = index;
        let setup = self.refs.matching_setup();
        self.camera = setup.camera;
        self.character = setup.character;
        self.field_size = setup.field_size;
        self.ground = setup.ground;
        self.edge = setup.edge;
        self.show_character = true;
        self.camera_label = format!(
            "{} · {}",
            self.refs.title(),
            if self.refs.has_saved_camera() {
                "saved setup"
            } else {
                "approximate scale match"
            }
        );
        self.comparison = ComparisonView::default();
        self.ready_frames = 0;
    }

    fn reference_setup(&self) -> ReferenceSetup {
        ReferenceSetup {
            camera: self.camera.clone(),
            character: self.character.clone(),
            field_size: self.field_size,
            ground: self.ground,
            edge: self.edge,
        }
    }

    fn scene_signature(&self, revision: u64, population: usize) -> SceneSignature {
        SceneSignature {
            revision,
            population,
            seed: self.seed,
            field_size: self.field_size.to_bits(),
            edge: self.edge.map(GrassEdge::signature),
        }
    }
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut textures: ResMut<EguiUserTextures>,
    mut state: ResMut<StudyState>,
    mut next: ResMut<NextState<EditorWorkspace>>,
) {
    state.refs = References::load();
    for (key, reference) in [
        ("low", "edge_1-"),
        ("top", "top_down-"),
        ("scale", "bottom_straight-"),
    ] {
        if state.camera == StudyCamera::preset(key).unwrap() {
            state.refs.select_named(reference);
        }
    }
    for path in state.launch.references.clone() {
        if let Err(e) = state.refs.import(&path) {
            state.refs.error = Some(e);
        }
    }
    if let Some(name) = state.launch.select_reference.clone() {
        match state.refs.find(&name) {
            Ok(index) => state.match_reference(index),
            Err(error) => {
                state.error = Some(error.clone());
                state.message = error;
                state.capture_failed = true;
                state.capture_finished = true;
            }
        }
    } else if state.launch.camera.is_none() && state.launch.load.is_none() {
        let index = state.refs.selected;
        state.match_reference(index);
    }
    if let Some(zoom) = state.launch.zoom {
        state.comparison.zoom_at(zoom, egui::Vec2::splat(0.5));
    }
    if let Some(size) = state.launch.field_size {
        state.field_size = size;
    }
    if let Some(ground) = state.launch.ground {
        state.ground = ground;
    }
    if state.launch.camera == StudyCamera::preset("game-close").ok() {
        state.character = CharacterPlacement {
            xz: [0.0, 0.0],
            yaw: 0.0,
        };
        state.show_character = true;
    }
    if state.launch.hide_character {
        state.show_character = false;
    }
    if let Some(enabled) = state.launch.edge {
        state.edge = enabled.then(|| state.edge.unwrap_or_default());
    }
    let image = images.add(Image::new_target_texture(
        state.render_size[0],
        state.render_size[1],
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    state.texture = textures.add_image(EguiTextureHandle::Strong(image.clone()));
    state.image = image.clone();
    commands.spawn((
        Camera3d::default(),
        Camera {
            is_active: false,
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.065, 0.08, 0.095)),
            ..default()
        },
        RenderTarget::Image(image.into()),
        Projection::Perspective(PerspectiveProjection {
            fov: state.camera.fov.to_radians(),
            near: 0.02,
            far: 512.0,
            ..default()
        }),
        if state.msaa_samples == 4 {
            Msaa::Sample4
        } else {
            Msaa::Off
        },
        Exposure { ev100: 13.0 },
        state.camera.transform(),
        RenderLayers::layer(LAYER),
        VegetationWorkspaceCamera,
        Name::new("Vegetation study viewport"),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(8.0, 8.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb_from_array(GROUND_COLOR),
            perceptual_roughness: 1.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.005, 0.0),
        RenderLayers::layer(LAYER),
        StudyGround,
        Name::new("Vegetation study neutral ground"),
    ));
    commands.spawn((
        CharacterPresentationPreview::new(DEFAULT_CHARACTER_PRESENTATION_ID),
        state.character.transform(),
        Visibility::Hidden,
        RenderLayers::layer(LAYER),
        StudyCharacter,
        Name::new("Vegetation scale character"),
    ));
    let ruler_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.82, 0.62, 0.22),
        unlit: true,
        ..default()
    });
    commands
        .spawn((
            Transform::from_xyz(1.45, 0.0, -0.2),
            Visibility::Hidden,
            StudyScaleRuler,
        ))
        .with_children(|parent| {
            parent.spawn((
                Mesh3d(meshes.add(Cuboid::new(0.012, 2.0, 0.012))),
                MeshMaterial3d(ruler_material.clone()),
                Transform::from_xyz(0.0, 1.0, 0.0),
                RenderLayers::layer(LAYER),
            ));
            for tick in 0..=8 {
                parent.spawn((
                    Mesh3d(meshes.add(Cuboid::new(
                        if tick % 4 == 0 { 0.20 } else { 0.10 },
                        0.012,
                        0.012,
                    ))),
                    MeshMaterial3d(ruler_material.clone()),
                    Transform::from_xyz(0.0, tick as f32 * 0.25, 0.0),
                    RenderLayers::layer(LAYER),
                ));
            }
        });
    if state.launch.open {
        next.set(EditorWorkspace::Vegetation);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{AnimationWorkspaceCamera, EditorWorkspacesPlugin};
    use super::*;

    #[test]
    fn repeated_workspace_switches_restore_world_resources_and_camera_ownership() {
        let mut app = App::new();
        let original_scene = VegetationDebugScene::reference();
        let mut original_wind = VegetationWind::default();
        original_wind.set_phase_seconds(123.0);
        let sun_transform = Transform::from_xyz(12.0, 20.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y);
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
            .spawn((WorldSun, sun_transform, DirectionalLight::default()))
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
            assert_eq!(app.world().resource::<VegetationLighting>().canopy, world_canopy);
            let study_canopy = vegetation::CanopyShading { height_metres: 0.23, ..world_canopy };
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
                .resource_mut::<VegetationDebugScene>()
                .replace(patch)
                .unwrap();
            app.world_mut()
                .resource_mut::<NextState<EditorWorkspace>>()
                .set(EditorWorkspace::World);
            app.update();
            assert_eq!(app.world().resource::<VegetationLighting>().canopy, study_canopy);
            assert!(app.world().get::<Camera>(world_camera).unwrap().is_active);
            assert!(!app.world().get::<Camera>(study_camera).unwrap().is_active);
            assert_eq!(
                app.world().resource::<VegetationDebugScene>().scene(),
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
        }
    }

    #[test]
    fn reproduction_round_trip_validates_and_retains_exact_phase_and_catalog() {
        let mut state = StudyState::new(StudyLaunch::default());
        state.signature = Some(state.scene_signature(4, 3));
        state.wind.time = 17.625;
        state.edge = Some(GrassEdge::default());
        state.comparison = ComparisonView {
            zoom: 2.0,
            center: [0.6, 0.4],
        };
        let scene = VegetationDebugScene::new(
            bounded_scene(&vegetation::fixtures::reference_catalog(), 3, state.seed)
                .unwrap()
                .0,
        )
        .unwrap();
        let doc = document(
            &state,
            &scene.scene().catalog,
            default(),
            default(),
            (&Transform::default(), &DirectionalLight::default()),
            &GlobalAmbientLight::default(),
        )
        .unwrap();
        let encoded = ron::ser::to_string(&doc).unwrap();
        let decoded: StudyDocument = ron::from_str(&encoded).unwrap();
        decoded.validate().unwrap();
        assert_eq!(decoded.wind.time, 17.625);
        assert_eq!(decoded.catalog, doc.catalog);
        assert_eq!(decoded.camera, doc.camera);
        assert_eq!(decoded.comparison, state.comparison);
        assert_eq!(decoded.character, state.character);
        assert_eq!(decoded.patch_size, state.field_size);
        assert_eq!(decoded.ground, GroundMode::Meadow);
        assert_eq!(decoded.edge, state.edge);
        let mut legacy = doc.clone();
        legacy.version = 1;
        legacy.render_size = [1280, 960];
        legacy.patch_size = 4.0;
        let field = format!(
            "comparison:{},",
            ron::ser::to_string(&legacy.comparison).unwrap()
        );
        let encoded = ron::ser::to_string(&legacy).unwrap();
        assert!(encoded.contains(&field));
        let mut encoded = encoded.replacen(&field, "", 1);
        for field in [
            format!(
                "character:{},",
                ron::ser::to_string(&legacy.character).unwrap()
            ),
            format!("ground:{},", ron::ser::to_string(&legacy.ground).unwrap()),
            format!("show_ruler:{},", legacy.show_ruler),
            format!("edge:{},", ron::ser::to_string(&legacy.edge).unwrap()),
        ] {
            assert!(encoded.contains(&field));
            encoded = encoded.replacen(&field, "", 1);
        }
        let old: StudyDocument = ron::from_str(&encoded).unwrap();
        old.validate().unwrap();
        assert_eq!(old.comparison, ComparisonView::default());
        assert_eq!(old.ground, GroundMode::Neutral);
        assert_eq!(old.character, CharacterPlacement::default());
        assert_eq!(old.edge, None);
        assert!(old.show_ruler);
        assert_eq!(
            StudyState::new(StudyLaunch {
                load: Some(old),
                ..default()
            })
            .render_size,
            [1280, 960]
        );
        let mut invalid = decoded;
        invalid.camera.pitch = f32::NAN;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn selecting_reference_restores_camera_and_framing_without_changing_wind() {
        let mut state = StudyState::new(StudyLaunch::default());
        state.refs.items = vec![(PathBuf::from("edge_2-0123456789abcdef.png"), None)];
        state.comparison = ComparisonView {
            zoom: 4.0,
            center: [0.3, 0.7],
        };
        state.wind.time = 13.25;
        state.match_reference(0);
        assert_eq!(state.camera, state.refs.matching_camera());
        assert_eq!(state.character.xz, [-0.55, 0.15]);
        assert!(state.show_character);
        assert_eq!(state.field_size, 16.0);
        assert!(state.edge.is_some());
        let signature = state.scene_signature(4, 3);
        state.edge.as_mut().unwrap().offset += 0.1;
        assert_ne!(signature, state.scene_signature(4, 3));
        assert_eq!(state.comparison, ComparisonView::default());
        assert_eq!(state.wind.time, 13.25);
    }
}

fn sync_scale_figure(
    mut commands: Commands,
    workspace: Res<State<EditorWorkspace>>,
    mut state: ResMut<StudyState>,
    mut roots: Query<
        (Entity, &mut Visibility, &mut Transform, Has<StudyCharacter>),
        Or<(With<StudyCharacter>, With<StudyScaleRuler>)>,
    >,
    children: Query<&Children>,
    meshes: Query<Option<&RenderLayers>, With<Mesh3d>>,
) {
    let active = *workspace.get() == EditorWorkspace::Vegetation;
    let mut ready = false;
    for (entity, mut visibility, mut transform, character) in &mut roots {
        let show = active
            && if character {
                state.show_character
            } else {
                state.show_ruler
            };
        let desired_transform = if character {
            state.character.transform()
        } else {
            Transform::from_xyz(state.character.xz[0] + 0.6, 0.0, state.character.xz[1])
        };
        if *transform != desired_transform {
            *transform = desired_transform;
            state.ready_frames = 0;
        }
        let desired = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != desired {
            *visibility = desired;
            state.ready_frames = 0;
        }
        if character {
            // Imported scene children do not inherit render layers.
            for child in children.iter_descendants(entity) {
                if let Ok(layers) = meshes.get(child) {
                    ready = true;
                    if layers.is_none_or(|l| *l != RenderLayers::layer(LAYER)) {
                        commands.entity(child).insert(RenderLayers::layer(LAYER));
                    }
                }
            }
        }
    }
    if state.character_ready != ready {
        state.character_ready = ready;
        state.ready_frames = 0;
    }
}

#[allow(clippy::too_many_arguments)]
fn enter(
    mut commands: Commands,
    mut state: ResMut<StudyState>,
    scene: Res<VegetationDebugScene>,
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
fn leave(
    mut commands: Commands,
    mut state: ResMut<StudyState>,
    mut scene: ResMut<VegetationDebugScene>,
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

#[allow(clippy::too_many_arguments)]
fn sync(
    time: Res<Time>,
    mut state: ResMut<StudyState>,
    mut authoring: ResMut<VegetationAuthoringState>,
    mut scene: ResMut<VegetationDebugScene>,
    mut wind: ResMut<VegetationWind>,
    mut settings: ResMut<VegetationDebugSettings>,
    mut cameras: Query<(&mut Transform, &mut Projection), With<VegetationWorkspaceCamera>>,
) {
    if authoring.study_source().is_none() {
        return;
    }
    if let Some(doc) = state.launch.load.take() {
        authoring.import_study_catalog(doc.catalog, &doc.population);
        if let Some(reference) = doc.reference.filter(|_| {
            state.launch.select_reference.is_none() && state.launch.references.is_empty()
        }) {
            if let Some(index) = state
                .refs
                .items
                .iter()
                .position(|(p, _)| p.file_name().is_some_and(|v| v == reference.as_str()))
            {
                state.refs.selected = index;
            }
        }
    }
    let (catalog, selected, revision) = authoring.study_source().unwrap();
    let band_density = catalog
        .populations
        .get(selected)
        .map_or(0.0, |p| (p.density_per_square_meter / 44.0).clamp(0.0, 1.0));
    if settings.blade_band_density != band_density {
        settings.blade_band_density = band_density;
    }
    let signature = state.scene_signature(revision, selected);
    if state.signature != Some(signature) {
        match bounded_field_with_edge(catalog, selected, state.seed, state.field_size, state.edge) {
            Ok((patch, count)) => {
                scene.replace(patch).expect("validated bounded study scene");
                state.candidates = count;
                state.error = None;
            }
            Err(error) => {
                scene
                    .replace(vegetation::VegetationScene {
                        catalog: catalog.clone(),
                        pages: vec![],
                    })
                    .expect("validated authoring draft");
                state.candidates = 0;
                state.error = Some(error);
            }
        }
        state.signature = Some(signature);
        state.ready_frames = 0;
    }
    if state.playing && state.pending == 0 && state.capture_dir.is_none() {
        state.wind.time = (state.wind.time + time.delta_secs().min(0.1) * state.playback_speed)
            .rem_euclid(4096.0);
    }
    state.wind.apply(&mut wind);
    for (mut transform, mut projection) in &mut cameras {
        let desired = state.camera.transform();
        if *transform != desired {
            *transform = desired;
            state.ready_frames = 0;
        }
        if let Projection::Perspective(p) = &mut *projection {
            let fov = state.camera.fov.to_radians();
            if p.fov != fov {
                p.fov = fov;
                state.ready_frames = 0;
            }
        }
    }
    state.ready_frames += 1;
}

fn document(
    state: &StudyState,
    catalog: &vegetation::VegetationCatalog,
    settings: VegetationDebugSettings,
    lighting: VegetationLighting,
    sun: (&Transform, &DirectionalLight),
    ambient: &GlobalAmbientLight,
) -> Option<StudyDocument> {
    let selected = state.signature.as_ref()?.population;
    let catalog = catalog.clone();
    let population = catalog.populations.get(selected)?.key.clone();
    Some(StudyDocument {
        version: 2,
        camera: state.camera.clone(),
        seed: state.seed,
        population,
        wind: state.wind.clone(),
        settings,
        lighting,
        catalog,
        reference: state.refs.name(),
        comparison: state.comparison.clone(),
        render_size: state.render_size,
        patch_size: state.field_size,
        msaa_samples: state.msaa_samples,
        exposure_ev100: 13.0,
        sun_rotation: sun.0.rotation.to_array(),
        sun_color: sun.1.color.to_srgba().to_f32_array(),
        sun_illuminance: sun.1.illuminance,
        ambient_color: ambient.color.to_srgba().to_f32_array(),
        ambient_brightness: ambient.brightness,
        ground_color: GROUND_COLOR,
        show_character: state.show_character,
        show_ruler: state.show_ruler,
        character: state.character.clone(),
        ground: state.ground,
        edge: state.edge,
        character_profile: DEFAULT_CHARACTER_PRESENTATION_ID.into(),
        build: format!(
            "yarra-app-editor {} · {}",
            env!("CARGO_PKG_VERSION"),
            std::env::var("YARRA_STUDY_BUILD")
                .unwrap_or_else(|_| "unidentified local build".into())
        ),
    })
}

#[allow(clippy::too_many_arguments)]
fn capture(
    mut commands: Commands,
    mut state: ResMut<StudyState>,
    scene: Res<VegetationDebugScene>,
    mut settings: ResMut<VegetationDebugSettings>,
    lighting: Res<VegetationLighting>,
    diagnostics: Res<VegetationDiagnostics>,
    render_diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    render_device: Option<Res<bevy::render::renderer::RenderDevice>>,
    authoring: Res<VegetationAuthoringState>,
    ground: Res<ground::StudyGroundAssets>,
    treatment: Res<ground_treatment::TreatmentAssets>,
    sun: Single<(&Transform, &DirectionalLight), With<WorldSun>>,
    ambient: Res<GlobalAmbientLight>,
    mut exit: MessageWriter<AppExit>,
) {
    if state.launch.exit && state.capture_finished {
        exit.write(if state.capture_failed {
            AppExit::error()
        } else {
            AppExit::Success
        });
        return;
    }
    if state.launch.exit && state.started.elapsed().as_secs() > 60 {
        error!(
            "Vegetation capture timed out: {}",
            state
                .error
                .as_deref()
                .unwrap_or("waiting for renderer/screenshot")
        );
        exit.write(AppExit::error());
        return;
    }
    if state.pending > 0
        || state.error.is_some()
        || (state.show_character && !state.character_ready)
        || (state.ground != GroundMode::Neutral && !ground.ready)
    {
        return;
    }
    if state.capture_dir.is_some() && !settings.gpu_counters_enabled {
        settings.gpu_counters_enabled = true;
        state.ready_frames = 0;
        return;
    }
    let Some((catalog, selected, revision)) = authoring.study_source() else {
        return;
    };
    if state.signature != Some(state.scene_signature(revision, selected)) {
        return;
    }
    let Some(doc) = document(&state, catalog, *settings, *lighting, *sun, &ambient) else {
        return;
    };
    if state.save_study {
        let path = directory().join("study.ron");
        state.message = match doc.write(&path) {
            Ok(()) => format!("Study saved: {}", path.display()),
            Err(e) => e,
        };
        state.save_study = false;
    }
    let stats = diagnostics.snapshot();
    if state.capture_dir.is_none()
        || state.ready_frames < if state.launch.profile { 360 } else { 65 }
        || stats.gpu_scene_revision != scene.revision()
        || stats.gpu_samples < 2
    {
        return;
    }
    let folder = state.capture_dir.take().unwrap();
    if state.launch.profile {
        let mut lines = vec![format!("Device features: {:?}", render_device.as_ref().map(|d| d.features())),
            "Recent render samples after 360 ready frames. elapsed_gpu is GPU ms; elapsed_cpu is CPU submission time. Missing GPU entries mean unsupported, not zero cost.".into()];
        for diagnostic in render_diagnostics.iter() {
            if diagnostic.path().as_str().starts_with("render/") {
                lines.push(format!(
                    "{}\t{}\t{:?}",
                    diagnostic.path(),
                    diagnostic.suffix,
                    diagnostic.values().copied().collect::<Vec<_>>()
                ));
            }
        }
        if let Err(error) = std::fs::create_dir_all(&folder)
            .and_then(|()| std::fs::write(folder.join("render-timings.txt"), lines.join("\n")))
        {
            state.message = error.to_string();
            state.capture_finished = true;
            state.capture_failed = true;
            return;
        }
    }
    if state.ground.treatment().is_some()
        && let Err(error) = treatment.write_diagnostics(&folder)
    {
        state.message = error;
        state.capture_finished = true;
        state.capture_failed = true;
        return;
    }
    if let Err(error) = doc.write(&folder.join("study.ron")) {
        state.message = error;
        state.capture_finished = true;
        state.capture_failed = true;
        return;
    }
    if let Err(error) = std::fs::write(
        folder.join("diagnostics.txt"),
        format!(
            "{stats:#?}\nShape inspection: {:?} (active comparisons use high topology; not a performance measurement)\nView opening disabled: {}\nField: {} x {} m\nGround: {}\nGrass edge: {:?}\nCandidate budget: {} / {}\nBlade bands: {:?}; source density factor: {:.3}\nGPU timings: see render-timings.txt when --profile is active; editor FPS is not a vegetation timing.\n",
            settings.shape_inspection,
            settings.inspection_disable_opening,
            state.field_size,
            state.field_size,
            state.ground.label(),
            state.edge,
            state.candidates,
            if state.field_size == PATCH_SIZE {
                MAX_CANDIDATES
            } else {
                MAX_FIELD_CANDIDATES
            },
            settings.blade_bands,
            settings.blade_band_density
        ),
    ) {
        state.message = error.to_string();
        state.capture_finished = true;
        state.capture_failed = true;
        return;
    }
    state.playing = false;
    state.pending = 2;
    state.capture_finished = false;
    state.capture_failed = false;
    state.message = format!("Capturing {}", folder.display());
    for (request, file) in [
        (Screenshot::image(state.image.clone()), "viewport.png"),
        (Screenshot::primary_window(), "editor.png"),
    ] {
        let path = folder.join(file);
        commands.spawn(request).observe(
            move |event: On<ScreenshotCaptured>, mut state: ResMut<StudyState>| {
                let saved = event
                    .image
                    .clone()
                    .try_into_dynamic()
                    .map_err(|e| e.to_string())
                    .and_then(|image| image.to_rgb8().save(&path).map_err(|e| e.to_string()));
                if let Err(e) = saved {
                    state.capture_failed = true;
                    state.message = format!("Capture failed: {e}");
                }
                state.pending = state.pending.saturating_sub(1);
                if state.pending == 0 {
                    state.capture_finished = true;
                    if !state.capture_failed {
                        state.message = format!("Captured {}", path.parent().unwrap().display());
                        info!("{}", state.message);
                    }
                }
            },
        );
    }
}
