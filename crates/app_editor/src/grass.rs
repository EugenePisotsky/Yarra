//! Grass in the editor: the renderer, the project's grass catalog and the live preview of the
//! accepted ground and grass. Grass types are tuned in `assets/packs/grass/*.ron` (the game
//! reloads their looks while it runs) and put into a project with `yarra-world-cook import-grass`.
use crate::{project_store::ProjectEditorStore, workspaces::EditorWorkspace};
use bevy::prelude::*;
use engine::{StreamedTerrainSurface, StreamedVegetationFieldPage, WorldOrigin};
use vegetation::{VegetationCatalog, VegetationScene};
use vegetation_render::{VegetationRenderPlugin, VegetationSceneState};

pub(crate) struct GrassPlugin;

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GrassPreviewSync;

impl Plugin for GrassPlugin {
    fn build(&self, app: &mut App) {
        // The editor explicitly opts into render diagnostics; the renderer itself is reusable
        // by game compositions that omit instrumentation.
        #[cfg(not(target_os = "ios"))]
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
        app.add_plugins(VegetationRenderPlugin)
            .init_resource::<GrassPreview>()
            .insert_resource(
                VegetationSceneState::new(VegetationScene {
                    catalog: VegetationCatalog::default(),
                    pages: Vec::new(),
                })
                .expect("the empty grass scene is valid"),
            )
            .add_systems(
                Update,
                (adopt_project_catalog, sync_live_preview)
                    .chain()
                    .in_set(GrassPreviewSync)
                    .after(engine::WorldStreamingSystems),
            );
    }
}

/// The project's grass catalog, as compiling and the live preview use it.
#[derive(Resource, Default)]
pub(crate) struct GrassPreview {
    catalog: Option<VegetationCatalog>,
    revision: u64,
    pub(crate) error: Option<String>,
}

impl GrassPreview {
    pub(crate) fn catalog(&self) -> Option<&VegetationCatalog> {
        self.catalog.as_ref()
    }

    /// Changes whenever the catalog does.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

fn adopt_project_catalog(project: Res<ProjectEditorStore>, mut preview: ResMut<GrassPreview>) {
    let catalog = project.vegetation_catalog();
    if catalog != preview.catalog.as_ref() {
        preview.catalog = catalog.cloned();
        preview.revision = preview.revision.wrapping_add(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewSignature {
    catalog_revision: u64,
    environment_revision: u64,
    enabled: bool,
    active_space: Option<world::WorldSpaceId>,
    origin_cell: world::CellCoord,
    pages: Vec<(Entity, i64, i32, i32, u8)>,
}

/// Joins resident terrain with the accepted grass fields (edited cells from the environment
/// preview, published pages elsewhere) into the rendered scene.
#[allow(clippy::too_many_arguments)]
fn sync_live_preview(
    runtime: Res<engine::WorldCatalog>,
    paint: Res<crate::environment_paint::EnvironmentPaintState>,
    workspace: Res<State<EditorWorkspace>>,
    environment: Res<crate::environment_paint::EnvironmentPreview>,
    origin: Res<WorldOrigin>,
    terrain_pages: Query<(Entity, &StreamedTerrainSurface)>,
    field_pages: Query<(Entity, &StreamedVegetationFieldPage)>,
    mut preview: ResMut<GrassPreview>,
    mut scene: ResMut<VegetationSceneState>,
    mut previous: Local<Option<PreviewSignature>>,
    render_origin: Option<ResMut<vegetation_render::VegetationRenderOrigin>>,
) {
    if *workspace.get() == EditorWorkspace::Presets {
        if let Some(mut render_origin) = render_origin {
            render_origin.world_xz = [0.; 2];
        }
        *previous = None;
        return;
    }
    let enabled = *workspace.get() == EditorWorkspace::World
        && preview.catalog.is_some()
        && !paint.coverage_visible;
    let active_space = origin.space();
    let fields = active_space.map_or_else(Vec::new, |space| {
        let mut fields = field_pages
            .iter()
            .filter(|(_, page)| page.key.space == space)
            .collect::<Vec<_>>();
        fields.sort_by_key(|(entity, page)| (page.key, *entity));
        fields
    });
    let mut page_signature = Vec::new();
    if enabled {
        page_signature.extend(fields.iter().map(|(entity, page)| {
            (
                *entity,
                page.key.space.0,
                page.key.cell.x,
                page.key.cell.z,
                page.key.lod,
            )
        }));
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
        catalog_revision: preview.revision,
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
    preview.error = scene
        .replace(VegetationScene { catalog, pages })
        .err()
        .map(|error| format!("Live preview rejected the scene: {error}"));
    *previous = Some(signature);
}
