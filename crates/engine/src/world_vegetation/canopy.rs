//! Bounded ground-canopy cache shared by published gameplay and accepted editor previews.
use crate::{StreamedTerrainSurface, WorldCatalog, WorldOrigin};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use std::collections::HashMap;
use terrain_render::TerrainMaterial;
use vegetation_render::{VegetationLighting, VegetationSceneState, canopy_coverage as canopy};

/// Requires a scene producer, world origin/catalog, Time and image/terrain material assets.
/// Order the producer before GroundCanopySystems. Disabling shading cancels pending jobs,
/// detaches textures and retains valid resident masks for reuse. Removal/frame/source changes
/// retire affected masks before polling jobs, so a completed old job cannot restore stale data.
pub struct GroundCanopyPlugin;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GroundCanopySystems;

impl Plugin for GroundCanopyPlugin {
    fn build(&self, app: &mut App) {
        // Apply masks in Update so PostUpdate's TerrainMaterialPreparation consumes them
        // in the same frame. Cross-schedule `.before` constraints do not establish ordering.
        app.init_resource::<GroundCanopyTiles>()
            .add_systems(Update, sync.in_set(GroundCanopySystems));
    }
}

#[derive(Default, Resource)]
pub struct GroundCanopyTiles {
    entries: HashMap<Entity, Tile>,
    // One immutable snapshot retains the previous sources for exact dependency comparison.
    // Tiles store indices, not a second deep copy of every overlapping page.
    source: Option<VegetationSceneState>,
    frame: Option<(Option<world::WorldSpaceId>, world::CellCoord)>,
    settle_until: f64,
    enabled: bool,
}

impl GroundCanopyTiles {
    /// Ready masks, resident tiles, and active jobs for optional UI diagnostics.
    pub fn counts(&self) -> (usize, usize, usize) {
        (
            self.entries
                .values()
                .filter(|t| t.image.is_some() && !t.needs_bake)
                .count(),
            self.entries.len(),
            self.entries.values().filter(|t| t.task.is_some()).count(),
        )
    }

    fn clear(&mut self, images: &mut Assets<Image>, materials: &mut Assets<TerrainMaterial>) {
        for tile in self.entries.values_mut() {
            tile.invalidate(images, materials);
        }
        self.entries.clear();
        self.source = None;
    }
}

struct Tile {
    key: world::PageKey,
    material: Handle<TerrainMaterial>,
    source_indices: Vec<usize>,
    origin: [f32; 2],
    extent: f32,
    image: Option<Handle<Image>>,
    bounds: Vec4,
    task: Option<Task<canopy::Bake>>,
    needs_bake: bool,
    applied: Option<AppliedCanopy>,
}

#[derive(PartialEq)]
struct AppliedCanopy {
    shading: [[f32; 4]; 4],
    coverage: Option<AssetId<Image>>,
    bounds: Vec4,
}

impl Tile {
    fn invalidate(&mut self, images: &mut Assets<Image>, materials: &mut Assets<TerrainMaterial>) {
        self.task = None;
        self.needs_bake = !self.source_indices.is_empty();
        if let Some(image) = self.image.take() {
            images.remove(image.id());
        }
        self.bounds = Vec4::ZERO;
        self.applied = None;
        if let Some(mut material) = materials.get_mut(&self.material) {
            material.set_canopy_coverage(None, Vec4::ZERO);
        }
    }
}

#[allow(clippy::too_many_arguments)] // Independent source, cache and asset ownership.
fn sync(
    mut lighting: ResMut<VegetationLighting>,
    scene: Res<VegetationSceneState>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    time: Res<Time>,
    terrain: Query<(
        Entity,
        &StreamedTerrainSurface,
        &MeshMaterial3d<TerrainMaterial>,
    )>,
    mut tiles: ResMut<GroundCanopyTiles>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let offset = origin
        .space()
        .and_then(|id| catalog.world_space(id))
        .map_or([0.; 2], |space| {
            origin.cell().origin(space.cell_size).map(|v| v as f32)
        });
    if lighting.canopy_origin != offset {
        lighting.canopy_origin = offset;
    }
    let frame = (origin.space(), origin.cell());
    if tiles.frame != Some(frame) {
        tiles.clear(&mut images, &mut materials);
        tiles.frame = Some(frame);
    }
    tiles.entries.retain(|entity, tile| {
        let keep = terrain.get(*entity).is_ok_and(|(_, surface, material)| {
            Some(surface.key.space) == origin.space()
                && surface.key == tile.key
                && surface.cell_size == tile.extent
                && material.0 == tile.material
        });
        if !keep {
            tile.invalidate(&mut images, &mut materials);
        }
        keep
    });
    let enabled = lighting.canopy.enabled
        && lighting.canopy.strength > 0.0
        && lighting.canopy.ground_amount > 0.0;
    if !enabled {
        if tiles.enabled {
            for tile in tiles.entries.values_mut() {
                if tile.task.take().is_some() {
                    tile.needs_bake = true;
                }
                tile.applied = None;
                if let Some(mut material) = materials.get_mut(&tile.material) {
                    material.set_canopy_coverage(None, Vec4::ZERO);
                }
            }
        }
        tiles.enabled = false;
        return;
    }
    tiles.enabled = true;
    if tiles
        .source
        .as_ref()
        .is_some_and(|old| old.scene().catalog != scene.scene().catalog)
    {
        tiles.clear(&mut images, &mut materials);
    }
    let source_changed =
        tiles.source.as_ref().map(VegetationSceneState::revision) != Some(scene.revision());
    if source_changed {
        tiles.settle_until = time.elapsed_secs_f64() + 0.35;
    }
    let previous = tiles.source.clone();
    for (entity, surface, material) in &terrain {
        if Some(surface.key.space) != origin.space() {
            continue;
        }
        let new = !tiles.entries.contains_key(&entity);
        if !new && !source_changed {
            continue;
        }
        let cell = surface.key.cell.origin(surface.cell_size);
        let base = origin.cell().origin(surface.cell_size);
        let min = [(cell[0] - base[0]) as f32, (cell[1] - base[1]) as f32];
        let tile = tiles.entries.entry(entity).or_insert_with(|| Tile {
            key: surface.key,
            material: material.0.clone(),
            source_indices: Vec::new(),
            origin: min,
            extent: surface.cell_size,
            image: None,
            bounds: Vec4::ZERO,
            task: None,
            needs_bake: false,
            applied: None,
        });
        let indices: Vec<_> = scene
            .scene()
            .pages
            .iter()
            .enumerate()
            .filter(|(_, page)| {
                (0..2).all(|i| {
                    page.origin_xz[i] < min[i] + surface.cell_size + canopy::MARGIN
                        && page.origin_xz[i] + page.size > min[i] - canopy::MARGIN
                })
            })
            .map(|(i, _)| i)
            .collect();
        // Includes relief, coverage and flow changes, even when streamed entity IDs survive.
        // Unrelated page additions/removals may shift indices but do not invalidate this mask.
        let unchanged = previous.as_ref().is_some_and(|old| {
            tile.source_indices
                .iter()
                .map(|&i| &old.scene().pages[i])
                .eq(indices.iter().map(|&i| &scene.scene().pages[i]))
        });
        tile.source_indices = indices;
        if !unchanged {
            tile.invalidate(&mut images, &mut materials);
        }
    }
    if source_changed {
        tiles.source = Some(scene.clone());
    }
    let packed = lighting.canopy.packed(offset);
    for tile in tiles.entries.values_mut() {
        if let Some(bake) = tile.task.as_mut().and_then(check_ready) {
            tile.task = None;
            tile.needs_bake = false;
            tile.bounds = bake.bounds;
            tile.image = Some(images.add(bake.image));
        }
        let applied = AppliedCanopy {
            shading: packed,
            coverage: tile.image.as_ref().map(Handle::id),
            bounds: tile.bounds,
        };
        if tile.applied.as_ref() != Some(&applied)
            && let Some(mut material) = materials.get_mut(&tile.material)
        {
            material.set_canopy_shading(packed);
            material.set_canopy_coverage(tile.image.clone(), tile.bounds);
            tile.applied = Some(applied);
        }
    }
    if time.elapsed_secs_f64() < tiles.settle_until {
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
        let neighbors: Vec<_> = tile
            .source_indices
            .iter()
            .map(|&i| scene.scene().pages[i].clone())
            .collect();
        let catalog = scene.scene().catalog.clone();
        let (min, size) = (tile.origin, tile.extent);
        tile.task = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { canopy::bake(&catalog, &neighbors, min, size) }),
        );
    }
}

#[cfg(test)]
mod tests;
