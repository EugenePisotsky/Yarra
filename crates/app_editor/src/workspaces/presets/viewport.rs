use super::fixture::{Product, Request};
use super::*;
use crate::vegetation_authoring::VegetationAuthoringState;
use bevy::{
    camera::{Exposure, RenderTarget, visibility::RenderLayers},
    render::{render_resource::TextureFormat, view::Msaa},
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures};
use engine::WorldSun;
use terrain_render::{
    PrepareTerrainMaterialContext, TerrainMacroVariation, TerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use vegetation_render::{
    VegetationDebugScene, VegetationDebugSettings, VegetationLighting, VegetationWind,
};

const LAYER: usize = 30;
#[derive(Component)]
pub(crate) struct PresetWorkspaceCamera;
#[derive(Component)]
struct PreviewGround;
struct SavedWorld {
    scene: VegetationDebugScene,
    settings: VegetationDebugSettings,
    wind: VegetationWind,
    lighting: VegetationLighting,
    sun: (Entity, Transform, DirectionalLight, Option<RenderLayers>),
    ambient: GlobalAmbientLight,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    source_epoch: u64,
    draft: u64,
    plants: u64,
    space: WorldSpaceId,
    preset: Option<PresetId>,
    mode: Mode,
    roads: u64,
    base: TerrainSurfaceId,
    size: u16,
    footprint: Footprint,
    underlay: Option<PresetId>,
}
struct GroundAssets {
    entity: Entity,
    mesh: Handle<Mesh>,
    material: Handle<TerrainMaterial>,
    weights: Handle<Image>,
}
#[derive(Resource, Default)]
pub(super) struct PreviewState {
    saved: Option<SavedWorld>,
    task: Option<(Key, Task<Result<Product, String>>)>,
    desired: Option<Key>,
    submitted: Option<Key>,
    ground: Option<GroundAssets>,
    pub error: Option<String>,
    pub candidates: u32,
    pub current: bool,
}
impl PreviewState {
    pub(super) fn matches(&self, state: &PresetAuthoringState) -> bool {
        self.current
            && self.desired.is_some_and(|k| {
                k.draft == state.revision
                    && k.mode == state.mode
                    && k.size == state.size
                    && k.footprint == state.footprint
                    && k.underlay == state.preview_underlay()
                    && Some(k.base) == state.base
                    && k.preset == state.selected
                    && Some(k.space) == state.space
            })
    }
    pub(super) fn pending(&self) -> bool {
        !self.current && self.error.is_none()
    }
}
pub(super) fn setup(
    mut commands: Commands,
    mut state: ResMut<PresetAuthoringState>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<EguiUserTextures>,
) {
    let image = images.add(Image::new_target_texture(
        960,
        720,
        TextureFormat::Rgba8UnormSrgb,
        None,
    ));
    state.texture = textures.add_image(EguiTextureHandle::Strong(image.clone()));
    state.image = image.clone();
    commands.spawn((
        Camera3d::default(),
        Camera {
            is_active: false,
            order: -2,
            clear_color: ClearColorConfig::Custom(Color::srgb(0.065, 0.08, 0.095)),
            ..default()
        },
        RenderTarget::Image(image.into()),
        Projection::Perspective(PerspectiveProjection {
            fov: 50f32.to_radians(),
            near: 0.02,
            far: 96.0,
            ..default()
        }),
        Msaa::Off,
        Exposure { ev100: 13.0 },
        state.camera(),
        RenderLayers::layer(LAYER),
        PresetWorkspaceCamera,
        Name::new("Environment preset viewport"),
    ));
}
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct SharedRender<'w, 's> {
    scene: ResMut<'w, VegetationDebugScene>,
    settings: ResMut<'w, VegetationDebugSettings>,
    wind: ResMut<'w, VegetationWind>,
    lighting: ResMut<'w, VegetationLighting>,
    ambient: ResMut<'w, GlobalAmbientLight>,
    sun: Single<
        'w,
        's,
        (
            Entity,
            &'static mut Transform,
            &'static mut DirectionalLight,
            Option<&'static RenderLayers>,
        ),
        With<WorldSun>,
    >,
}
pub(super) fn enter(
    mut commands: Commands,
    mut preview: ResMut<PreviewState>,
    mut state: ResMut<PresetAuthoringState>,
    mut shared: SharedRender,
) {
    let (entity, transform, light, layers) = &mut *shared.sun;
    preview.saved = Some(SavedWorld {
        scene: shared.scene.clone(),
        settings: *shared.settings,
        wind: *shared.wind,
        lighting: *shared.lighting,
        ambient: shared.ambient.clone(),
        sun: (*entity, **transform, **light, layers.cloned()),
    });
    commands.entity(*entity).insert(RenderLayers::layer(LAYER));
    **transform = Transform::from_xyz(-8.0, 12.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y);
    light.color = Color::WHITE;
    light.illuminance = 15_000.0;
    shared.ambient.color = Color::WHITE;
    shared.ambient.brightness = 300.0;
    *shared.settings = VegetationDebugSettings::default();
    *shared.lighting = VegetationLighting::default();
    shared.lighting.canopy_origin = [0.0; 2];
    *shared.wind = VegetationWind::default();
    shared.wind.externally_driven = true;
    shared.wind.set_phase_seconds(state.phase);
    // The render resources are exclusively owned until this workspace exits.
    let _ = shared.scene.replace(vegetation::VegetationScene {
        catalog: vegetation::VegetationCatalog {
            species: vec![],
            populations: vec![],
            assemblages: vec![],
        },
        pages: vec![],
    });
    state.bump();
    preview.desired = None;
    preview.submitted = None;
    preview.current = false;
    preview.error = None;
}
pub(super) fn leave(
    mut commands: Commands,
    mut preview: ResMut<PreviewState>,
    mut shared: SharedRender,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<PresetAuthoringState>,
) {
    if let Some(saved) = preview.saved.take() {
        *shared.scene = saved.scene;
        *shared.settings = saved.settings;
        *shared.wind = saved.wind;
        *shared.lighting = saved.lighting;
        *shared.ambient = saved.ambient;
        let (entity, t, l, layers) = saved.sun;
        let mut e = commands.entity(entity);
        e.insert((t, l));
        if let Some(layers) = layers {
            e.insert(layers);
        } else {
            e.remove::<RenderLayers>();
        }
    }
    remove_ground(
        &mut commands,
        &mut preview,
        &mut materials,
        &mut meshes,
        &mut images,
    );
    preview.current = false;
    state.playing = false;
}
fn remove_ground(
    commands: &mut Commands,
    preview: &mut PreviewState,
    materials: &mut Assets<TerrainMaterial>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
) {
    if let Some(old) = preview.ground.take() {
        commands.entity(old.entity).despawn();
        materials.remove(old.material.id());
        meshes.remove(old.mesh.id());
        images.remove(old.weights.id());
    }
}
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PreviewAssets<'w> {
    server: Res<'w, AssetServer>,
    images: ResMut<'w, Assets<Image>>,
    materials: ResMut<'w, Assets<TerrainMaterial>>,
    meshes: ResMut<'w, Assets<Mesh>>,
}
#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    mut commands: Commands,
    mut state: ResMut<PresetAuthoringState>,
    mut preview: ResMut<PreviewState>,
    project: Res<ProjectEditorStore>,
    dense: Res<DenseDomainWorkingSets>,
    plants: Res<VegetationAuthoringState>,
    mut scene: ResMut<VegetationDebugScene>,
    mut wind: ResMut<VegetationWind>,
    time: Res<Time>,
    mut assets: PreviewAssets,
    mut cameras: Query<&mut Transform, With<PresetWorkspaceCamera>>,
) {
    state.refresh(&dense, &project);
    for mut camera in &mut cameras {
        *camera = state.camera();
    }
    if state.playing {
        state.phase = (state.phase + time.delta_secs().min(0.1)).rem_euclid(4096.0);
    }
    wind.set_phase_seconds(state.phase);
    let Some((catalog, _, revision)) = plants.study_source() else {
        preview.current = false;
        return;
    };
    let (Some(space), Some(base), Some(draft)) = (state.space, state.base, state.draft.as_ref())
    else {
        preview.current = false;
        return;
    };
    let road = if state.mode == Mode::Roads {
        let Some(d) = &state.styles.draft else {
            preview.current = false;
            preview.desired = None;
            preview.submitted = None;
            // Drop pending work so an empty selection cannot wedge the next preview.
            preview.task = None;
            preview.error = Some("Choose or create a road style".into());
            return;
        };
        Some(fixture::RoadFixture {
            profile: d.value.clone(),
            curved: state.styles.curved,
            width: state.styles.width,
        })
    } else {
        None
    };
    if road.is_none() && state.selected.is_none() {
        preview.current = false;
        preview.desired = None;
        preview.submitted = None;
        preview.task = None;
        preview.error = Some("Choose or create a preset".into());
        return;
    }
    let key = Key {
        source_epoch: project.source_epoch(),
        draft: state.revision,
        plants: revision,
        space,
        preset: state.selected,
        mode: state.mode,
        roads: dense.roads.revision,
        base,
        size: state.size,
        footprint: state.footprint,
        underlay: state.preview_underlay(),
    };
    if preview.desired != Some(key) {
        preview.desired = Some(key);
        preview.current = false;
        preview.error = None;
    }
    let complete = preview
        .task
        .as_mut()
        .and_then(|(key, task)| block_on(poll_once(task)).map(|r| (*key, r)));
    if let Some((completed, result)) = complete {
        preview.task = None;
        if preview.desired == Some(completed) {
            let result = result.and_then(|product| {
                let resources = project
                    .terrain_resources(space)
                    .ok_or("Terrain resources are loading")?;
                let layers = product
                    .ground
                    .surfaces
                    .iter()
                    .filter_map(|id| {
                        resources
                            .surfaces
                            .iter()
                            .find(|s| s.surface.id == *id)
                            .map(|s| TerrainSurfaceLayer {
                                surface: s.surface.clone(),
                                layer: s.layer,
                            })
                    })
                    .collect::<Vec<_>>();
                let plane = if let Some(heightfield) = &product.terrain {
                    terrain_render::build_heightfield_mesh(heightfield, product.size)?
                } else {
                    let mut plane = Plane3d::default()
                        .mesh()
                        .size(product.size, product.size)
                        .build();
                    plane.generate_tangents().map_err(|e| e.to_string())?;
                    plane
                };
                let PreviewAssets {
                    server,
                    images,
                    materials,
                    meshes,
                } = &mut assets;
                let prepared = prepare_terrain_material(PrepareTerrainMaterialContext {
                    asset_server: server,
                    images,
                    materials,
                    cell: world::CellCoord::ZERO,
                    origin_cell: world::CellCoord::ZERO,
                    cell_size: product.size,
                    page_surfaces: &product.ground.surfaces,
                    weight_pages: &product.ground.weight_pages,
                    profile: &resources.profile,
                    texture_set: &resources.texture_set,
                    surfaces: &layers,
                    macro_variation: TerrainMacroVariation::Enabled,
                })?;
                scene.replace(product.scene).map_err(|e| e.to_string())?;
                remove_ground(&mut commands, &mut preview, materials, meshes, images);
                let mesh = meshes.add(plane);
                let entity = commands
                    .spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(prepared.material.clone()),
                        // Material control coordinates and foliage both occupy [0, size].
                        Transform::from_xyz(
                            product.size / 2.0,
                            if product.terrain.is_some() {
                                0.0
                            } else {
                                -0.005
                            },
                            product.size / 2.0,
                        ),
                        RenderLayers::layer(LAYER),
                        PreviewGround,
                        Name::new("Compiled preset patch"),
                    ))
                    .id();
                preview.ground = Some(GroundAssets {
                    entity,
                    mesh,
                    material: prepared.material,
                    weights: prepared.weight_image,
                });
                preview.candidates = product.candidates;
                Ok(())
            });
            match result {
                Ok(()) => preview.current = true,
                Err(e) => preview.error = Some(e),
            }
        }
    }
    if preview.task.is_none() && preview.submitted != Some(key) {
        let Some(definition) = dense.definition(space) else {
            return;
        };
        let request = Request {
            road,
            library: draft.library.clone(),
            plants: catalog.clone(),
            preset: state.selected.unwrap_or(PresetId([0; 16])),
            base,
            surfaces: definition.surfaces.clone(),
            size: state.size,
            footprint: state.footprint,
            underlay: state.preview_underlay(),
        };
        preview.task = Some((
            key,
            AsyncComputeTaskPool::get().spawn(async move { request.compile() }),
        ));
        preview.submitted = Some(key);
    }
}
