//! Translate source payloads into ECS entities and explicitly owned terrain assets.
use super::PreparedPage;
use crate::object_lod::{ObjectFootprint, ScreenSpaceLod, ScreenSpaceLodVariant};
use crate::world_streaming::{
    GameplayObject, GeneratedEnvironmentObject, StreamedPageEntity, StreamedTerrainSurface,
    StreamedVegetationFieldPage, StreamedVisualObject,
};
use bevy::prelude::*;
use std::collections::HashMap;
#[cfg(not(target_os = "ios"))]
use terrain_render::build_heightfield_mesh;
use terrain_render::{
    PrepareTerrainMaterialContext, TerrainMacroVariation, TerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use vegetation::VegetationCatalog;
use world::{
    AssetId, CellCoord, ObjectActivationPolicy, PageKey, PagePayload, TerrainTextureSetId,
};
use world_db::{PageDependency, RuntimeObjectDefinition};

#[derive(Default)]
pub(in crate::world_streaming) struct PageAttachment {
    pub(in crate::world_streaming) entities: Vec<Entity>,
    pub(in crate::world_streaming) owned_terrain_meshes: Vec<Handle<Mesh>>,
    pub(in crate::world_streaming) owned_terrain_materials: Vec<Handle<TerrainMaterial>>,
    pub(in crate::world_streaming) owned_terrain_images: Vec<Handle<Image>>,
    pub(in crate::world_streaming) decoded_bytes: u64,
    /// The page's own GPU bytes; shared assets are in `asset_variants`.
    pub(in crate::world_streaming) gpu_bytes_estimate: u64,
    /// Asset variants the page draws, with their GPU bytes. Pages share them, so the
    /// residency counts each variant once however many pages use it.
    pub(in crate::world_streaming) asset_variants: Vec<(AssetVariantKey, u64)>,
    pub(in crate::world_streaming) gameplay_objects: usize,
    pub(in crate::world_streaming) vegetation_pages: usize,
    pub(in crate::world_streaming) height_only_pages: usize,
    pub(in crate::world_streaming) terrain_texture_set: Option<(TerrainTextureSetId, u64)>,
}

pub(in crate::world_streaming) type AssetVariantKey = (world::AssetId, u8);

/// The asset variants a page's attachment loads. Impostors are not among them: far-object
/// blocks draw those, and their textures are not part of a page's residency.
pub(in crate::world_streaming) fn page_asset_variants(
    dependencies: &[PageDependency],
) -> Vec<(AssetVariantKey, u64)> {
    dependencies
        .iter()
        .filter(|d| !world::is_impostor_uri(&d.uri))
        .map(|d| ((d.asset, d.asset_lod), d.gpu_bytes_estimate))
        .collect()
}

#[derive(Resource)]
pub(in crate::world_streaming) struct WorldRenderAssets {
    /// iOS draws terrain cells flat on this plane.
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
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

#[allow(clippy::too_many_arguments)] // The page, where it goes, and the assets terrain owns.
pub(super) fn attach_page(
    commands: &mut Commands,
    asset_server: &AssetServer,
    render_assets: &WorldRenderAssets,
    vegetation_catalog: Option<&VegetationCatalog>,
    terrain_meshes: &mut Assets<Mesh>,
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
    let mut attachment = PageAttachment {
        decoded_bytes: prepared.decoded.decoded_bytes,
        gpu_bytes_estimate: prepared.decoded.gpu_bytes_estimate,
        asset_variants: page_asset_variants(&prepared.dependencies),
        ..default()
    };
    match prepared.decoded.payload {
        PagePayload::TerrainHeightfield(terrain) => attach_terrain(
            &mut attachment,
            commands,
            asset_server,
            render_assets,
            (terrain_meshes, terrain_materials, terrain_images),
            macro_variation,
            origin_cell,
            cell_size,
            key,
            terrain,
            prepared.terrain.as_ref(),
        )?,
        PagePayload::StaticObjects(objects) => attach_static_objects(
            &mut attachment,
            commands,
            asset_server,
            key.cell.offset_from(origin_cell, cell_size),
            key,
            objects,
            &prepared.dependencies,
        )?,
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
            attachment.entities.push(entity);
            attachment.vegetation_pages = 1;
        }
        PagePayload::GameplayObjects(objects) => attach_gameplay_objects(
            &mut attachment,
            commands,
            key.cell.offset_from(origin_cell, cell_size),
            key,
            objects,
            &prepared.definitions,
        )?,
    }
    Ok(attachment)
}

/// A terrain cell drawn as its own relief mesh (flat on iOS), with its CPU heightfield.
#[allow(clippy::too_many_arguments)]
fn attach_terrain(
    attachment: &mut PageAttachment,
    commands: &mut Commands,
    asset_server: &AssetServer,
    _render_assets: &WorldRenderAssets,
    (_terrain_meshes, terrain_materials, terrain_images): (
        &mut Assets<Mesh>,
        &mut Assets<TerrainMaterial>,
        &mut Assets<Image>,
    ),
    macro_variation: TerrainMacroVariation,
    origin_cell: CellCoord,
    cell_size: f32,
    key: PageKey,
    terrain: world::TerrainHeightfieldPage,
    resources: Option<&world_db::TerrainRenderResources>,
) -> Result<(), String> {
    terrain
        .heightfield
        .validate()
        .map_err(|error| error.to_string())?;
    let resources =
        resources.ok_or_else(|| "terrain page has no fetched render resources".to_owned())?;
    if resources.profile.space != key.space
        || resources.profile.texture_set != resources.texture_set.id
    {
        return Err("terrain page render resources are inconsistent".into());
    }
    attachment.terrain_texture_set = Some((
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
            let runtime = surface_lookup
                .get(surface)
                .ok_or_else(|| format!("terrain page has unresolved surface {:?}", surface))?;
            Ok(TerrainSurfaceLayer {
                surface: runtime.surface.clone(),
                layer: runtime.layer,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let [x, z] = key.cell.offset_from(origin_cell, cell_size);
    let center = [x as f32 + cell_size * 0.5, z as f32 + cell_size * 0.5];
    #[cfg(not(target_os = "ios"))]
    let heightfield_mesh = build_heightfield_mesh(&terrain.heightfield, cell_size)?;
    #[cfg(target_os = "ios")]
    let streamed_heightfield =
        world::TerrainHeightfield::from_heights(2, &[0.0; 4], 0.0, 0.0, cell_size)
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
        _render_assets.unit_plane.clone(),
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
    attachment.entities.push(entity);
    #[cfg(not(target_os = "ios"))]
    attachment.owned_terrain_meshes.push(mesh);
    attachment
        .owned_terrain_materials
        .push(prepared_material.material);
    attachment
        .owned_terrain_images
        .push(prepared_material.weight_image);
    Ok(())
}

/// Placed objects, each a screen-space LOD whose mesh scenes load as it nears.
fn attach_static_objects(
    attachment: &mut PageAttachment,
    commands: &mut Commands,
    asset_server: &AssetServer,
    cell_origin: [f64; 2],
    key: PageKey,
    objects: world::StaticObjectsPage,
    dependencies_of_page: &[PageDependency],
) -> Result<(), String> {
    let mut dependencies: HashMap<AssetId, Vec<&PageDependency>> = HashMap::new();
    for dependency in dependencies_of_page {
        dependencies
            .entry(dependency.asset)
            .or_default()
            .push(dependency);
    }
    for variants in dependencies.values_mut() {
        variants.sort_by_key(|variant| variant.asset_lod);
    }
    // Validate the whole page first: a late error must not orphan earlier entities.
    for instance in &objects.instances {
        let dependencies = dependencies
            .get(&instance.asset)
            .ok_or_else(|| format!("object {:?} has no cooked asset dependency", instance.id))?;
        if dependencies.is_empty() {
            return Err(format!("asset {:?} has no LOD variants", instance.asset));
        }
        let mut previous_minimum = f32::INFINITY;
        // Only the last LOD may be an impostor, and a mesh must come before it.
        for (i, dependency) in dependencies.iter().enumerate() {
            if world::is_impostor_uri(&dependency.uri) && (i == 0 || i + 1 != dependencies.len()) {
                return Err(format!(
                    "asset {:?} has an impostor that is not its last LOD after a mesh",
                    dependency.asset
                ));
            }
        }
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
    // Impostor LODs have no scene here: far-object blocks draw them.
    for instance in objects.instances {
        let dependencies = &dependencies[&instance.asset];
        let translation = Vec3::new(
            cell_origin[0] as f32 + instance.translation[0],
            instance.translation[1],
            cell_origin[1] as f32 + instance.translation[2],
        );
        let variants: Vec<_> = dependencies
            .iter()
            .map(|dependency| {
                ScreenSpaceLodVariant::load(
                    asset_server,
                    dependency.asset_lod,
                    &dependency.uri,
                    dependency.minimum_screen_height,
                )
            })
            .collect();
        let bounds = dependencies.iter().fold([0.0_f32; 3], |b, dependency| {
            std::array::from_fn(|i| b[i].max(dependency.bounds[i]))
        });
        let bounds_height = bounds[1];
        let lod = ScreenSpaceLod::new(variants, bounds_height);
        let mut object = commands.spawn((
            Transform::from_translation(translation)
                .with_rotation(Quat::from_rotation_y(instance.yaw))
                .with_scale(Vec3::splat(instance.scale)),
            Visibility::default(),
            StreamedVisualObject { id: instance.id },
            ObjectFootprint {
                half_extent: Vec2::new(bounds[0], bounds[2]) * 0.5,
                height: bounds_height,
            },
            StreamedPageEntity(key),
            Name::new(format!("Streamed object {:?}", instance.id)),
        ));
        lod.spawn_scenes(&mut object);
        let entity = object.insert(lod).id();
        if instance.generated {
            commands.entity(entity).insert(GeneratedEnvironmentObject {
                space: key.space,
                cell: key.cell,
            });
        }
        attachment.entities.push(entity);
    }
    Ok(())
}

/// Gameplay objects of proximity-activated definitions.
fn attach_gameplay_objects(
    attachment: &mut PageAttachment,
    commands: &mut Commands,
    cell_origin: [f64; 2],
    key: PageKey,
    objects: world::GameplayObjectsPage,
    definitions: &[RuntimeObjectDefinition],
) -> Result<(), String> {
    let definitions: HashMap<_, _> = definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect();
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
        attachment.entities.push(entity);
        attachment.gameplay_objects += 1;
    }
    Ok(())
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
