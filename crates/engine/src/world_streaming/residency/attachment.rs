//! Translate source payloads into ECS entities.
use super::PreparedPage;
use crate::object_lod::{ObjectFootprint, ScreenSpaceLod, ScreenSpaceLodVariant};
use crate::world_streaming::{
    GameplayObject, GeneratedEnvironmentObject, StreamedPageEntity, StreamedTerrainSurface,
    StreamedVegetationFieldPage, StreamedVisualObject,
};
use bevy::prelude::*;
use std::collections::HashMap;
use terrain_render::TerrainSurfaceLayer;
use vegetation::VegetationCatalog;
use world::{AssetId, CellCoord, ObjectActivationPolicy, PageKey, PagePayload};
use world_db::{PageDependency, RuntimeObjectDefinition};

#[derive(Default)]
pub(in crate::world_streaming) struct PageAttachment {
    pub(in crate::world_streaming) entities: Vec<Entity>,
    pub(in crate::world_streaming) decoded_bytes: u64,
    /// The page's own GPU bytes; shared assets are in `asset_variants`.
    pub(in crate::world_streaming) gpu_bytes_estimate: u64,
    /// Asset variants the page draws, with their GPU bytes. Pages share them, so the
    /// residency counts each variant once however many pages use it.
    pub(in crate::world_streaming) asset_variants: Vec<(AssetVariantKey, u64)>,
    pub(in crate::world_streaming) gameplay_objects: usize,
    pub(in crate::world_streaming) vegetation_pages: usize,
    pub(in crate::world_streaming) terrain_source_pages: usize,
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

/// The page, and where it goes.
pub(super) fn attach_page(
    commands: &mut Commands,
    asset_server: &AssetServer,
    vegetation_catalog: Option<&VegetationCatalog>,
    origin_cell: CellCoord,
    cell_size: f32,
    prepared: PreparedPage,
) -> Result<PageAttachment, String> {
    let key = prepared.decoded.key;
    let mut attachment = PageAttachment {
        decoded_bytes: prepared.decoded.decoded_bytes,
        gpu_bytes_estimate: prepared.decoded.gpu_bytes_estimate,
        asset_variants: page_asset_variants(&prepared.dependencies),
        ..default()
    };
    match prepared.decoded.payload {
        PagePayload::TerrainHeightfield(terrain) => attach_terrain_source(
            &mut attachment,
            commands,
            key,
            terrain,
            prepared.terrain,
            cell_size,
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

/// A terrain cell's CPU relief and surface inputs; the hierarchy draws the ground, and GPU
/// near shading admits its own bounded subset of these sources.
pub(super) fn attach_terrain_source(
    attachment: &mut PageAttachment,
    commands: &mut Commands,
    key: PageKey,
    terrain: world::TerrainHeightfieldPage,
    resources: Option<world_db::TerrainRenderResources>,
    cell_size: f32,
) -> Result<(), String> {
    let world::TerrainHeightfieldPage {
        heightfield,
        surfaces,
        weight_pages: weights,
    } = terrain;
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
    let near = resources
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
    attachment.entities.push(entity);
    attachment.terrain_source_pages = 1;
    Ok(())
}

pub(super) fn despawn_attachment(commands: &mut Commands, attachment: PageAttachment) {
    for entity in attachment.entities {
        commands.entity(entity).despawn();
    }
}
