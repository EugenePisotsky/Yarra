//! Production joining of streamed fields/terrain, render-space focus and ground canopy.
//! Editor source selection stays in the editor; terrain conversion and canopy caching are shared.
mod canopy;
pub use canopy::{GroundCanopyPlugin, GroundCanopySystems, GroundCanopyTiles};

use bevy::prelude::*;
use std::collections::HashMap;
use vegetation::{
    VegetationFieldPage, VegetationFieldPageData, VegetationScene, VegetationSurfaceField,
};
use vegetation_render::{VegetationLodFocus, VegetationRenderPlugin, VegetationSceneState};
use world::{CellCoord, PageKey, WorldSpaceId};

use crate::{
    StreamedTerrainSurface, StreamedVegetationFieldPage, WorldCatalog, WorldOrigin,
    WorldStreamingSystems, WorldViewpoint,
};

/// Installs the renderer, joins published resident sources, updates grass focus, and maintains
/// ground canopy. Requires world streaming, Time and terrain/image asset resources.
///
/// Render isolation (including Grass Disabled) does not suspend source joining: contact and
/// canopy consumers must see the current world/origin even while vegetation draws are disabled.
/// The editor uses its own scene producer and only shares GroundCanopyPlugin and page conversion.
pub struct WorldVegetationPlugin;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldVegetationSystems;

impl Plugin for WorldVegetationPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(VegetationRenderPlugin)
            .insert_resource(
                VegetationSceneState::new(VegetationScene {
                    catalog: vegetation::fixtures::reference_catalog(),
                    pages: Vec::new(),
                })
                .expect("empty vegetation scene is valid"),
            )
            .add_systems(
                Update,
                sync_scene
                    .in_set(WorldVegetationSystems)
                    .after(WorldStreamingSystems),
            )
            .add_plugins(GroundCanopyPlugin)
            .configure_sets(Update, GroundCanopySystems.after(WorldVegetationSystems))
            .add_systems(
                PostUpdate,
                update_lod_focus.after(TransformSystems::Propagate),
            );
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

fn cell_key(key: PageKey) -> (WorldSpaceId, CellCoord, u8) {
    (key.space, key.cell, key.lod)
}

#[derive(Default)]
struct SourceState {
    pairs: Vec<(Entity, Entity)>,
    frame: Option<(WorldSpaceId, CellCoord)>,
}

fn sync_scene(
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
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
    if previous.pairs == pairs && previous.frame == Some(frame) && !catalog.is_changed() && !edited
    {
        return;
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
    scene
        .replace(VegetationScene {
            catalog: source_catalog.clone(),
            pages,
        })
        .expect("published resident vegetation and terrain form a valid scene");
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
