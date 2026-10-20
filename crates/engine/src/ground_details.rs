//! Ground details: stones, pebbles and twigs strewn where the ground surfaces say
//! ([`vegetation::DetailType`]). Each page of the grass scene within a type's distance of the
//! camera is placed on the CPU with the grass's lattice, clumps and coverage acceptance, and every
//! object is a plain mesh entity of one of its asset's pieces, which Bevy draws instanced; objects
//! dissolve out at their type's distance. Each page's objects of one type are spawned under one
//! root while the page lies within that type's distance: the scene's page content keys survive
//! rebases, which move the roots with the world.
use crate::world_streaming::WorldRenderRoot;

use bevy::{
    camera::visibility::VisibilityRange,
    ecs::system::SystemParam,
    gltf::{Gltf, GltfMaterial, GltfMaterialExtras, GltfMesh},
    light::NotShadowCaster,
    mesh::VertexAttributeValues,
    pbr::gltf::standard_material_from_gltf_material,
    prelude::*,
};

use std::collections::{HashMap, HashSet};

use vegetation::{
    DetailType, GrassField, GrassTypeId, Lattice, VegetationFieldPage, candidate_domain, hash32,
    random01, sample_candidate,
};

use vegetation_render::{VegetationRenderOrigin, VegetationSceneState, VegetationView};

/// Pages leave this far beyond their farthest type's distance, so moving back and forth across
/// the edge does not rebuild them.
const HYSTERESIS: f32 = 16.0;

/// Share of a type's distance over which its objects dissolve out.
const FADE_SHARE: f32 = 0.2;

pub(crate) struct GroundDetailsPlugin;

impl Plugin for GroundDetailsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DetailAssets>()
            .init_resource::<DetailPages>()
            .add_systems(
                Update,
                place_details
                    .after(crate::world_vegetation::WorldVegetationSystems)
                    // Headless apps (tests, tools) load no assets.
                    .run_if(resource_exists::<AssetServer>)
                    .run_if(resource_exists::<Assets<GltfMaterial>>)
                    .run_if(resource_exists::<Assets<StandardMaterial>>),
            );
    }
}

/// One piece of a detail asset: a mesh standing on the origin, `height` tall.
#[derive(Clone)]
struct Piece {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    height: f32,
    /// The material's glTF tags: plants sway and are shaded like tree foliage by them.
    tags: Option<GltfMaterialExtras>,
}

enum AssetState {
    Loading(Handle<Gltf>),
    Ready(Vec<Piece>),
    Failed,
}

#[derive(Resource, Default)]
struct DetailAssets(HashMap<String, AssetState>);

/// Spawned roots by page content key and type, and the catalog their objects were placed with.
#[derive(Resource, Default)]
struct DetailPages {
    pages: HashMap<(u64, GrassTypeId), Entity>,
    catalog: Option<vegetation::VegetationCatalog>,
}

#[derive(Component)]
struct DetailPage;

/// The loaded glTF assets pieces come from, and where their converted materials go.
#[derive(SystemParam)]
struct Sources<'w> {
    server: Res<'w, AssetServer>,
    gltfs: Res<'w, Assets<Gltf>>,
    gltf_meshes: Res<'w, Assets<GltfMesh>>,
    gltf_materials: Res<'w, Assets<GltfMaterial>>,
    meshes: Res<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
}

fn place_details(
    mut commands: Commands,
    scene: Res<VegetationSceneState>,
    origin: Res<VegetationRenderOrigin>,
    cameras: Query<&GlobalTransform, With<VegetationView>>,
    mut sources: Sources,
    mut assets: ResMut<DetailAssets>,
    mut pages: ResMut<DetailPages>,
) {
    let catalog = &scene.scene().catalog;

    // New looks or types: place every page again.
    if pages.catalog.as_ref() != Some(catalog) {
        for (_, entity) in pages.pages.drain() {
            commands.entity(entity).despawn();
        }
        pages.catalog = Some(catalog.clone());
    }

    if catalog.details.is_empty() {
        return;
    }

    let ready = load_assets(&catalog.details, &mut sources, &mut assets);
    let Some(camera) = cameras.iter().next().map(GlobalTransform::translation) else {
        return;
    };
    let keys = scene.page_keys();
    let mut live = HashSet::new();

    for (index, page) in scene.scene().pages.iter().enumerate() {
        let Some(&key) = keys.get(index) else {
            continue;
        };
        let distance = page_distance(page, camera);

        for field in &page.fields {
            let Some(detail) = catalog.detail(field.grass) else {
                continue;
            };
            let group = (key, field.grass);
            let spawned = pages.pages.contains_key(&group);

            if distance > detail.distance + if spawned { HYSTERESIS } else { 0. } {
                continue;
            }

            live.insert(group);

            if spawned || !ready.contains(detail.key.as_str()) {
                continue;
            }

            let entity = spawn_group(&mut commands, page, field, detail, origin.world_xz, &assets);
            pages.pages.insert(group, entity);
        }
    }

    pages.pages.retain(|group, entity| {
        let keep = live.contains(group);

        if !keep {
            commands.entity(*entity).despawn();
        }

        keep
    });
}

/// Starts loading every type's assets and returns the keys of types whose assets are all ready.
fn load_assets<'a>(
    details: &'a [DetailType],
    sources: &mut Sources,
    assets: &mut DetailAssets,
) -> HashSet<&'a str> {
    let mut ready = HashSet::new();

    for detail in details {
        let mut all = true;

        for path in &detail.assets {
            let state = assets
                .0
                .entry(path.clone())
                .or_insert_with(|| AssetState::Loading(sources.server.load(path.clone())));

            if let AssetState::Loading(handle) = state {
                let loaded = sources
                    .gltfs
                    .get(handle.id())
                    .map(|gltf| gltf.meshes.clone());
                if let Some(meshes) = loaded {
                    *state = pieces(&meshes, sources).map_or_else(
                        || {
                            warn!("ground detail asset {path} has no usable pieces");
                            AssetState::Failed
                        },
                        AssetState::Ready,
                    );
                } else if sources.server.load_state(handle.id()).is_failed() {
                    warn!("ground detail asset {path} failed to load");
                    *state = AssetState::Failed;
                }
            }

            all &= matches!(state, AssetState::Ready(_));
        }

        if all {
            ready.insert(detail.key.as_str());
        }
    }

    ready
}

/// The pieces of a type: every mesh of its assets, in order.
fn type_pieces(detail: &DetailType, assets: &DetailAssets) -> Vec<Piece> {
    detail
        .assets
        .iter()
        .filter_map(|path| match assets.0.get(path) {
            Some(AssetState::Ready(pieces)) => Some(pieces.iter().cloned()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// An asset's meshes in order, each its first primitive. Pieces sharing a glTF material
/// share one converted material, so they draw instanced.
fn pieces(meshes: &[Handle<GltfMesh>], sources: &mut Sources) -> Option<Vec<Piece>> {
    let mut converted = HashMap::new();
    let mut pieces = Vec::new();

    for handle in meshes {
        let Some(primitive) = sources
            .gltf_meshes
            .get(handle)
            .and_then(|m| m.primitives.first())
        else {
            continue;
        };
        let (Some(mesh), Some(source)) = (
            sources.meshes.get(&primitive.mesh),
            primitive.material.as_ref(),
        ) else {
            continue;
        };

        // Bounds are only kept once a mesh is on the GPU; read the positions while loading.
        let Ok(VertexAttributeValues::Float32x3(positions)) =
            mesh.try_attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        let (low, high) = positions
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(l, h), p| {
                (l.min(p[1]), h.max(p[1]))
            });
        let material = match converted.get(&source.id()) {
            Some(material) => Handle::clone(material),
            None => {
                let Some(gltf_material) = sources.gltf_materials.get(source) else {
                    continue;
                };
                let material = sources
                    .materials
                    .add(standard_material_from_gltf_material(gltf_material));
                converted.insert(source.id(), material.clone());
                material
            }
        };
        pieces.push(Piece {
            mesh: primitive.mesh.clone(),
            material,
            height: (high - low).max(0.001),
            tags: primitive
                .material_extras
                .as_ref()
                .map(|e| GltfMaterialExtras {
                    value: e.value.clone(),
                }),
        });
    }

    (!pieces.is_empty()).then_some(pieces)
}

/// Horizontal distance from the camera to the nearest point of the page.
fn page_distance(page: &VegetationFieldPage, camera: Vec3) -> f32 {
    let min = Vec2::from(page.origin_xz);
    let nearest = camera.xz().clamp(min, min + page.size);
    nearest.distance(camera.xz())
}

/// One page's objects of one type, under a root at the page's corner.
fn spawn_group(
    commands: &mut Commands,
    page: &VegetationFieldPage,
    field: &GrassField,
    detail: &DetailType,
    origin: [f64; 2],
    assets: &DetailAssets,
) -> Entity {
    let corner = Vec3::new(page.origin_xz[0], 0., page.origin_xz[1]);
    let root = commands
        .spawn((
            DetailPage,
            WorldRenderRoot,
            Transform::from_translation(corner),
            Visibility::default(),
        ))
        .id();
    let pieces = type_pieces(detail, assets);

    if pieces.is_empty() {
        return root;
    }

    let range = VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: detail.distance * (1. - FADE_SHARE)..detail.distance,
        use_aabb: false,
    };

    for (piece, transform) in place(page, field, detail, origin, &pieces) {
        let mut object = commands.spawn((
            Mesh3d(piece.mesh.clone()),
            MeshMaterial3d(piece.material.clone()),
            Transform {
                translation: transform.translation - corner,
                ..transform
            },
            range.clone(),
            ChildOf(root),
        ));

        if !detail.shadows {
            object.insert(NotShadowCaster);
        }

        if let Some(tags) = &piece.tags {
            object.insert(tags.clone());
        }
    }

    root
}

/// Every object `detail` grows on `page`, in render coordinates: one candidate per cell of the
/// type's world-aligned lattice, kept where the page owns it, its ground is valid and coverage,
/// thinned by its clump, beats its rank (as `vegetation::accepted`).
fn place<'p>(
    page: &VegetationFieldPage,
    field: &GrassField,
    detail: &DetailType,
    origin: [f64; 2],
    pieces: &'p [Piece],
) -> Vec<(&'p Piece, Transform)> {
    let canonical = |v: [f32; 2]| {
        [
            (f64::from(v[0]) + origin[0]) as f32,
            (f64::from(v[1]) + origin[1]) as f32,
        ]
    };
    let domain = candidate_domain(canonical(page.origin_xz), page.size, detail);
    let mut objects = Vec::new();

    for index in 0..domain.candidate_count() {
        let Some(candidate) = sample_candidate(detail, domain, index) else {
            continue;
        };
        let root = [
            (f64::from(candidate.root_xz[0]) - origin[0]) as f32,
            (f64::from(candidate.root_xz[1]) - origin[1]) as f32,
        ];

        if !page.owns(root) {
            continue;
        }

        let surface = page.surface.sample(page.origin_xz, page.size, root);

        if surface.validity < 0.5
            || candidate.rank >= field.sample_coverage(page, root) * candidate.clump.retention
        {
            continue;
        }

        let seed = hash32(candidate.seed ^ detail.seed());
        let piece = &pieces[seed as usize % pieces.len()];
        let scale =
            detail.scale[0] + (detail.scale[1] - detail.scale[0]) * random01(seed ^ 0x1b87_3593);
        let normal = Vec3::from(surface.normal);
        let up = Vec3::Y.lerp(normal, detail.align).normalize_or(Vec3::Y);
        let yaw = random01(seed ^ 0x85eb_ca6b) * std::f32::consts::TAU;
        let lean = random01(seed ^ 0xc2b2_ae35) * detail.tilt;
        let lean_axis = random01(seed ^ 0x27d4_eb2f) * std::f32::consts::TAU;
        let rotation = Quat::from_rotation_arc(Vec3::Y, up)
            * Quat::from_axis_angle(Vec3::new(lean_axis.cos(), 0., lean_axis.sin()), lean)
            * Quat::from_rotation_y(yaw);
        let sink = detail.sink * piece.height * scale;
        objects.push((
            piece,
            Transform {
                translation: Vec3::new(root[0], surface.height, root[1]) - up * sink,
                rotation,
                scale: Vec3::splat(scale),
            },
        ));
    }

    objects
}

#[cfg(test)]
mod tests;
