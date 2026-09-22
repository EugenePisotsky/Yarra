//! Published terrain/vegetation joining for the editor live preview.
use crate::{vegetation_authoring::VegetationAuthoringState, workspaces::EditorWorkspace};
use bevy::prelude::*;
use engine::{StreamedTerrainSurface, StreamedVegetationFieldPage, WorldOrigin};
use vegetation::VegetationScene;
use vegetation_render::VegetationSceneState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreviewSignature {
    authoring_revision: u64,
    environment_revision: u64,
    enabled: bool,
    active_space: Option<world::WorldSpaceId>,
    origin_cell: world::CellCoord,
    pages: Vec<(Entity, i64, i32, i32, u8)>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync_live_preview(
    runtime: Res<engine::WorldCatalog>,
    paint: Res<crate::environment_paint::EnvironmentPaintState>,
    workspace: Res<State<EditorWorkspace>>,
    environment: Res<crate::environment_paint::EnvironmentPreview>,
    origin: Res<WorldOrigin>,
    terrain_pages: Query<(Entity, &StreamedTerrainSurface)>,
    field_pages: Query<(Entity, &StreamedVegetationFieldPage)>,
    mut state: ResMut<VegetationAuthoringState>,
    mut scene: ResMut<VegetationSceneState>,
    mut previous: Local<Option<PreviewSignature>>,
    render_origin: Option<ResMut<vegetation_render::VegetationRenderOrigin>>,
) {
    if matches!(
        *workspace.get(),
        EditorWorkspace::Vegetation | EditorWorkspace::Presets
    ) {
        if let Some(mut render_origin) = render_origin {
            render_origin.world_xz = [0.; 2];
        }
        *previous = None;
        return;
    }
    let tool_active = *workspace.get() == EditorWorkspace::World;
    let enabled =
        tool_active && state.preview_enabled && state.working.is_some() && !paint.coverage_visible;
    let active_space = origin.space();
    let fields = active_space.map_or_else(Vec::new, |space| {
        let mut fields = field_pages
            .iter()
            .filter(|(_, page)| page.key.space == space)
            .collect::<Vec<_>>();
        fields.sort_by_key(|(entity, page)| (page.key, *entity));
        fields
    });
    let mut page_signature = if enabled {
        fields
            .iter()
            .map(|(entity, page)| {
                (
                    *entity,
                    page.key.space.0,
                    page.key.cell.x,
                    page.key.cell.z,
                    page.key.lod,
                )
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if enabled {
        page_signature.extend(terrain_pages.iter().map(|(entity, page)| {
            (
                entity,
                page.key.space.0,
                page.key.cell.x,
                page.key.cell.z,
                page.key.lod | 0x80,
            )
        }));
        page_signature.sort();
    }
    let signature = PreviewSignature {
        authoring_revision: state.revision,
        environment_revision: environment.revision,
        enabled,
        active_space,
        origin_cell: origin.cell(),
        pages: page_signature,
    };
    if previous.as_ref() == Some(&signature) {
        return;
    }

    // The accepted catalog and fields switch together after a definition edit. While a new
    // catalog is compiling, keep rendering the previously accepted ground-and-grass products.
    let Some(catalog) = environment
        .catalog()
        .or_else(|| runtime.vegetation())
        .cloned()
    else {
        return;
    };
    let pages = if enabled {
        terrain_pages
            .iter()
            .filter(|(_, terrain)| Some(terrain.key.space) == active_space)
            .filter_map(|(_, terrain)| {
                let data = environment
                    .cell(terrain.key.space, terrain.key.cell)
                    .map(|cell| &cell.vegetation)
                    .or_else(|| {
                        if environment.catalog().is_some() {
                            return None;
                        }
                        fields
                            .iter()
                            .find(|(_, page)| terrain.matches_vegetation(page))
                            .map(|(_, page)| &page.data)
                    })?;
                Some(terrain.vegetation_page(origin.cell(), data))
            })
            .collect()
    } else {
        Vec::new()
    };
    match scene.replace(VegetationScene { catalog, pages }) {
        Ok(()) => {
            state.preview_error = None;
        }
        Err(error) => {
            state.preview_error = Some(format!("Live preview rejected the scene: {error}"));
        }
    }
    *previous = Some(signature);
}
