//! Cached source coverage for the editor's streamed world, using the game's baker.
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use engine::{StreamedTerrainSurface, WorldCatalog, WorldOrigin};
use std::collections::HashMap;
use terrain_render::TerrainMaterial;
use vegetation::{VegetationCatalog, VegetationFieldPage};
use vegetation_render::{VegetationDebugScene, VegetationLighting, canopy_coverage};

#[derive(Resource, Default)]
pub(super) struct GroundTiles {
    entries: HashMap<Entity, Tile>,
    catalog: Option<VegetationCatalog>,
    revision: Option<u64>,
    coordinate_frame: Option<(Option<world::WorldSpaceId>, world::CellCoord)>,
    settle_until: f64,
}

struct Tile {
    material: Handle<TerrainMaterial>,
    origin: [f32; 2],
    extent: f32,
    sources: Vec<VegetationFieldPage>,
    image: Option<Handle<Image>>,
    bounds: Vec4,
    task: Option<Task<canopy_coverage::Bake>>,
    needs_bake: bool,
    applied: Option<([[f32; 4]; 4], Option<AssetId<Image>>, Vec4)>,
}

impl Tile {
    fn invalidate(&mut self, images: &mut Assets<Image>) {
        self.task = None;
        self.needs_bake = !self.sources.is_empty();
        if let Some(image) = self.image.take() {
            images.remove(image.id());
        }
        self.bounds = Vec4::ZERO;
    }

    fn clear(&mut self, images: &mut Assets<Image>, materials: &mut Assets<TerrainMaterial>) {
        self.invalidate(images);
        if let Some(mut material) = materials.get_mut(&self.material) {
            material.set_canopy_coverage(None, Vec4::ZERO);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync(
    mut lighting: ResMut<VegetationLighting>,
    scene: Res<VegetationDebugScene>,
    world_catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    time: Res<Time>,
    terrain: Query<(
        Entity,
        &StreamedTerrainSurface,
        &MeshMaterial3d<TerrainMaterial>,
    )>,
    mut tiles: ResMut<GroundTiles>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let offset = origin
        .space()
        .and_then(|id| world_catalog.world_space(id))
        .map_or([0.0; 2], |space| {
            origin.cell().origin(space.cell_size).map(|v| v as f32)
        });
    if lighting.canopy_origin != offset {
        lighting.canopy_origin = offset;
    }
    let source = scene.scene();
    let frame = (origin.space(), origin.cell());
    if tiles.coordinate_frame != Some(frame) || tiles.catalog.as_ref() != Some(&source.catalog) {
        for tile in tiles.entries.values_mut() {
            tile.clear(&mut images, &mut materials);
        }
        tiles.entries.clear();
        tiles.coordinate_frame = Some(frame);
        tiles.catalog = Some(source.catalog.clone());
        tiles.revision = None;
    }
    tiles.entries.retain(|entity, tile| {
        let keep = terrain.get(*entity).is_ok_and(|(_, surface, material)| {
            Some(surface.key.space) == origin.space() && material.0 == tile.material
        });
        if !keep {
            tile.clear(&mut images, &mut materials);
        }
        keep
    });
    let source_changed = tiles.revision != Some(scene.revision());
    if source_changed {
        tiles.revision = Some(scene.revision());
        tiles.settle_until = time.elapsed_secs_f64() + 0.35;
    }

    for (entity, surface, material) in &terrain {
        if Some(surface.key.space) != origin.space() {
            continue;
        }
        let cell_min = surface.key.cell.origin(surface.cell_size);
        let base = origin.cell().origin(surface.cell_size);
        let min = [
            (cell_min[0] - base[0]) as f32,
            (cell_min[1] - base[1]) as f32,
        ];
        let new = !tiles.entries.contains_key(&entity);
        let tile = tiles.entries.entry(entity).or_insert_with(|| Tile {
            material: material.0.clone(),
            origin: min,
            extent: surface.cell_size,
            sources: Vec::new(),
            image: None,
            bounds: Vec4::ZERO,
            task: None,
            needs_bake: false,
            applied: None,
        });
        if new || source_changed {
            // The border includes neighbor roots so coverage agrees across terrain tiles.
            let neighbors: Vec<_> = source
                .pages
                .iter()
                .filter(|page| {
                    (0..2).all(|i| {
                        page.origin_xz[i]
                            < min[i]
                                + surface.cell_size
                                + vegetation_render::canopy_coverage::MARGIN
                            && page.origin_xz[i] + page.size
                                > min[i] - vegetation_render::canopy_coverage::MARGIN
                    })
                })
                .collect();
            if !tile.sources.iter().eq(neighbors.iter().copied()) {
                tile.sources = neighbors.into_iter().cloned().collect();
                tile.invalidate(&mut images);
            }
        }
    }

    let packed = lighting.canopy.packed(offset);
    for tile in tiles.entries.values_mut() {
        if let Some(bake) = tile.task.as_mut().and_then(check_ready) {
            tile.task = None;
            tile.needs_bake = false;
            tile.bounds = bake.bounds;
            tile.image = Some(images.add(bake.image));
        }
        let image = if lighting.canopy.enabled {
            tile.image.clone()
        } else {
            None
        };
        let applied = (packed, image.as_ref().map(Handle::id), tile.bounds);
        if tile.applied.as_ref() != Some(&applied)
            && let Some(mut material) = materials.get_mut(&tile.material)
        {
            material.set_canopy_shading(packed);
            material.set_canopy_coverage(image, tile.bounds);
            tile.applied = Some(applied);
        }
    }
    if !lighting.canopy.enabled || time.elapsed_secs_f64() < tiles.settle_until {
        return;
    }
    let active = tiles
        .entries
        .values()
        .filter(|tile| tile.task.is_some())
        .count();
    let mut pending: Vec<_> = tiles
        .entries
        .iter()
        .filter(|(_, tile)| tile.needs_bake && tile.task.is_none())
        .map(|(entity, tile)| {
            (
                *entity,
                (tile.origin[0] + tile.extent * 0.5).powi(2)
                    + (tile.origin[1] + tile.extent * 0.5).powi(2),
            )
        })
        .collect();
    pending.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (entity, _) in pending.into_iter().take(2usize.saturating_sub(active)) {
        let tile = tiles.entries.get_mut(&entity).unwrap();
        let neighbors = tile.sources.clone();
        let catalog = source.catalog.clone();
        let (min, size) = (tile.origin, tile.extent);
        tile.task = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { canopy_coverage::bake(&catalog, &neighbors, min, size) }),
        );
    }
}
