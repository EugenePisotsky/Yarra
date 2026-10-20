use std::{ffi::OsString, path::PathBuf};

use anyhow::{Context, Result, bail};

const USAGE: &str = "expected `cook`, `import-heightfield`, `import-roads`, `import-grass`, \
                     `import-assets`, `create-demo` or `create-road-demo`";

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .unwrap_or_else(|| "cook".into());

    match command.as_str() {
        "cook" => {
            let options = CookOptions::parse(arguments)?;

            if !options.cook {
                bail!("`cook --no-cook` does nothing");
            }

            cook(&options)
        }
        "import-heightfield" => import_heightfield(arguments),
        "import-roads" => import_roads(arguments),
        "import-grass" => import_grass(arguments),
        "import-assets" => import_assets(arguments),
        "create-demo" | "create-road-demo" => create_demo(&command, arguments),
        _ => bail!("unknown command {command:?}; {USAGE}"),
    }
}

/// Imports a heightfield as the default world's terrain, creating the project if needed.
fn import_heightfield(mut arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let manifest = PathBuf::from(
        arguments
            .next()
            .context("expected a heightfield MANIFEST_JSON")?,
    );
    let options = CookOptions::parse(arguments)?;
    let project = &options.project;
    let report = yarra_world_cook::import_heightfield(&manifest, project)?;
    let stats = report.stats;
    println!(
        "{} {} in {:.1} s: {} cells ({} added, {} changed, {} removed, {} flat), heights {} to {} m",
        if report.created { "Created" } else { "Updated" },
        project.display(),
        report.seconds,
        stats.cells,
        stats.added,
        stats.changed,
        stats.removed,
        report.flat_cells,
        report.bounds[0],
        report.bounds[1]
    );

    if !report.unused_masks.is_empty() {
        println!(
            "No layer reads the masks {}; give a layer one in the editor to use it",
            report.unused_masks.join(", ")
        );
    }

    println!("Start views: {}", project.with_extension("views").display());
    cook_after_import(&options)
}

fn import_roads(mut arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let source = PathBuf::from(arguments.next().context("expected a roads RON")?);
    let options = CookOptions::parse(arguments)?;
    let report =
        yarra_world_cook::import_roads(&std::fs::read_to_string(&source)?, &options.project)?;
    println!(
        "imported {} road styles and {} roads ({} knots) from {}",
        report.styles,
        report.roads,
        report.knots,
        source.display()
    );
    cook_after_import(&options)
}

fn import_grass(mut arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let source = PathBuf::from(arguments.next().context("expected a grass pack RON")?);
    let options = CookOptions::parse(arguments)?;
    let catalog = yarra_world_cook::grass_catalog(&std::fs::read_to_string(&source)?)?;
    world_db::ProjectWriter::open(&options.project)?.replace_vegetation_catalog(&catalog)?;
    println!("imported {}", source.display());
    cook_after_import(&options)
}

fn import_assets(mut arguments: impl Iterator<Item = OsString>) -> Result<()> {
    let source = PathBuf::from(arguments.next().context("expected CATALOG_RON")?);
    let project = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(default_project_path);

    if arguments.next().is_some() {
        bail!("usage: yarra-world-cook import-assets CATALOG_RON [PROJECT_DB]");
    }

    let catalog: world_db::AssetImportCatalog = ron::from_str(&std::fs::read_to_string(&source)?)?;
    catalog.validate()?;
    let asset_root = repository_root().join("assets");

    for asset in &catalog.assets {
        for uri in std::iter::once(&asset.source_uri).chain(asset.variants.iter().map(|v| &v.uri)) {
            if !asset_root.join(uri).is_file() {
                bail!(
                    "missing local asset {}; run the pack exporter first",
                    asset_root.join(uri).display()
                );
            }
        }
    }

    world_db::ProjectWriter::open(&project)?.import_assets(&catalog)?;
    println!(
        "Registered {} assets in {}. Reopen the editor to refresh its palette; cook after placing or scattering them.",
        catalog.assets.len(),
        project.display()
    );
    Ok(())
}

fn create_demo(command: &str, mut arguments: impl Iterator<Item = OsString>) -> Result<()> {
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
    Ok(())
}

/// Publishes what an import changed, unless asked not to: a build of several imports cooks
/// once, at the end.
fn cook_after_import(options: &CookOptions) -> Result<()> {
    if options.cook {
        cook(options)
    } else {
        println!("Not cooked; cook to publish the change");
        Ok(())
    }
}

fn cook(options: &CookOptions) -> Result<()> {
    if !options.project.exists() {
        bail!(
            "project {} does not exist; build Reteya with `content/reteya/scripts/build_world.sh` (see content/reteya/README.md) or supply an existing PROJECT_DB",
            options.project.display()
        );
    }

    let report = if options.full {
        yarra_world_cook::cook_project_fresh(&options.project, &options.runtime)
    } else {
        yarra_world_cook::cook_project_with_report(&options.project, &options.runtime)
    }?;
    println!(
        "published runtime generation {}: {}",
        report.manifest.generation_id,
        options.runtime.display()
    );
    let stats = report.stats;
    println!(
        "Cook time: cells {:.1} s, hierarchy/controls/validation {:.1} s",
        stats.cell_seconds, stats.publish_seconds
    );
    println!(
        "Cooked {} {}terrain cells and validated {} coverage-only cells",
        stats.terrain_cells,
        if stats.incremental { "changed " } else { "" },
        stats.coverage_only_cells
    );

    if stats.incremental {
        println!(
            "Terrain hierarchy: updated for {} cells with changed ground in {:.1} s",
            stats.changed_ground_cells, stats.hierarchy_seconds
        );
    } else {
        println!(
            "Terrain hierarchy: built in {:.1} s",
            stats.hierarchy_seconds
        );
    }

    println!(
        "Far objects: {} impostor-drawn objects in {} blocks",
        stats.far_objects, stats.far_object_blocks
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

struct CookOptions {
    project: PathBuf,
    runtime: PathBuf,
    /// Recompile every cell instead of continuing the existing runtime.
    full: bool,
    /// Whether an import publishes what it changed.
    cook: bool,
}

impl CookOptions {
    fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self> {
        let mut paths = Vec::new();
        let mut full = false;
        let mut cook = true;

        for argument in arguments {
            if argument == "--full" {
                full = true;
            } else if argument == "--no-cook" {
                cook = false;
            } else if argument.to_string_lossy().starts_with("--") {
                bail!("unknown cook option {argument:?}");
            } else {
                paths.push(PathBuf::from(argument));
            }
        }

        if paths.len() > 2 {
            bail!("expected [PROJECT_DB] [RUNTIME_DB] [--full] [--no-cook]");
        }

        let mut paths = paths.into_iter();
        Ok(Self {
            project: paths.next().unwrap_or_else(default_project_path),
            runtime: paths.next().unwrap_or_else(default_runtime_path),
            full,
            cook,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(args: &[&str]) -> Result<CookOptions> {
        CookOptions::parse(args.iter().map(OsString::from))
    }

    #[test]
    fn cooks_take_default_or_explicit_paths_and_reject_unknown_options() {
        let default = options(&[]).unwrap();
        assert_eq!(default.project, default_project_path());
        assert_eq!(default.runtime, default_runtime_path());
        assert!(!default.full && default.cook);
        let custom = options(&["source.sqlite", "runtime.sqlite", "--full", "--no-cook"]).unwrap();
        assert_eq!(custom.project, PathBuf::from("source.sqlite"));
        assert_eq!(custom.runtime, PathBuf::from("runtime.sqlite"));
        assert!(custom.full && !custom.cook);

        for args in [vec!["--unknown"], vec!["one", "two", "three"]] {
            assert!(options(&args).is_err(), "{args:?}");
        }
    }
}
