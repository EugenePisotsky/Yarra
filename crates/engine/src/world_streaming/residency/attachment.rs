//! Translate source payloads into ECS entities and explicitly owned terrain assets.
use super::PreparedPage;
use crate::object_lod::{ScreenSpaceLod, ScreenSpaceLodVariant};
use crate::world_streaming::{
    GameplayObject, GeneratedEnvironmentObject, StreamedPageEntity, StreamedTerrainSurface,
    StreamedVegetationFieldPage, StreamedVisualObject,
};
use bevy::{gltf::GltfAssetLabel, prelude::*};
use std::collections::HashMap;
#[cfg(not(target_os = "ios"))]
use terrain_render::build_heightfield_mesh;
use terrain_render::{
    PrepareTerrainMaterialContext, TerrainMacroVariation, TerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use vegetation::VegetationCatalog;
use world::{
    AssetId, CellCoord, ObjectActivationPolicy, PagePayload, TerrainHeightfield,
    TerrainTextureSetId,
};
use world_db::PageDependency;

#[derive(Default)]
pub(in crate::world_streaming) struct PageAttachment {
    pub(in crate::world_streaming) entities: Vec<Entity>,
    pub(in crate::world_streaming) owned_terrain_meshes: Vec<Handle<Mesh>>,
    pub(in crate::world_streaming) owned_terrain_materials: Vec<Handle<TerrainMaterial>>,
    pub(in crate::world_streaming) owned_terrain_images: Vec<Handle<Image>>,
    pub(in crate::world_streaming) decoded_bytes: u64,
    pub(in crate::world_streaming) gpu_bytes_estimate: u64,
    pub(in crate::world_streaming) gameplay_objects: usize,
    pub(in crate::world_streaming) vegetation_pages: usize,
    pub(in crate::world_streaming) height_only_pages: usize,
    pub(in crate::world_streaming) terrain_texture_set: Option<(TerrainTextureSetId, u64)>,
}

#[derive(Resource)]
pub(in crate::world_streaming) struct WorldRenderAssets {
    pub(super) unit_plane: Handle<Mesh>,
}

pub(in crate::world_streaming) fn create_world_render_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let mut unit_plane = Plane3d::default().mesh().size(1.0, 1.0).build();
    unit_plane
        .generate_tangents()
        .expect("the built-in terrain plane must support tangent generation");
    commands.insert_resource(WorldRenderAssets {
        unit_plane: meshes.add(unit_plane),
    });
}

pub(super) fn attach_page(
    commands: &mut Commands,
    asset_server: &AssetServer,
    render_assets: &WorldRenderAssets,
    vegetation_catalog: Option<&VegetationCatalog>,
    _terrain_meshes: &mut Assets<Mesh>,
    terrain_materials: &mut Assets<TerrainMaterial>,
    terrain_images: &mut Assets<Image>,
    macro_variation: TerrainMacroVariation,
    origin_cell: CellCoord,
    cell_size: f32,
    prepared: PreparedPage,
) -> Result<PageAttachment, String> {
    if prepared.height_only {
        return attach_height_source(commands, prepared, cell_size);
    }
    let key = prepared.decoded.key;
    let mut entities = Vec::new();
    #[cfg(target_os = "ios")]
    let owned_terrain_meshes: Vec<Handle<Mesh>> = Vec::new();
    #[cfg(not(target_os = "ios"))]
    let mut owned_terrain_meshes = Vec::new();
    let mut owned_terrain_materials = Vec::new();
    let mut owned_terrain_images = Vec::new();
    let mut gameplay_objects = 0;
    let mut vegetation_pages = 0;
    let mut terrain_texture_set = None;
    match prepared.decoded.payload {
        PagePayload::TerrainRender(terrain) => {
            let resources = prepared
                .terrain
                .as_ref()
                .ok_or_else(|| "terrain page has no fetched render resources".to_owned())?;
            if resources.profile.space != key.space
                || resources.profile.texture_set != resources.texture_set.id
            {
                return Err("terrain page render resources are inconsistent".into());
            }
            terrain_texture_set = Some((
                resources.texture_set.id,
                resources.texture_set.runtime_gpu_bytes(),
            ));
            let surface_lookup: HashMap<_, _> = resources
                .surfaces
                .iter()
                .map(|runtime| (runtime.surface.id, runtime))
                .collect();
            let surface_layers = terrain
                .surfaces
                .iter()
                .map(|surface| {
                    let runtime = surface_lookup.get(surface).ok_or_else(|| {
                        format!("terrain page has unresolved surface {:?}", surface)
                    })?;
                    Ok(TerrainSurfaceLayer {
                        surface: runtime.surface.clone(),
                        layer: runtime.layer,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let center = [
                (i64::from(key.cell.x) - i64::from(origin_cell.x)) as f32 * cell_size
                    + cell_size * 0.5,
                (i64::from(key.cell.z) - i64::from(origin_cell.z)) as f32 * cell_size
                    + cell_size * 0.5,
            ];
            let heightfield = TerrainHeightfield::from_heights(
                2,
                &[terrain.height; 4],
                terrain.height,
                terrain.height,
                cell_size,
            )
            .map_err(|error| error.to_string())?;
            let prepared_material = prepare_terrain_material(PrepareTerrainMaterialContext {
                asset_server,
                images: terrain_images,
                materials: terrain_materials,
                cell: key.cell,
                origin_cell,
                cell_size,
                page_surfaces: &terrain.surfaces,
                weight_pages: &terrain.weight_pages,
                profile: &resources.profile,
                texture_set: &resources.texture_set,
                surfaces: &surface_layers,
                macro_variation,
            })?;
            let entity = commands
                .spawn((
                    Mesh3d(render_assets.unit_plane.clone()),
                    MeshMaterial3d(prepared_material.material.clone()),
                    Transform::from_xyz(center[0], terrain.height, center[1])
                        .with_scale(Vec3::new(cell_size, 1.0, cell_size)),
                    StreamedTerrainSurface {
                        key,
                        cell_size,
                        heightfield,
                    },
                    StreamedPageEntity(key),
                    Name::new(format!("Terrain cell {}, {}", key.cell.x, key.cell.z)),
                ))
                .id();
            entities.push(entity);
            owned_terrain_materials.push(prepared_material.material);
            owned_terrain_images.push(prepared_material.weight_image);
        }
        PagePayload::TerrainHeightfield(terrain) => {
            terrain
                .heightfield
                .validate()
                .map_err(|error| error.to_string())?;
            let resources = prepared
                .terrain
                .as_ref()
                .ok_or_else(|| "terrain page has no fetched render resources".to_owned())?;
            if resources.profile.space != key.space
                || resources.profile.texture_set != resources.texture_set.id
            {
                return Err("terrain page render resources are inconsistent".into());
            }
            terrain_texture_set = Some((
                resources.texture_set.id,
                resources.texture_set.runtime_gpu_bytes(),
            ));
            let surface_lookup: HashMap<_, _> = resources
                .surfaces
                .iter()
                .map(|runtime| (runtime.surface.id, runtime))
                .collect();
            let surface_layers = terrain
                .surfaces
                .iter()
                .map(|surface| {
                    let runtime = surface_lookup.get(surface).ok_or_else(|| {
                        format!("terrain page has unresolved surface {:?}", surface)
                    })?;
                    Ok(TerrainSurfaceLayer {
                        surface: runtime.surface.clone(),
                        layer: runtime.layer,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let center = [
                (i64::from(key.cell.x) - i64::from(origin_cell.x)) as f32 * cell_size
                    + cell_size * 0.5,
                (i64::from(key.cell.z) - i64::from(origin_cell.z)) as f32 * cell_size
                    + cell_size * 0.5,
            ];
            #[cfg(not(target_os = "ios"))]
            let heightfield_mesh = build_heightfield_mesh(&terrain.heightfield, cell_size)?;
            #[cfg(target_os = "ios")]
            let streamed_heightfield =
                TerrainHeightfield::from_heights(2, &[0.0; 4], 0.0, 0.0, cell_size)
                    .map_err(|error| error.to_string())?;
            let prepared_material = prepare_terrain_material(PrepareTerrainMaterialContext {
                asset_server,
                images: terrain_images,
                materials: terrain_materials,
                cell: key.cell,
                origin_cell,
                cell_size,
                page_surfaces: &terrain.surfaces,
                weight_pages: &terrain.weight_pages,
                profile: &resources.profile,
                texture_set: &resources.texture_set,
                surfaces: &surface_layers,
                macro_variation,
            })?;
            #[cfg(target_os = "ios")]
            let (mesh, transform, terrain_name) = (
                render_assets.unit_plane.clone(),
                Transform::from_xyz(center[0], 0.0, center[1])
                    .with_scale(Vec3::new(cell_size, 1.0, cell_size)),
                format!("Flat terrain cell {}, {}", key.cell.x, key.cell.z),
            );
            #[cfg(not(target_os = "ios"))]
            let (mesh, transform, terrain_name) = {
                let mesh = _terrain_meshes.add(heightfield_mesh);
                (
                    mesh,
                    Transform::from_xyz(center[0], 0.0, center[1]),
                    format!("Relief terrain cell {}, {}", key.cell.x, key.cell.z),
                )
            };
            #[cfg(not(target_os = "ios"))]
            let streamed_heightfield = terrain.heightfield;
            let entity = commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(prepared_material.material.clone()),
                    transform,
                    StreamedTerrainSurface {
                        key,
                        cell_size,
                        heightfield: streamed_heightfield,
                    },
                    StreamedPageEntity(key),
                    Name::new(terrain_name),
                ))
                .id();
            entities.push(entity);
            #[cfg(not(target_os = "ios"))]
            owned_terrain_meshes.push(mesh);
            owned_terrain_materials.push(prepared_material.material);
            owned_terrain_images.push(prepared_material.weight_image);
        }
        PagePayload::StaticObjects(objects) => {
            let mut dependencies: HashMap<AssetId, Vec<&PageDependency>> = HashMap::new();
            for dependency in &prepared.dependencies {
                dependencies
                    .entry(dependency.asset)
                    .or_default()
                    .push(dependency);
            }
            for variants in dependencies.values_mut() {
                variants.sort_by_key(|variant| variant.asset_lod);
            }
            let cell_origin = [
                (f64::from(key.cell.x) - f64::from(origin_cell.x)) * f64::from(cell_size),
                (f64::from(key.cell.z) - f64::from(origin_cell.z)) * f64::from(cell_size),
            ];
            // Validate the whole page first: a late error must not orphan earlier entities.
            for instance in &objects.instances {
                let dependencies = dependencies.get(&instance.asset).ok_or_else(|| {
                    format!("object {:?} has no cooked asset dependency", instance.id)
                })?;
                if dependencies.is_empty() {
                    return Err(format!("asset {:?} has no LOD variants", instance.asset));
                }
                let mut previous_minimum = f32::INFINITY;
                for dependency in dependencies {
                    if dependency.kind != "gltf-scene" {
                        return Err(format!(
                            "asset {:?} has unsupported kind {}",
                            dependency.asset, dependency.kind
                        ));
                    }
                    if dependency.minimum_screen_height > previous_minimum {
                        return Err(format!(
                            "asset {:?} LOD thresholds are not descending",
                            dependency.asset
                        ));
                    }
                    previous_minimum = dependency.minimum_screen_height;
                }
            }
            for instance in objects.instances {
                let dependencies = &dependencies[&instance.asset];
                let translation = Vec3::new(
                    cell_origin[0] as f32 + instance.translation[0],
                    instance.translation[1],
                    cell_origin[1] as f32 + instance.translation[2],
                );
                let variants: Vec<_> = dependencies
                    .iter()
                    .map(|dependency| ScreenSpaceLodVariant {
                        lod: dependency.asset_lod,
                        scene: asset_server
                            .load(GltfAssetLabel::Scene(0).from_asset(dependency.uri.clone())),
                        minimum_screen_height: dependency.minimum_screen_height,
                    })
                    .collect();
                let bounds_height = dependencies
                    .iter()
                    .map(|dependency| dependency.bounds[1])
                    .fold(0.0_f32, f32::max);
                let lod = ScreenSpaceLod::new(variants, bounds_height);
                let entity = commands
                    .spawn((
                        lod.scene_root(),
                        Transform::from_translation(translation)
                            .with_rotation(Quat::from_rotation_y(instance.yaw))
                            .with_scale(Vec3::splat(instance.scale)),
                        lod,
                        StreamedVisualObject { id: instance.id },
                        StreamedPageEntity(key),
                        Name::new(format!("Streamed object {:?}", instance.id)),
                    ))
                    .id();
                if instance.generated {
                    commands.entity(entity).insert(GeneratedEnvironmentObject {
                        space: key.space,
                        cell: key.cell,
                    });
                }
                entities.push(entity);
            }
        }
        PagePayload::Vegetation(data) => {
            let catalog = vegetation_catalog
                .ok_or_else(|| "vegetation page has no generation catalog".to_owned())?;
            data.validate(catalog).map_err(|error| error.to_string())?;
            let entity = commands
                .spawn((
                    StreamedVegetationFieldPage {
                        key,
                        cell_size,
                        data,
                    },
                    StreamedPageEntity(key),
                    Name::new(format!("Vegetation fields {}, {}", key.cell.x, key.cell.z)),
                ))
                .id();
            entities.push(entity);
            vegetation_pages = 1;
        }
        PagePayload::ShadowCasters(_) => {
            return Err("shadow-caster page attachment is not enabled in the first slice".into());
        }
        PagePayload::GameplayObjects(objects) => {
            let definitions: HashMap<_, _> = prepared
                .definitions
                .iter()
                .map(|definition| (definition.id, definition))
                .collect();
            let cell_origin = [
                (f64::from(key.cell.x) - f64::from(origin_cell.x)) * f64::from(cell_size),
                (f64::from(key.cell.z) - f64::from(origin_cell.z)) * f64::from(cell_size),
            ];
            // Validate the whole page first: a late error must not orphan earlier entities.
            for instance in &objects.instances {
                let definition = definitions.get(&instance.definition).ok_or_else(|| {
                    format!(
                        "gameplay object {:?} has no fetched definition {:?}",
                        instance.id, instance.definition
                    )
                })?;
                if definition.activation != ObjectActivationPolicy::Proximity {
                    return Err(format!(
                        "gameplay page contains render-only definition {}",
                        definition.key
                    ));
                }
            }
            for instance in objects.instances {
                let definition = &definitions[&instance.definition];
                let translation = Vec3::new(
                    cell_origin[0] as f32 + instance.translation[0],
                    instance.translation[1],
                    cell_origin[1] as f32 + instance.translation[2],
                );
                let entity = commands
                    .spawn((
                        Transform::from_translation(translation)
                            .with_rotation(Quat::from_rotation_y(instance.yaw))
                            .with_scale(Vec3::splat(instance.scale)),
                        GameplayObject {
                            id: instance.id,
                            definition: instance.definition,
                        },
                        StreamedPageEntity(key),
                        Name::new(definition.display_name.clone()),
                    ))
                    .id();
                entities.push(entity);
                gameplay_objects += 1;
            }
        }
    }

    Ok(PageAttachment {
        entities,
        owned_terrain_meshes,
        owned_terrain_materials,
        owned_terrain_images,
        decoded_bytes: prepared.decoded.decoded_bytes,
        gpu_bytes_estimate: prepared.decoded.gpu_bytes_estimate
            + prepared
                .dependencies
                .iter()
                .map(|dependency| dependency.gpu_bytes_estimate)
                .sum::<u64>(),
        gameplay_objects,
        vegetation_pages,
        height_only_pages: 0,
        terrain_texture_set,
    })
}

/// Keep CPU relief and surface inputs; GPU near shading admits its own bounded subset.
pub(super) fn attach_height_source(
    commands: &mut Commands,
    page: PreparedPage,
    cell_size: f32,
) -> Result<PageAttachment, String> {
    let key = page.decoded.key;
    let (heightfield, surfaces, weights) = match page.decoded.payload {
        PagePayload::TerrainHeightfield(t) => (t.heightfield, t.surfaces, t.weight_pages),
        PagePayload::TerrainRender(t) => (
            TerrainHeightfield::from_heights(2, &[t.height; 4], t.height, t.height, cell_size)
                .map_err(|e| e.to_string())?,
            t.surfaces,
            t.weight_pages,
        ),
        _ => return Err("height-only request returned a non-terrain page".into()),
    };
    heightfield.validate().map_err(|e| e.to_string())?;
    let bounds = [
        heightfield
            .heights
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min),
        heightfield
            .heights
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max),
    ];
    let near = page
        .terrain
        .map(|resources| {
            let layers = surfaces
                .iter()
                .map(|id| {
                    let r = resources
                        .surfaces
                        .iter()
                        .find(|s| s.surface.id == *id)
                        .ok_or("unresolved near terrain surface")?;
                    Ok(TerrainSurfaceLayer {
                        surface: r.surface.clone(),
                        layer: r.layer,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok::<_, String>(terrain_render::near::NearSource {
                key,
                cell_size,
                height_bounds: bounds,
                surfaces,
                weights,
                profile: resources.profile,
                texture_set: resources.texture_set,
                layers,
            })
        })
        .transpose()?;
    let entity = commands
        .spawn((
            StreamedTerrainSurface {
                key,
                cell_size,
                heightfield,
            },
            StreamedPageEntity(key),
            Name::new(format!("Terrain source {}, {}", key.cell.x, key.cell.z)),
        ))
        .id();
    if let Some(near) = near {
        commands.entity(entity).insert(near);
    }
    Ok(PageAttachment {
        entities: vec![entity],
        decoded_bytes: page.decoded.decoded_bytes,
        height_only_pages: 1,
        ..default()
    })
}

pub(super) fn despawn_attachment(
    commands: &mut Commands,
    terrain_meshes: &mut Assets<Mesh>,
    terrain_materials: &mut Assets<TerrainMaterial>,
    terrain_images: &mut Assets<Image>,
    attachment: PageAttachment,
) {
    for entity in attachment.entities {
        commands.entity(entity).despawn();
    }
    for mesh in attachment.owned_terrain_meshes {
        terrain_meshes.remove(mesh.id());
    }
    for material in attachment.owned_terrain_materials {
        terrain_materials.remove(material.id());
    }
    for image in attachment.owned_terrain_images {
        terrain_images.remove(image.id());
    }
}
