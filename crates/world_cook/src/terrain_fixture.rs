//! Explicit geometry acceptance fixture; never used by normal initialization.
use super::*;

pub fn create_mountain_fixture(path: &Path) -> Result<()> {
    write_project_database(path, &mountain_project(32))
        .with_context(|| format!("failed to create mountain fixture at {}", path.display()))
}

pub(crate) fn mountain_project(half_cells: i32) -> ProjectDocument {
    let mut project = demo_project_document();
    let space = project.default_world_space;
    project
        .world_spaces
        .iter_mut()
        .find(|w| w.id == space)
        .unwrap()
        .maximum_y = 1600.0;
    project.cells.clear();
    project.terrain_cell_heightfields.clear();
    project.environment_cells.clear();
    project.objects.clear();
    for x in -half_cells..half_cells {
        for z in -half_cells..half_cells {
            let cell = CellCoord { x, z };
            project.cells.push(SourceCellRecord {
                space,
                cell,
                height: 0.0,
                source_revision: 1,
            });
            let heights = (0..33)
                .flat_map(|j| {
                    (0..33).map(move |i| {
                        let wx = (x * 32 + i) as f32;
                        let wz = (z * 32 + j) as f32;
                        100.0
                            + 1400.0 * (-((wx - 40.0) / 170.0).powi(2) - (wz / 600.0).powi(2)).exp()
                    })
                })
                .collect();
            project
                .terrain_cell_heightfields
                .push(SourceTerrainCellHeightfieldRecord {
                    space,
                    cell,
                    resolution: 33,
                    heights,
                    source_revision: 1,
                });
        }
    }
    project
}
