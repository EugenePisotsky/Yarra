use std::path::PathBuf;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .unwrap_or_else(|| "cook".into());
    if command == "create-demo" || command == "create-road-demo" {
        let project = PathBuf::from(arguments.next().context("expected a new PROJECT_DB path")?);
        if arguments.next().is_some() {
            bail!("usage: yarra-world-cook {command} PROJECT_DB");
        }
        if command == "create-road-demo" {
            yarra_world_cook::create_road_demo_project(&project)?;
        } else {
            yarra_world_cook::create_demo_project(&project)?;
        }
        println!(
            "created layer-based authoring database: {}",
            project.display()
        );
        return Ok(());
    }
    if command == "export-vegetation" {
        use std::io::Write;
        let project = PathBuf::from(arguments.next().context("expected PROJECT_DB")?);
        let output = PathBuf::from(arguments.next().context("expected CATALOG_RON")?);
        if arguments.next().is_some() {
            bail!("usage: yarra-world-cook export-vegetation PROJECT_DB CATALOG_RON");
        }
        let catalog = world_db::ProjectReader::open_read_only(&project)?
            .read_vegetation_catalog()?
            .context("project has no vegetation catalog")?;
        let text = ron::ser::to_string_pretty(&catalog, ron::ser::PrettyConfig::default())?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?
            .write_all(text.as_bytes())?;
        return Ok(());
    }
    if command == "import-vegetation" {
        let source = PathBuf::from(arguments.next().context("expected catalog.ron")?);
        let project = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(default_project_path);
        let runtime = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(default_runtime_path);
        if arguments.next().is_some() {
            bail!(
                "usage: yarra-world-cook import-vegetation CATALOG_RON [PROJECT_DB] [RUNTIME_DB]"
            );
        }
        let catalog: vegetation::VegetationCatalog =
            ron::from_str(&std::fs::read_to_string(&source)?)?;
        catalog.validate()?;
        world_db::ProjectWriter::open(&project)?.replace_vegetation_catalog(&catalog)?;
        let manifest = yarra_world_cook::cook_project(&project, &runtime)?;
        println!(
            "imported {} and published runtime generation {}",
            source.display(),
            manifest.generation_id
        );
        return Ok(());
    }
    if command != "cook" && command != "init" {
        bail!(
            "unknown command {command:?}; expected `init`, `cook`, `create-demo`, `create-road-demo`, `export-vegetation` or `import-vegetation`"
        );
    }

    let project_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(default_project_path);
    let runtime_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(default_runtime_path);
    if arguments.next().is_some() {
        bail!("too many arguments; usage: yarra-world-cook {command} [PROJECT_DB] [RUNTIME_DB]");
    }

    if command == "init" && !project_path.exists() {
        yarra_world_cook::create_world_project(&project_path)
            .with_context(|| format!("could not initialize {}", project_path.display()))?;
        println!("created authoring database: {}", project_path.display());
    }
    if !project_path.exists() {
        bail!(
            "project {} does not exist; initialize a new world with `cargo run -p yarra-world-cook -- init` or supply an existing PROJECT_DB",
            project_path.display()
        );
    }
    let report = yarra_world_cook::cook_project_with_report(&project_path, &runtime_path)?;
    let manifest = report.manifest;
    println!(
        "published runtime generation {}: {}",
        manifest.generation_id,
        runtime_path.display()
    );
    let stats = report.stats;
    println!(
        "Cooked {} terrain cells and validated {} coverage-only cells",
        stats.terrain_cells, stats.coverage_only_cells
    );
    println!(
        "Peak source batch: {} height samples, {} mask bytes, {} manual objects, {} road spans",
        stats.peak_source_height_samples,
        stats.peak_source_mask_bytes,
        stats.peak_source_manual_objects,
        stats.peak_source_road_spans
    );
    println!(
        "Peak cell output: {} encoded bytes / {} decoded bytes (excludes catalog, SQLite and compiler scratch memory)",
        stats.peak_encoded_cell_bytes, stats.peak_decoded_cell_bytes
    );
    Ok(())
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn default_project_path() -> PathBuf {
    repository_root().join(world::DEFAULT_PROJECT_DATABASE)
}
fn default_runtime_path() -> PathBuf {
    repository_root()
        .join("assets")
        .join(world::DEFAULT_RUNTIME_DATABASE)
}
