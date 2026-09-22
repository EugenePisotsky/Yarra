//! Exercise the supported crate-root API across source and runtime databases.
use std::{fs, path::PathBuf};
use world::{CellCoord, RUNTIME_SCHEMA_VERSION, WorldSpaceId};
use yarra_world_db::*;

#[test]
fn project_and_runtime_databases_are_distinct_and_readable() {
    let directory = unique_test_directory();
    fs::create_dir_all(&directory).unwrap();
    let project_path = directory.join("project.sqlite");
    let runtime_path = directory.join("runtime.sqlite");
    let space = WorldSpaceRecord {
        atmosphere: Default::default(),
        atmosphere_revision: 1,
        id: WorldSpaceId(1),
        name: "test".into(),
        cell_size: 32.0,
        minimum_y: -8.0,
        maximum_y: 16.0,
    };
    let vegetation_catalog = vegetation::fixtures::reference_catalog();
    let project = ProjectDocument {
        default_world_space: space.id,
        world_spaces: vec![space.clone()],
        vegetation_catalog: Some(vegetation_catalog.clone()),
        cells: vec![SourceCellRecord {
            space: space.id,
            cell: CellCoord::ZERO,
            height: 1.5,
            source_revision: 1,
        }],
        terrain_surfaces: Vec::new(),
        terrain_texture_sets: Vec::new(),
        terrain_texture_layers: Vec::new(),
        terrain_profiles: Vec::new(),
        presets: Default::default(),
        environments: Vec::new(),
        environment_cells: Vec::new(),
        roads: Default::default(),
        terrain_cell_heightfields: Vec::new(),
        assets: Vec::new(),
        asset_variants: Vec::new(),
        definitions: Vec::new(),
        objects: Vec::new(),
    };
    write_project_database(&project_path, &project).unwrap();
    let read_project = read_project_database(&project_path).unwrap();
    assert_eq!(read_project.default_world_space, space.id);
    assert_eq!(read_project.cells.len(), 1);
    assert_eq!(
        read_project
            .vegetation_catalog
            .as_ref()
            .map(|catalog| catalog.populations.len()),
        Some(vegetation_catalog.populations.len())
    );

    let mut replacement_catalog = vegetation_catalog.clone();
    replacement_catalog.populations[0].density_per_square_meter = 5.5;
    ProjectWriter::open(&project_path)
        .unwrap()
        .replace_vegetation_catalog(&replacement_catalog)
        .unwrap();
    assert_eq!(
        ProjectReader::open_read_only(&project_path)
            .unwrap()
            .read_vegetation_catalog()
            .unwrap(),
        Some(replacement_catalog.clone())
    );

    let mut authored_catalog = replacement_catalog.clone();
    authored_catalog.populations[0].density_per_square_meter = 6.25;
    let mut writer = ProjectWriter::open(&project_path).unwrap();
    assert_eq!(
        writer
            .replace_vegetation_catalog_if_matches(Some(&replacement_catalog), &authored_catalog,)
            .unwrap(),
        VegetationCatalogWriteResult::Committed
    );

    let mut stale_catalog = replacement_catalog.clone();
    stale_catalog.populations[0].density_per_square_meter = 7.0;
    assert_eq!(
        writer
            .replace_vegetation_catalog_if_matches(Some(&replacement_catalog), &stale_catalog,)
            .unwrap(),
        VegetationCatalogWriteResult::Conflict {
            actual: Some(authored_catalog.clone()),
        }
    );
    assert_eq!(
        ProjectReader::open_read_only(&project_path)
            .unwrap()
            .read_vegetation_catalog()
            .unwrap(),
        Some(authored_catalog)
    );

    let runtime = RuntimeBuild {
        manifest: RuntimeManifest {
            schema_version: RUNTIME_SCHEMA_VERSION,
            generation_id: "test-generation".into(),
            content_hash: [7; 32],
            default_world_space: space.id,
            world_spaces: vec![space],
            vegetation_catalog: Some(vegetation_catalog.clone()),
        },
        cells: Vec::new(),
        pages: Vec::new(),
        terrain_surfaces: Vec::new(),
        terrain_texture_sets: Vec::new(),
        terrain_texture_layers: Vec::new(),
        terrain_profiles: Vec::new(),
        assets: Vec::new(),
        definitions: Vec::new(),
        dependencies: Vec::new(),
        definition_dependencies: Vec::new(),
        terrain_surface_dependencies: Vec::new(),
    };
    write_runtime_database(&runtime_path, &runtime).unwrap();
    let reader = RuntimeReader::open_immutable(&runtime_path).unwrap();
    assert_eq!(reader.manifest().generation_id, "test-generation");
    assert_eq!(
        reader
            .manifest()
            .vegetation_catalog
            .as_ref()
            .map(|catalog| catalog.populations.len()),
        Some(vegetation_catalog.populations.len())
    );
    fs::remove_dir_all(directory).unwrap();
}

fn unique_test_directory() -> PathBuf {
    std::env::temp_dir().join(format!(
        "yarra-world-db-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ))
}
