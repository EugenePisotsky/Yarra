//! Production joining of streamed grass fields and terrain, and the render-space detail focus.
//! Editor source selection stays in the editor; terrain conversion is shared.

use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use terrain_render::near::{NearGrassCover, NearSource};
use vegetation::{
    VegetationCatalog, VegetationFieldPage, VegetationFieldPageData, VegetationScene,
    VegetationSurfaceField,
};
use vegetation_render::{
    GrassBender, GrassBenders, MAX_GRASS_BENDERS, VegetationLodFocus, VegetationRenderPlugin,
    VegetationSceneState,
};
use world::{CellCoord, PageKey, WorldSpaceId};

use crate::{
    StreamedTerrainSurface, StreamedVegetationFieldPage, WorldCatalog, WorldOrigin,
    WorldStreamingSystems, WorldViewpoint,
};

/// Installs the renderer, joins published resident sources and updates grass focus. Requires
/// world streaming and Time.
///
/// Render isolation (including Grass Disabled) does not suspend source joining: contact
/// consumers must see the current world/origin even while vegetation draws are disabled.
/// The editor uses its own scene producer and shares page conversion.
pub struct WorldVegetationPlugin;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldVegetationSystems;

/// Grass looks to draw instead of the published ones, matched by type key: a grass pack being
/// edited while the game runs. Which types grow where stays as published.
#[derive(Resource, Default)]
pub struct GrassLooks(pub Option<VegetationCatalog>);

impl Plugin for WorldVegetationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            VegetationRenderPlugin,
            crate::ground_details::GroundDetailsPlugin,
        ))
        .insert_resource(
            VegetationSceneState::new(VegetationScene {
                catalog: Default::default(),
                pages: Vec::new(),
            })
            .expect("empty vegetation scene is valid"),
        )
        .init_resource::<GrassLooks>()
        .add_systems(
            Update,
            (sync_scene, sync_near_grass_cover)
                .in_set(WorldVegetationSystems)
                .after(WorldStreamingSystems),
        )
        .add_systems(
            PostUpdate,
            (update_lod_focus, list_grass_benders).after(TransformSystems::Propagate),
        );
    }
}

/// Radius of the grass a character presses down around its feet, in metres.
const BENDER_RADIUS: f32 = 0.7;

/// Characters press the grass under them: the ones nearest the detail focus (the player).
fn list_grass_benders(
    time: Res<Time>,
    focus: Res<VegetationLodFocus>,
    actors: Query<(&GlobalTransform, &crate::actor::CharacterMotion)>,
    mut benders: ResMut<GrassBenders>,
) {
    benders.delta_seconds = time.delta_secs();
    benders.benders.clear();
    benders
        .benders
        .extend(actors.iter().map(|(transform, motion)| GrassBender {
            position: transform.translation(),
            velocity: motion.displacement.normalize_or_zero() * motion.speed_mps,
            radius: BENDER_RADIUS,
        }));
    if benders.benders.len() > MAX_GRASS_BENDERS {
        let centre = focus.position.unwrap_or(Vec3::ZERO);
        benders.benders.sort_by(|a, b| {
            a.position
                .distance_squared(centre)
                .total_cmp(&b.position.distance_squared(centre))
        });
        benders.benders.truncate(MAX_GRASS_BENDERS);
    }
}

impl StreamedTerrainSurface {
    /// Terrain and vegetation use different page domains but share a cell and LOD.
    pub fn matches_vegetation(&self, fields: &StreamedVegetationFieldPage) -> bool {
        cell_key(self.key) == cell_key(fields.key) && self.cell_size == fields.cell_size
    }

    /// Construct render-relative vegetation using the authoritative terrain relief.
    /// `data` may be published fields or the editor's accepted unsaved overlay.
    pub fn vegetation_page(
        &self,
        origin: CellCoord,
        data: &VegetationFieldPageData,
    ) -> VegetationFieldPage {
        let cell = self.key.cell.origin(self.cell_size);
        let base = origin.origin(self.cell_size);
        let resolution = self.heightfield.resolution;
        let samples = usize::from(resolution).pow(2);
        VegetationFieldPage::from_data(
            [(cell[0] - base[0]) as f32, (cell[1] - base[1]) as f32],
            self.cell_size,
            VegetationSurfaceField {
                resolution,
                heights: (0..samples)
                    .map(|i| {
                        self.heightfield
                            .height_at(i % usize::from(resolution), i / usize::from(resolution))
                    })
                    .collect(),
                normals_oct: self.heightfield.normals_oct.clone(),
                validity: vec![u8::MAX; samples],
            },
            data.clone(),
        )
    }
}

/// How grass covers a page's ground for near terrain, from its grass fields; `None` without grass.
/// Each cover gets a new revision, so a resident near page uploads it again.
pub fn near_grass_cover(
    data: &VegetationFieldPageData,
    catalog: &VegetationCatalog,
) -> Option<NearGrassCover> {
    static REVISION: AtomicU64 = AtomicU64::new(1);
    let (resolution, rgba) = data.cover_map(catalog)?;
    Some(NearGrassCover {
        revision: REVISION.fetch_add(1, Ordering::Relaxed),
        resolution,
        rgba,
    })
}

/// Near terrain shades the ground under grass where blades grow: each near page takes the cover
/// of its cell's grass fields, renewed when either page or the grass looks change.
fn sync_near_grass_cover(
    mut commands: Commands,
    catalog: Res<WorldCatalog>,
    looks: Res<GrassLooks>,
    terrain: Query<(Entity, Ref<StreamedTerrainSurface>), With<NearSource>>,
    fields: Query<(Entity, Ref<StreamedVegetationFieldPage>)>,
    mut joined: Local<HashMap<Entity, Entity>>,
) {
    let Some(source) = catalog.vegetation() else {
        return;
    };
    let renew = catalog.is_changed() || looks.is_changed();
    let by_cell: HashMap<_, _> = fields
        .iter()
        .map(|(entity, page)| (cell_key(page.key), entity))
        .collect();
    let mut effective: Option<VegetationCatalog> = None;
    let mut seen = HashMap::with_capacity(joined.len());
    for (entity, surface) in &terrain {
        let field = by_cell
            .get(&cell_key(surface.key))
            .and_then(|&field| fields.get(field).ok())
            .filter(|(_, page)| surface.matches_vegetation(page));
        let Some((field, page)) = field else {
            if joined.contains_key(&entity) {
                commands.entity(entity).remove::<NearGrassCover>();
            }
            continue;
        };
        seen.insert(entity, field);
        if joined.get(&entity) == Some(&field)
            && !renew
            && !page.is_changed()
            && !surface.is_changed()
        {
            continue;
        }
        let catalog = effective.get_or_insert_with(|| {
            let mut catalog = source.clone();
            if let Some(looks) = &looks.0 {
                catalog.adopt_looks(looks);
            }
            catalog
        });
        match near_grass_cover(&page.data, catalog) {
            Some(cover) => commands.entity(entity).insert(cover),
            None => commands.entity(entity).remove::<NearGrassCover>(),
        };
    }
    *joined = seen;
}

fn cell_key(key: PageKey) -> (WorldSpaceId, CellCoord, u8) {
    (key.space, key.cell, key.lod)
}

#[derive(Default)]
struct SourceState {
    pairs: Vec<(Entity, Entity)>,
    frame: Option<(WorldSpaceId, CellCoord)>,
    /// Content key per field and terrain page, renewed when either changes. A rebase moves
    /// pages without changing their samples, so it keeps their keys.
    keys: HashMap<(Entity, Entity), u64>,
}

fn sync_scene(
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    looks: Res<GrassLooks>,
    terrain: Query<(Entity, Ref<StreamedTerrainSurface>)>,
    fields: Query<(Entity, Ref<StreamedVegetationFieldPage>)>,
    mut scene: ResMut<VegetationSceneState>,
    mut previous: Local<SourceState>,
) {
    let (Some(space), Some(source_catalog)) = (origin.space(), catalog.vegetation()) else {
        if !scene.scene().pages.is_empty() {
            let catalog = scene.scene().catalog.clone();
            scene
                .replace(VegetationScene {
                    catalog,
                    pages: Vec::new(),
                })
                .expect("clearing vegetation preserves a validated catalog");
        }
        *previous = default();
        return;
    };
    let surfaces: HashMap<_, _> = terrain
        .iter()
        .filter(|(_, page)| page.key.space == space)
        .map(|(entity, page)| (cell_key(page.key), entity))
        .collect();
    let mut pairs: Vec<_> = fields
        .iter()
        .filter(|(_, page)| page.key.space == space)
        .filter_map(|(entity, page)| {
            let surface = *surfaces.get(&cell_key(page.key))?;
            terrain
                .get(surface)
                .ok()?
                .1
                .matches_vegetation(&page)
                .then_some((cell_key(page.key), entity, surface))
        })
        .collect();
    pairs.sort_by_key(|&(key, entity, _)| (key, entity));
    let pairs: Vec<_> = pairs
        .into_iter()
        .map(|(_, field, surface)| (field, surface))
        .collect();
    let frame = (space, origin.cell());
    let edited = pairs.iter().any(|&(field, surface)| {
        fields.get(field).unwrap().1.is_changed() || terrain.get(surface).unwrap().1.is_changed()
    });
    if previous.pairs == pairs
        && previous.frame == Some(frame)
        && !catalog.is_changed()
        && !looks.is_changed()
        && !edited
    {
        return;
    }
    let mut source_catalog = source_catalog.clone();
    if let Some(looks) = &looks.0 {
        source_catalog.adopt_looks(looks);
    }
    let pages = pairs
        .iter()
        .map(|&(field, surface)| {
            terrain
                .get(surface)
                .unwrap()
                .1
                .vegetation_page(origin.cell(), &fields.get(field).unwrap().1.data)
        })
        .collect();
    let mut keys = HashMap::with_capacity(pairs.len());
    let page_keys = pairs
        .iter()
        .map(|&(field, surface)| {
            let changed = fields.get(field).unwrap().1.is_changed()
                || terrain.get(surface).unwrap().1.is_changed();
            let key = match previous.keys.get(&(field, surface)) {
                Some(&key) if !changed => key,
                _ => VegetationSceneState::new_page_key(),
            };
            keys.insert((field, surface), key);
            key
        })
        .collect();
    scene
        .replace_with_page_keys(
            VegetationScene {
                catalog: source_catalog,
                pages,
            },
            page_keys,
        )
        .expect("published resident vegetation and terrain form a valid scene");
    previous.keys = keys;
    previous.pairs = pairs;
    previous.frame = Some(frame);
}

fn update_lod_focus(
    viewpoint: Res<WorldViewpoint>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    mut focus: ResMut<VegetationLodFocus>,
) {
    let position = viewpoint.position().and_then(|position| {
        if Some(position.space) != origin.space() {
            return None;
        }
        let space = catalog.world_space(position.space)?;
        Some(Vec3::from_array(
            position.relative_to(origin.cell(), space.cell_size),
        ))
    });
    if focus.position != position {
        focus.position = position;
    }
}

#[cfg(test)]
mod tests;
