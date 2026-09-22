//! Study camera, scene/pose updates and metric stage entities.
use crate::{
    vegetation_authoring::VegetationAuthoringState,
    workspaces::{
        EditorWorkspace,
        vegetation::{
            references::References,
            stage::CharacterPlacement,
            state::StudyState,
            study::{StudyCamera, bounded_field_with_edge},
        },
    },
};
use bevy::{
    camera::{Exposure, RenderTarget, visibility::RenderLayers},
    prelude::*,
    render::{render_resource::TextureFormat, view::Msaa},
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};
use engine::{CharacterPresentationPreview, DEFAULT_CHARACTER_PRESENTATION_ID};
use vegetation_render::{VegetationDebugSettings, VegetationSceneState, VegetationWind};

pub(super) const LAYER: usize = 31;
pub(super) const GROUND_COLOR: [f32; 3] = [0.105, 0.12, 0.065];

#[derive(Component)]
pub(crate) struct VegetationWorkspaceCamera;

#[derive(Component)]
pub(super) struct StudyGround;

#[derive(Component)]
pub(super) struct StudyCharacter;

#[derive(Component)]
pub(super) struct StudyScaleRuler;
pub(super) fn setup(
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

pub(super) fn sync_scale_figure(
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
pub(super) fn sync(
    time: Res<Time>,
    mut state: ResMut<StudyState>,
    mut authoring: ResMut<VegetationAuthoringState>,
    mut scene: ResMut<VegetationSceneState>,
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
