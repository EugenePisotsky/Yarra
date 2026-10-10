//! The open generation's world spaces, vegetation catalog and named places, as the rest of
//! the game reads them.
use super::*;

#[derive(Debug, Clone)]
pub struct WorldSpaceInfo {
    pub atmosphere: world::atmosphere::AtmosphereProfile,
    pub id: WorldSpaceId,
    pub name: String,
    pub cell_size: f32,
    pub minimum_y: f32,
    pub maximum_y: f32,
    pub sea_level: Option<f32>,
}

#[derive(Resource, Debug, Default, Clone)]
pub struct WorldCatalog {
    pub(super) generation_id: String,
    pub(super) default_world_space: Option<WorldSpaceId>,
    pub(super) world_spaces: Vec<WorldSpaceInfo>,
    pub(super) vegetation: Option<VegetationCatalog>,
    pub(super) gameplay_areas: world::GameplayAreaIndex,
}

impl WorldCatalog {
    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    pub fn default_world_space(&self) -> Option<WorldSpaceId> {
        self.default_world_space
    }

    pub fn world_spaces(&self) -> &[WorldSpaceInfo] {
        &self.world_spaces
    }

    pub fn world_space(&self, id: WorldSpaceId) -> Option<&WorldSpaceInfo> {
        self.world_spaces.iter().find(|space| space.id == id)
    }

    pub fn vegetation(&self) -> Option<&VegetationCatalog> {
        self.vegetation.as_ref()
    }

    /// The named places gameplay reacts to, published with the world.
    pub fn gameplay_areas(&self) -> &world::GameplayAreaIndex {
        &self.gameplay_areas
    }

    /// Takes the identity, world spaces, vegetation and places of an opened generation.
    pub(super) fn adopt(&mut self, manifest: &RuntimeManifest) {
        self.generation_id = manifest.generation_id.clone();
        self.default_world_space = Some(manifest.default_world_space);
        self.world_spaces = manifest
            .world_spaces
            .iter()
            .map(|space| WorldSpaceInfo {
                atmosphere: space.atmosphere.clone(),
                id: space.id,
                name: space.name.clone(),
                cell_size: space.cell_size,
                minimum_y: space.minimum_y,
                maximum_y: space.maximum_y,
                sea_level: space.sea_level,
            })
            .collect();
        self.vegetation = manifest.vegetation_catalog.clone();
        self.gameplay_areas = world::GameplayAreaIndex::new(manifest.gameplay_areas.clone());
    }
}

pub(super) fn adopt_runtime_manifest(
    manifest: RuntimeManifest,
    active_space: &mut ActiveWorldSpace,
    catalog: &mut WorldCatalog,
    viewpoint: &mut WorldViewpoint,
    origin: &mut WorldOrigin,
    stream: &mut WorldStream,
) {
    let active = active_space
        .current
        .filter(|space| manifest.world_space(*space).is_some())
        .unwrap_or(manifest.default_world_space);
    active_space.current = Some(active);
    catalog.adopt(&manifest);
    if viewpoint
        .position
        .is_none_or(|position| position.space != active)
    {
        viewpoint.position = Some(WorldPosition {
            space: active,
            cell: CellCoord::ZERO,
            local: [0.0, 0.0, 0.0],
        });
    }
    if origin.space != Some(active) {
        origin.space = Some(active);
        origin.cell = viewpoint
            .position
            .map_or(CellCoord::ZERO, |position| position.cell);
    }
    stream.manifest = Some(manifest);
    stream.phase = StreamPhase::Ready;
}

#[cfg(test)]
impl WorldCatalog {
    pub(crate) fn world_spaces_mut(&mut self) -> &mut Vec<WorldSpaceInfo> {
        &mut self.world_spaces
    }
}
