//! Source compilation for the editor's live ground and vegetation preview. Ground and
//! vegetation share the same accepted product.
pub(crate) mod live;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use bevy::prelude::*;
use crossbeam_channel::Receiver;
use engine::{StreamedTerrainSurface, WorldCatalog, WorldOrigin};
use environment_compile::{CompilePlan, CompiledCell};
use terrain_render::TerrainSurfaceLayer;
use world::{CellCoord, WorldSpaceId};
use world_db::{ProjectReader, SourceEnvironmentCellRecord, environment_dependency_cells};

use crate::{
    domain_editing::SourceWorkingSets, project_store::ProjectEditorStore,
    vegetation_authoring::VegetationAuthoringState, workspaces::EditorWorkspace,
};

const MAX_PREVIEW_CELLS: usize = 256;
const MAX_PREVIEW_BYTES: usize = 32 * 1024 * 1024;
type Key = (WorldSpaceId, CellCoord);
type Stamp = [u8; 32];

#[cfg(test)]
pub(super) fn compile_source_cells(
    reader: &ProjectReader,
    plan: &CompilePlan,
    space: WorldSpaceId,
    definition_revision: u64,
    library_revision: u64,
    cells: &[CellCoord],
    overrides: &[SourceEnvironmentCellRecord],
) -> Result<Vec<CompiledCell>, String> {
    compile_source_cells_with_roads(
        reader,
        plan,
        space,
        definition_revision,
        library_revision,
        cells,
        overrides,
        &[],
    )
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_source_cells_with_roads(
    reader: &ProjectReader,
    plan: &CompilePlan,
    space: WorldSpaceId,
    definition_revision: u64,
    library_revision: u64,
    cells: &[CellCoord],
    overrides: &[SourceEnvironmentCellRecord],
    roads: &[crate::road_authoring::working::RoadChange],
) -> Result<Vec<CompiledCell>, String> {
    let halo = environment_dependency_cells(cells).map_err(|e| e.to_string())?;
    if cells.len() > 1 {
        let mut result = vec![];
        for cell in cells {
            result.extend(compile_source_cells_with_roads(
                reader,
                plan,
                space,
                definition_revision,
                library_revision,
                &[*cell],
                overrides,
                roads,
            )?);
        }
        return Ok(result);
    }
    let Some(&cell) = cells.first() else {
        return Ok(vec![]);
    };
    let bounds = environment::roads::RoadCellBounds {
        minimum: world::CellCoord {
            x: cell.x - 1,
            z: cell.z - 1,
        },
        maximum: world::CellCoord {
            x: cell.x + 1,
            z: cell.z + 1,
        },
    };
    let input = reader
        .read_road_terrain_source(space, &halo, bounds)
        .map_err(|e| e.to_string())?;
    let source = input.source;
    if source.roads.roads.truncated {
        return Err("Road preview query is incomplete".into());
    }
    let mut snapshot = source.environment;
    let mut records = world_db::road_snapshot_records(&source.roads.roads);
    for change in roads {
        if let Some(r) = &change.record {
            records.insert(change.key, r.clone());
        } else {
            records.remove(&change.key);
        }
    }
    let roads = world_db::road_snapshot_from_records(
        space,
        snapshot.definition.cell_size,
        bounds,
        &records,
    )
    .map_err(|e| e.to_string())?;
    if snapshot.presets.revision != library_revision {
        return Err(
            "Shared presets changed in the database. Resolve or reload before previewing.".into(),
        );
    }
    if snapshot.definition.revision != definition_revision {
        return Err("The layer definition changed. Reload the project before painting.".into());
    }
    for record in overrides {
        if record.definition_revision != definition_revision {
            return Err("A painted cell belongs to a different layer definition.".into());
        }
        if let Some(cell) = snapshot
            .coverage
            .cells
            .iter_mut()
            .find(|cell| cell.cell == record.cell)
        {
            *cell = record.coverage();
        }
    }
    plan.compile_cell_with_terrain(
        cell,
        &snapshot.coverage,
        &roads,
        &input.terrain,
        Default::default(),
    )
    .map(|cell| vec![cell])
    .map_err(|e| e.to_string())
}

/// The accepted live preview: compiled cells for the resident neighbourhood and the catalog
/// they were compiled against.
#[derive(Resource, Default)]
pub(crate) struct EnvironmentPreview {
    pub(super) assets: BTreeMap<world::AssetId, world_db::CollectionAssetView>,
    desired: BTreeMap<Key, Stamp>,
    accepted: BTreeMap<Key, (Stamp, CompiledCell)>,
    active_catalog: Option<vegetation::VegetationCatalog>,
    pub(crate) revision: u64,
    pub(crate) error: Option<String>,
}
impl EnvironmentPreview {
    pub(crate) fn cell(&self, space: WorldSpaceId, cell: CellCoord) -> Option<&CompiledCell> {
        self.accepted.get(&(space, cell)).map(|(_, cell)| cell)
    }
    pub(crate) fn catalog(&self) -> Option<&vegetation::VegetationCatalog> {
        self.active_catalog.as_ref()
    }
    pub(super) fn visible_cells(&self) -> &BTreeMap<Key, (Stamp, CompiledCell)> {
        &self.accepted
    }
    pub(crate) fn pending(&self) -> usize {
        self.desired
            .iter()
            .filter(|(key, stamp)| self.accepted.get(key).map(|(s, _)| s) != Some(stamp))
            .count()
    }
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
}

/// Includes only the local cells in this output's halo, so an unrelated stroke cannot discard it.
fn dependency_stamp(
    plan: Stamp,
    epoch: u64,
    generation: &str,
    cell: CellCoord,
    overrides: &[(CellCoord, Stamp)],
) -> Stamp {
    let mut hash = blake3::Hasher::new();
    hash.update(&plan);
    hash.update(&epoch.to_le_bytes());
    hash.update(generation.as_bytes());
    hash.update(&cell.x.to_le_bytes());
    hash.update(&cell.z.to_le_bytes());
    for (neighbor, stamp) in overrides {
        if (i64::from(neighbor.x) - i64::from(cell.x)).abs() <= 1
            && (i64::from(neighbor.z) - i64::from(cell.z)).abs() <= 1
        {
            hash.update(&neighbor.x.to_le_bytes());
            hash.update(&neighbor.z.to_le_bytes());
            hash.update(stamp);
        }
    }
    *hash.finalize().as_bytes()
}
fn override_stamp(record: &SourceEnvironmentCellRecord) -> Stamp {
    let mut hash = blake3::Hasher::new();
    hash.update(&record.definition_revision.to_le_bytes());
    let mut tiles = record.tiles.iter().collect::<Vec<_>>();
    tiles.sort_by_key(|tile| tile.layer);
    for tile in tiles {
        hash.update(&tile.layer.0);
        hash.update(&(tile.samples.len() as u64).to_le_bytes());
        hash.update(&tile.samples);
    }
    *hash.finalize().as_bytes()
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PreviewSource<'w, 's> {
    project: Res<'w, ProjectEditorStore>,
    dense: Res<'w, SourceWorkingSets>,
    plants: Res<'w, VegetationAuthoringState>,
    origin: Res<'w, WorldOrigin>,
    runtime: Res<'w, WorldCatalog>,
    workspace: Res<'w, State<EditorWorkspace>>,
    terrain: Query<'w, 's, &'static StreamedTerrainSurface>,
}

fn compiled_bytes(cell: &CompiledCell) -> usize {
    1024 + cell.objects.len() * std::mem::size_of::<world::StaticObjectInstance>()
        + cell
            .ground
            .weight_pages
            .iter()
            .map(|p| p.rgba.len())
            .sum::<usize>()
        + cell
            .vegetation
            .fields
            .iter()
            .map(|f| f.coverage.len() + 128)
            .sum::<usize>()
        + cell
            .terrain
            .as_ref()
            .map_or(0, |h| h.heights.len() * 4 + h.normals_oct.len() * 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_stamp_tracks_halo_but_not_unrelated_strokes() {
        let cell = CellCoord { x: 0, z: 0 };
        let stamp =
            |overrides: &[(CellCoord, Stamp)]| dependency_stamp([3; 32], 1, "a", cell, overrides);
        assert_eq!(stamp(&[]), stamp(&[(CellCoord { x: 2, z: 0 }, [1; 32])]));
        assert_ne!(stamp(&[]), stamp(&[(CellCoord { x: 1, z: 1 }, [1; 32])]));
        assert_ne!(stamp(&[]), dependency_stamp([3; 32], 2, "a", cell, &[]));
        assert_ne!(stamp(&[]), dependency_stamp([3; 32], 1, "b", cell, &[]));
    }
}
