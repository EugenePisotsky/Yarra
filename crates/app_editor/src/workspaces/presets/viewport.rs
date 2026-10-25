use super::fixture::{Product, Request};
use super::*;
use crate::{grass::GrassPreview, workspaces::SavedDaylight};
use bevy::world_serialization::WorldInstanceReady;

use bevy::{
    camera::{Exposure, RenderTarget, visibility::RenderLayers},
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass},
    render::{render_resource::TextureFormat, view::Msaa},
    tasks::{AsyncComputeTaskPool, Task, block_on, poll_once},
};

use bevy_egui::{EguiTextureHandle, EguiUserTextures};
use engine::WorldSun;
use std::time::{Duration, Instant};

use terrain_render::{
    TerrainSurfaceLayer,
    clipmap::{ClipmapGround, ClipmapPatch, ClipmapTerrainMaterial, ClipmapUploads, PatchAssets},
};

use vegetation_render::{
    VegetationLighting, VegetationSceneState, VegetationSettings, VegetationWind,
};

const LAYER: usize = 30;

#[derive(Component)]
pub(crate) struct PresetWorkspaceCamera;

#[derive(Component)]
struct PreviewGround;

struct SavedWorld {
    scene: VegetationSceneState,
    settings: VegetationSettings,
    wind: VegetationWind,
    lighting: VegetationLighting,
    daylight: SavedDaylight,
    sun_layers: Option<RenderLayers>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    source_epoch: u64,
    draft: u64,
    plants: u64,
    space: WorldSpaceId,
    preset: Option<PresetId>,
    mode: Mode,
    base: TerrainSurfaceId,
    size: u16,
    footprint: Footprint,
    underlay: Option<PresetId>,
}

struct GroundAssets {
    entity: Entity,
    patch: ClipmapPatch,
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
    pub objects: usize,
    pub assets: Vec<world_db::CollectionAssetView>,
    object_entities: Vec<Entity>,
    framed: Option<(Option<PresetId>, Mode, u16, bool)>,
    pub current: bool,
    pending_since: Option<Instant>,
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

    pub(super) fn has_image(&self) -> bool {
        self.ground.is_some()
    }

    pub(super) fn slow_update(&self, now: Instant) -> bool {
        !self.current
            && self.error.is_none()
            && self.pending_since.is_some_and(|since| {
                now.saturating_duration_since(since) >= Duration::from_millis(300)
            })
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
        // Environment materials draw into a G-buffer, lit by the deferred lighting.
        DepthPrepass,
        DeferredPrepass,
        Exposure { ev100: 13.0 },
        state.camera(),
        RenderLayers::layer(LAYER),
        PresetWorkspaceCamera,
        Name::new("Environment preset viewport"),
    ));
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct SharedRender<'w, 's> {
    scene: ResMut<'w, VegetationSceneState>,
    settings: ResMut<'w, VegetationSettings>,
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
        daylight: SavedDaylight::save(transform, light, &shared.ambient),
        sun_layers: layers.cloned(),
    });
    commands.entity(*entity).insert(RenderLayers::layer(LAYER));
    **transform = Transform::from_xyz(-8.0, 12.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y);
    light.color = Color::WHITE;
    light.illuminance = 15_000.0;
    shared.ambient.color = Color::WHITE;
    shared.ambient.brightness = 300.0;
    *shared.settings = VegetationSettings::default();
    *shared.lighting = VegetationLighting::default();
    *shared.wind = VegetationWind::default();
    shared.wind.externally_driven = true;
    shared.wind.set_phase_seconds(state.phase);

    // The render resources are exclusively owned until this workspace exits.
    let _ = shared.scene.replace(vegetation::VegetationScene {
        catalog: vegetation::VegetationCatalog::default(),
        pages: vec![],
    });
    state.bump();
    preview.desired = None;
    preview.submitted = None;
    preview.current = false;
    preview.error = None;
    preview.pending_since = None;
}

pub(super) fn leave(
    mut commands: Commands,
    mut preview: ResMut<PreviewState>,
    mut shared: SharedRender,
    mut materials: ResMut<Assets<ClipmapTerrainMaterial>>,
    mut uploads: ResMut<ClipmapUploads>,
    mut state: ResMut<PresetAuthoringState>,
) {
    if let Some(saved) = preview.saved.take() {
        *shared.scene = saved.scene;
        *shared.settings = saved.settings;
        *shared.wind = saved.wind;
        *shared.lighting = saved.lighting;
        let (entity, transform, light, _) = &mut *shared.sun;
        saved
            .daylight
            .restore(transform, light, &mut shared.ambient);
        let mut sun = commands.entity(*entity);

        if let Some(layers) = saved.sun_layers {
            sun.insert(layers);
        } else {
            sun.remove::<RenderLayers>();
        }
    }

    remove_ground(&mut commands, &mut preview, &mut materials, &mut uploads);
    preview.current = false;
    preview.pending_since = None;
    state.playing = false;
}

fn remove_ground(
    commands: &mut Commands,
    preview: &mut PreviewState,
    materials: &mut Assets<ClipmapTerrainMaterial>,
    uploads: &mut ClipmapUploads,
) {
    for entity in preview.object_entities.drain(..) {
        commands.entity(entity).despawn();
    }

    if let Some(old) = preview.ground.take() {
        commands.entity(old.entity).despawn();
        old.patch.remove(uploads, materials);
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct PreviewAssets<'w> {
    server: Res<'w, AssetServer>,
    images: ResMut<'w, Assets<Image>>,
    buffers: ResMut<'w, Assets<bevy::render::storage::ShaderBuffer>>,
    materials: ResMut<'w, Assets<ClipmapTerrainMaterial>>,
    meshes: ResMut<'w, Assets<Mesh>>,
    uploads: ResMut<'w, ClipmapUploads>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    mut commands: Commands,
    mut state: ResMut<PresetAuthoringState>,
    mut preview: ResMut<PreviewState>,
    project: Res<ProjectEditorStore>,
    path: Res<crate::project_store::ProjectDatabasePath>,
    dense: Res<SourceWorkingSets>,
    grass: Res<GrassPreview>,
    mut scene: ResMut<VegetationSceneState>,
    mut wind: ResMut<VegetationWind>,
    time: Res<Time>,
    mut assets: PreviewAssets,
    mut cameras: Query<&mut Transform, With<PresetWorkspaceCamera>>,
) {
    state.refresh(&dense, &project);

    for mut camera in &mut cameras {
        *camera = state.camera();
    }

    if let Some(ground) = &preview.ground {
        ground
            .patch
            .follow(state.camera().translation, &mut assets.uploads);
    }

    if state.playing {
        state.phase = (state.phase + time.delta_secs().min(0.1)).rem_euclid(4096.0);
    }

    wind.set_phase_seconds(state.phase);
    let revision = grass.revision();
    let Some(catalog) = grass.catalog() else {
        preview.current = false;
        return;
    };
    let (Some(space), Some(base), Some(_)) = (state.space, state.base, state.draft.as_ref()) else {
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
            traffic: state
                .styles
                .traffic
                .unwrap_or(environment::roads::DEFAULT_TRAFFIC),
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
        base,
        size: state.size,
        footprint: state.footprint,
        underlay: state.preview_underlay(),
    };

    if preview.desired != Some(key) {
        preview.desired = Some(key);
        preview.current = false;
        preview.error = None;
        preview.pending_since.get_or_insert_with(Instant::now);
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
                let PreviewAssets {
                    server,
                    images,
                    buffers,
                    materials,
                    meshes,
                    uploads,
                } = &mut assets;
                let patch = ground_patch(
                    &product,
                    space,
                    resources,
                    server,
                    uploads,
                    PatchAssets {
                        images,
                        buffers,
                        materials,
                    },
                )?;
                scene.replace(product.scene).map_err(|e| e.to_string())?;
                remove_ground(&mut commands, &mut preview, materials, uploads);
                let entity = commands
                    .spawn((
                        patch.bundle(meshes),
                        RenderLayers::layer(LAYER),
                        PreviewGround,
                        Name::new("Compiled preset patch"),
                    ))
                    .id();
                preview.ground = Some(GroundAssets { entity, patch });
                let height = product
                    .objects
                    .iter()
                    .filter_map(|o| {
                        product
                            .assets
                            .iter()
                            .find(|a| a.id == o.asset)
                            .map(|a| a.variants[0].bounds[1] * o.scale + o.translation[1])
                    })
                    .fold(0.0_f32, f32::max);
                let framing = (key.preset, key.mode, key.size, !product.objects.is_empty());
                state.focus_height = (height * 0.5).max(0.25);
                state.fit_distance = (product.size * 1.375).max(height * 1.8).min(80.0);

                if preview.framed != Some(framing) {
                    state.distance = state.fit_distance;
                    preview.framed = Some(framing);
                }

                for object in &product.objects {
                    let asset = product
                        .assets
                        .iter()
                        .find(|a| a.id == object.asset)
                        .ok_or("Missing preview asset")?;
                    let entity = commands
                        .spawn((
                            WorldAssetRoot(
                                server.load(
                                    bevy::gltf::GltfAssetLabel::Scene(0)
                                        .from_asset(asset.variants[0].uri.clone()),
                                ),
                            ),
                            Transform::from_translation(Vec3::from_array(object.translation))
                                .with_rotation(Quat::from_rotation_y(object.yaw))
                                .with_scale(Vec3::splat(object.scale)),
                            RenderLayers::layer(LAYER),
                            Name::new(format!("Preset preview {}", asset.name)),
                        ))
                        .observe(set_object_layers)
                        .id();
                    preview.object_entities.push(entity);
                }

                preview.objects = product.objects.len();
                preview.assets = product.assets;
                preview.candidates = product.candidates;
                Ok(())
            });

            match result {
                Ok(()) => preview.current = true,
                Err(e) => preview.error = Some(e),
            }

            preview.pending_since = None;
        }
    }

    if preview.task.is_none() && preview.submitted != Some(key) {
        let Some(definition) = dense.definition(space) else {
            return;
        };
        let request = Request {
            road,
            library: state.draft.as_ref().unwrap().library.clone(),
            plants: catalog.clone(),
            preset: state.selected.unwrap_or(PresetId([0; 16])),
            base,
            surfaces: definition.surfaces.clone(),
            worn: definition.worn.clone(),
            size: state.size,
            footprint: state.footprint,
            underlay: state.preview_underlay(),
        };
        let database_path = path.0.clone();
        preview.task = Some((
            key,
            AsyncComputeTaskPool::get().spawn(async move {
                let mut product = request.compile()?;
                product.assets = world_db::ProjectReader::open_read_only(&database_path)
                    .and_then(|reader| reader.read_collection_assets(&product.asset_ids))
                    .map_err(|e| e.to_string())?;
                Ok(product)
            }),
        ));
        preview.submitted = Some(key);
    }
}

/// The previewed cell as a clipmap patch: its ground among the world's surfaces with the
/// preview's grass, worn where its road wears it, as the game draws a cell.
fn ground_patch(
    product: &Product,
    space: WorldSpaceId,
    resources: &world_db::TerrainRenderResources,
    server: &AssetServer,
    uploads: &mut ClipmapUploads,
    assets: PatchAssets,
) -> Result<ClipmapPatch, String> {
    let grass = &product.scene.catalog;
    let layers: Vec<_> = resources
        .surfaces
        .iter()
        .map(|s| TerrainSurfaceLayer {
            surface: s.surface.clone(),
            layer: s.layer,
            grass: grass.cover(s.surface.id.0).rgba(),
        })
        .collect();
    let ground = ClipmapGround::new(server, &resources.profile, &resources.texture_set, &layers);
    let slot_layers = product
        .ground
        .surfaces
        .iter()
        .map(|id| {
            let surface = resources.surfaces.iter().find(|s| s.surface.id == *id);
            surface
                .map(|s| s.layer)
                .ok_or_else(|| format!("unknown terrain surface {id:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let cell = world::CellCoord::ZERO;
    let control = world::leaf_control(
        &slot_layers,
        &product.ground.weight_pages,
        product.wear.as_ref(),
        cell,
        product.size,
    )?;
    let field = preview_relief(product)?;
    let key = world::TerrainNodeKey::leaf(space, cell);
    let node =
        world::TerrainNode::leaf(key, &field, fixture::NODE_SAMPLES).map_err(|e| e.to_string())?;
    ClipmapPatch::new(
        &node,
        product.size,
        control,
        product.wear.as_ref(),
        ground,
        uploads,
        assets,
    )
}

/// The previewed cell's relief; without one, flat just below the grass roots.
fn preview_relief(product: &Product) -> Result<world::TerrainHeightfield, String> {
    if let Some(field) = &product.terrain {
        return Ok(field.clone());
    }

    let n = usize::from(fixture::NODE_SAMPLES);
    let h = -0.005;
    world::TerrainHeightfield::from_heights(
        fixture::NODE_SAMPLES,
        &vec![h; n * n],
        h,
        h,
        product.size,
    )
    .map_err(|e| e.to_string())
}

fn set_object_layers(
    event: On<WorldInstanceReady>,
    children: Query<&Children>,
    mut commands: Commands,
) {
    for entity in children.iter_descendants(event.entity) {
        commands.entity(entity).insert(RenderLayers::layer(LAYER));
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    #[test]
    fn brief_rebuilds_are_quiet_but_long_updates_report_progress() {
        let start = Instant::now();
        let mut preview = PreviewState {
            pending_since: Some(start),
            ..Default::default()
        };
        assert!(!preview.slow_update(start + Duration::from_millis(100)));
        assert!(preview.slow_update(start + Duration::from_millis(400)));
        preview.current = true;
        assert!(!preview.slow_update(start + Duration::from_secs(1)));
        preview.current = false;
        preview.error = Some("Invalid road geometry".into());
        assert!(!preview.slow_update(start + Duration::from_secs(1)));
    }
}
