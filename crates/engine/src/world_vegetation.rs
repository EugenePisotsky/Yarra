//! Production joining of streamed grass fields and terrain, and the render-space detail focus.
//! Editor source selection stays in the editor; terrain conversion is shared.

use bevy::prelude::*;
use std::collections::HashMap;

use vegetation::{
    VegetationCatalog, VegetationFieldPage, VegetationFieldPageData, VegetationScene,
    VegetationSurfaceField,
};

use vegetation_render::{
    GrassBender, GrassBenders, MAX_GRASS_BENDERS, VegetationLodFocus, VegetationRenderPlugin,
    VegetationSceneState, VegetationTerrainGate,
};

use world::{CellCoord, PageKey, WorldSpaceId};

use crate::{
    ClipmapTerrain, StreamedTerrainSurface, StreamedVegetationFieldPage, WorldCatalog, WorldOrigin,
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
            (sync_scene, gate_grass_on_ground)
                .chain()
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

fn cell_key(key: PageKey) -> (WorldSpaceId, CellCoord) {
    (key.space, key.cell)
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

    // Each cell's grass fields join its terrain page.
    let mut surfaces: HashMap<_, _> = terrain
        .iter()
        .filter(|(_, page)| page.key.space == space)
        .map(|(entity, page)| (cell_key(page.key), (entity, page)))
        .collect();
    let mut joined: Vec<_> = fields
        .iter()
        .filter(|(_, page)| page.key.space == space)
        .filter_map(|(field, page)| {
            let key = cell_key(page.key);
            let (surface, ground) = surfaces.remove(&key)?;
            ground
                .matches_vegetation(&page)
                .then_some((key, field, page, surface, ground))
        })
        .collect();
    joined.sort_by_key(|&(key, field, ..)| (key, field));
    let pairs: Vec<_> = joined
        .iter()
        .map(|&(_, field, _, surface, _)| (field, surface))
        .collect();
    let changed = |(_, _, page, _, ground): &(
        _,
        _,
        Ref<StreamedVegetationFieldPage>,
        _,
        Ref<StreamedTerrainSurface>,
    )| { page.is_changed() || ground.is_changed() };
    let frame = (space, origin.cell());

    if previous.pairs == pairs
        && previous.frame == Some(frame)
        && !catalog.is_changed()
        && !looks.is_changed()
        && !joined.iter().any(changed)
    {
        return;
    }

    let mut source_catalog = source_catalog.clone();

    if let Some(looks) = &looks.0 {
        source_catalog.adopt_looks(looks);
    }

    let pages = joined
        .iter()
        .map(|(_, _, page, _, ground)| ground.vegetation_page(origin.cell(), &page.data))
        .collect();
    let mut keys = HashMap::with_capacity(joined.len());
    let page_keys = joined
        .iter()
        .map(|j| {
            let pair = (j.1, j.3);
            let key = match previous.keys.get(&pair) {
                Some(&key) if !changed(j) => key,
                _ => VegetationSceneState::new_page_key(),
            };
            keys.insert(pair, key);
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

/// Grass stands on its page's full-detail heights. Where the clipmap still draws a coarser
/// level because the cell's leaf is not resident yet (at start, after a teleport, on a fast
/// move), blades would float or sink, so the page waits for its leaf; all grass waits while
/// no ground is resident.
fn gate_grass_on_ground(
    terrain: Option<Res<ClipmapTerrain>>,
    origin: Res<WorldOrigin>,
    scene: Res<VegetationSceneState>,
    mut gate: ResMut<VegetationTerrainGate>,
) {
    let Some(terrain) = terrain else {
        return;
    };
    let mut next = VegetationTerrainGate {
        block_all: !terrain.has_ground(),
        ..default()
    };

    if !next.block_all {
        next.blocked_pages = scene
            .scene()
            .pages
            .iter()
            .filter(|page| {
                let half = page.size * 0.5;
                let centre = Vec3::new(page.origin_xz[0] + half, 0., page.origin_xz[1] + half);
                !terrain.leaf_resident(centre, &origin)
            })
            .map(VegetationTerrainGate::page_id)
            .collect();
    }

    gate.set_if_neq(next);
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
