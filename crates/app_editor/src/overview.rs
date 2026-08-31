//! Bounded coarse-product demand used when the World camera leaves local editing scale.

use bevy::prelude::*;
use engine::{WorldCatalog, WorldDetailDemand, WorldOrigin, WorldViewCamera, WorldViewpoint};
use world::{CellCoord, WorldPosition, WorldSpaceId};

use crate::derived_jobs::{
    DerivedArtifact, DerivedArtifactStore, DerivedJobKey, DerivedJobScheduler, DerivedJobScope,
};
use crate::project_store::ProjectEditorStore;
use crate::tools::{DerivedProduct, EditorToolId};
use crate::workspaces::{
    world_impl::{EditorCamera, EditorOverlayGizmos, update_editor_camera},
    world_workspace_active,
};

const OVERVIEW_ENTER_DISTANCE: f32 = 768.0;
const OVERVIEW_EXIT_DISTANCE: f32 = 576.0;
const OVERVIEW_TILE_CELLS: i32 = 16;
const OVERVIEW_RADIUS_TILES: i32 = 2;
const MAX_OVERVIEW_TILES: usize = 25;
const MAX_OVERVIEW_PRODUCTS: usize = MAX_OVERVIEW_TILES * OverviewProductKind::ALL.len();
const MAX_DRAWN_OBJECT_ICONS_PER_TILE: usize = 256;
const OVERVIEW_DERIVED_OWNER: EditorToolId = EditorToolId("world.overview");

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverviewMode {
    #[default]
    Detail,
    Overview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OverviewProductKind {
    CoarseTerrain,
    DensityMap,
    ObjectIcons,
    CellStatus,
}

impl OverviewProductKind {
    pub(crate) const ALL: [Self; 4] = [
        Self::CoarseTerrain,
        Self::DensityMap,
        Self::ObjectIcons,
        Self::CellStatus,
    ];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::CoarseTerrain => "terrain",
            Self::DensityMap => "density",
            Self::ObjectIcons => "icons",
            Self::CellStatus => "status",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OverviewTileKey {
    pub(crate) space: WorldSpaceId,
    pub(crate) x: i32,
    pub(crate) z: i32,
}

impl OverviewTileKey {
    fn containing(space: WorldSpaceId, cell: CellCoord) -> Self {
        Self {
            space,
            x: cell.x.div_euclid(OVERVIEW_TILE_CELLS),
            z: cell.z.div_euclid(OVERVIEW_TILE_CELLS),
        }
    }

    fn minimum_cell(self) -> CellCoord {
        CellCoord {
            x: self.x.saturating_mul(OVERVIEW_TILE_CELLS),
            z: self.z.saturating_mul(OVERVIEW_TILE_CELLS),
        }
    }

    fn maximum_cell(self) -> CellCoord {
        let minimum = self.minimum_cell();
        CellCoord {
            x: minimum.x.saturating_add(OVERVIEW_TILE_CELLS - 1),
            z: minimum.z.saturating_add(OVERVIEW_TILE_CELLS - 1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverviewProductState {
    Missing,
    Requested,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct OverviewProductDemand {
    pub(crate) tile: OverviewTileKey,
    pub(crate) kind: OverviewProductKind,
    pub(crate) state: OverviewProductState,
}

#[derive(Resource, Default)]
pub(crate) struct OverviewState {
    mode: OverviewMode,
    tiles: Vec<OverviewTileKey>,
    products: Vec<OverviewProductDemand>,
}

impl OverviewState {
    pub(crate) const fn mode(&self) -> OverviewMode {
        self.mode
    }

    pub(crate) fn tiles(&self) -> &[OverviewTileKey] {
        &self.tiles
    }

    pub(crate) fn products(&self) -> &[OverviewProductDemand] {
        &self.products
    }

    pub(crate) fn focus_position(
        &self,
        tile: OverviewTileKey,
        current: WorldPosition,
        cell_size: f32,
    ) -> WorldPosition {
        let minimum = tile.minimum_cell();
        WorldPosition {
            space: tile.space,
            cell: CellCoord {
                x: minimum.x.saturating_add(OVERVIEW_TILE_CELLS / 2),
                z: minimum.z.saturating_add(OVERVIEW_TILE_CELLS / 2),
            },
            local: [cell_size * 0.5, current.local[1], cell_size * 0.5],
        }
    }
}

pub(crate) struct OverviewPlugin;

impl Plugin for OverviewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OverviewState>()
            .add_systems(
                Update,
                (
                    update_overview_state,
                    request_overview_products,
                    sync_overview_products,
                )
                    .chain()
                    .after(update_editor_camera)
                    .run_if(world_workspace_active),
            )
            .add_systems(
                PostUpdate,
                draw_overview_tile_proxies.run_if(world_workspace_active),
            );
    }
}

fn update_overview_state(
    camera: Single<&EditorCamera, With<WorldViewCamera>>,
    viewpoint: Res<WorldViewpoint>,
    mut detail_demand: ResMut<WorldDetailDemand>,
    mut overview: ResMut<OverviewState>,
) {
    overview.mode = overview_mode(overview.mode, camera.distance);
    detail_demand.set_enabled(overview.mode == OverviewMode::Detail);
    let Some(position) = viewpoint.position() else {
        overview.tiles.clear();
        overview.products.clear();
        return;
    };
    overview.tiles = demanded_tiles(position.space, position.cell);
    overview.products = overview
        .tiles
        .iter()
        .flat_map(|tile| {
            OverviewProductKind::ALL.map(|kind| OverviewProductDemand {
                tile: *tile,
                kind,
                state: OverviewProductState::Missing,
            })
        })
        .take(MAX_OVERVIEW_PRODUCTS)
        .collect();
}

fn request_overview_products(
    overview: Res<OverviewState>,
    project: Res<ProjectEditorStore>,
    mut scheduler: ResMut<DerivedJobScheduler>,
) {
    let requests = if overview.mode == OverviewMode::Overview {
        overview
            .tiles
            .iter()
            .map(|tile| (overview_job_key(*tile), project.source_epoch().max(1)))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    scheduler.replace_tool_requests(OVERVIEW_DERIVED_OWNER, requests);
}

fn sync_overview_products(
    artifacts: Res<DerivedArtifactStore>,
    mut overview: ResMut<OverviewState>,
) {
    let ready_tiles = overview
        .tiles
        .iter()
        .filter(|tile| artifacts.get(overview_job_key(**tile)).is_some())
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let failed_tiles = overview
        .tiles
        .iter()
        .filter(|tile| artifacts.failure(overview_job_key(**tile)).is_some())
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let mode = overview.mode;
    for product in &mut overview.products {
        product.state = if ready_tiles.contains(&product.tile) {
            OverviewProductState::Ready
        } else if failed_tiles.contains(&product.tile) {
            OverviewProductState::Failed
        } else if mode == OverviewMode::Overview {
            OverviewProductState::Requested
        } else {
            OverviewProductState::Missing
        };
    }
}

fn overview_job_key(tile: OverviewTileKey) -> DerivedJobKey {
    DerivedJobKey {
        product: DerivedProduct::Overview,
        scope: DerivedJobScope::Region {
            space: tile.space,
            minimum: tile.minimum_cell(),
            maximum: tile.maximum_cell(),
        },
    }
}

fn overview_mode(current: OverviewMode, distance: f32) -> OverviewMode {
    match current {
        OverviewMode::Detail if distance >= OVERVIEW_ENTER_DISTANCE => OverviewMode::Overview,
        OverviewMode::Overview if distance <= OVERVIEW_EXIT_DISTANCE => OverviewMode::Detail,
        mode => mode,
    }
}

fn demanded_tiles(space: WorldSpaceId, cell: CellCoord) -> Vec<OverviewTileKey> {
    let center = OverviewTileKey::containing(space, cell);
    let mut tiles = Vec::with_capacity(MAX_OVERVIEW_TILES);
    for z in -OVERVIEW_RADIUS_TILES..=OVERVIEW_RADIUS_TILES {
        for x in -OVERVIEW_RADIUS_TILES..=OVERVIEW_RADIUS_TILES {
            tiles.push(OverviewTileKey {
                space,
                x: center.x.saturating_add(x),
                z: center.z.saturating_add(z),
            });
        }
    }
    tiles
}

fn draw_overview_tile_proxies(
    overview: Res<OverviewState>,
    artifacts: Res<DerivedArtifactStore>,
    catalog: Res<WorldCatalog>,
    origin: Res<WorldOrigin>,
    mut gizmos: Gizmos<EditorOverlayGizmos>,
) {
    if overview.mode != OverviewMode::Overview {
        return;
    }
    for tile in &overview.tiles {
        let Some(space) = catalog.world_space(tile.space) else {
            continue;
        };
        let minimum = tile.minimum_cell();
        let x = (i64::from(minimum.x) - i64::from(origin.cell().x)) as f32 * space.cell_size;
        let z = (i64::from(minimum.z) - i64::from(origin.cell().z)) as f32 * space.cell_size;
        let size = OVERVIEW_TILE_CELLS as f32 * space.cell_size;
        let artifact = artifacts.get(overview_job_key(*tile));
        let color = if artifact.is_some() {
            Color::srgba(0.3, 0.72, 0.98, 0.9)
        } else {
            Color::srgba(0.95, 0.62, 0.2, 0.75)
        };
        let corners = [
            Vec3::new(x, 0.08, z),
            Vec3::new(x + size, 0.08, z),
            Vec3::new(x + size, 0.08, z + size),
            Vec3::new(x, 0.08, z + size),
        ];
        for edge in 0..4 {
            gizmos.line(corners[edge], corners[(edge + 1) % 4], color);
        }
        let Some(DerivedArtifact::Overview {
            coarse_heights,
            object_icons,
            cell_status: _,
            fingerprint: _,
        }) = artifact
        else {
            continue;
        };
        for (cell, height) in coarse_heights.iter().filter(|(cell, _)| {
            (cell.x - minimum.x).rem_euclid(4) == 0 && (cell.z - minimum.z).rem_euclid(4) == 0
        }) {
            let center = overview_cell_center(*cell, *height, space.cell_size, origin.cell());
            gizmos.line(
                center - Vec3::X * 2.0,
                center + Vec3::X * 2.0,
                Color::srgba(0.42, 0.68, 0.95, 0.7),
            );
            gizmos.line(
                center - Vec3::Z * 2.0,
                center + Vec3::Z * 2.0,
                Color::srgba(0.42, 0.68, 0.95, 0.7),
            );
        }
        for (_, cell) in object_icons.iter().take(MAX_DRAWN_OBJECT_ICONS_PER_TILE) {
            let center = overview_cell_center(*cell, 1.0, space.cell_size, origin.cell());
            gizmos.line(
                center - Vec3::X * 1.5,
                center + Vec3::X * 1.5,
                Color::srgba(1.0, 0.82, 0.28, 0.95),
            );
            gizmos.line(
                center - Vec3::Z * 1.5,
                center + Vec3::Z * 1.5,
                Color::srgba(1.0, 0.82, 0.28, 0.95),
            );
        }
    }
}

fn overview_cell_center(cell: CellCoord, height: f32, cell_size: f32, origin: CellCoord) -> Vec3 {
    Vec3::new(
        (i64::from(cell.x) - i64::from(origin.x)) as f32 * cell_size + cell_size * 0.5,
        height + 0.12,
        (i64::from(cell.z) - i64::from(origin.z)) as f32 * cell_size + cell_size * 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_mode_uses_hysteresis() {
        assert_eq!(
            overview_mode(OverviewMode::Detail, OVERVIEW_ENTER_DISTANCE),
            OverviewMode::Overview
        );
        assert_eq!(
            overview_mode(OverviewMode::Overview, 700.0),
            OverviewMode::Overview
        );
        assert_eq!(
            overview_mode(OverviewMode::Overview, OVERVIEW_EXIT_DISTANCE),
            OverviewMode::Detail
        );
    }

    #[test]
    fn overview_tile_demand_is_bounded_and_handles_negative_cells() {
        let tiles = demanded_tiles(WorldSpaceId(4), CellCoord { x: -1, z: -17 });
        assert_eq!(tiles.len(), MAX_OVERVIEW_TILES);
        assert!(tiles.contains(&OverviewTileKey {
            space: WorldSpaceId(4),
            x: -1,
            z: -2,
        }));
    }
}
