use std::path::PathBuf;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .unwrap_or_else(|| "cook".into());
    if command == "import-assets" {
        let source = PathBuf::from(arguments.next().context("expected CATALOG_RON")?);
        let project = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(default_project_path);
        if arguments.next().is_some() {
            bail!("usage: yarra-world-cook import-assets CATALOG_RON [PROJECT_DB]");
        }
        let catalog: world_db::AssetImportCatalog =
            ron::from_str(&std::fs::read_to_string(&source)?)?;
        catalog.validate()?;
        let asset_root = repository_root().join("assets");
        for asset in &catalog.assets {
            for uri in
                std::iter::once(&asset.source_uri).chain(asset.variants.iter().map(|v| &v.uri))
            {
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
        return Ok(());
    }
    if command == "create-demo"
        || command == "create-road-demo"
        || command == "create-mountain-fixture"
        || command == "create-hill-fixture"
    {
        let project = PathBuf::from(arguments.next().context("expected a new PROJECT_DB path")?);
        if arguments.next().is_some() {
            bail!("usage: yarra-world-cook {command} PROJECT_DB");
        }
        if command == "create-hill-fixture" {
            yarra_world_cook::create_hill_fixture(&project)?;
        } else if command == "create-mountain-fixture" {
            yarra_world_cook::create_mountain_fixture(&project)?;
        } else if command == "create-road-demo" {
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
        let options = CookOptions::parse(arguments)?;
        let materials = options.load_materials()?;
        let catalog: vegetation::VegetationCatalog =
            ron::from_str(&std::fs::read_to_string(&source)?)?;
        catalog.validate()?;
        world_db::ProjectWriter::open(&options.project)?.replace_vegetation_catalog(&catalog)?;
        let manifest = options.cook(materials.as_ref())?.manifest;
        println!(
            "imported {} and published runtime generation {}",
            source.display(),
            manifest.generation_id
        );
        return Ok(());
    }
    if command != "cook" && command != "init" {
        bail!(
            "unknown command {command:?}; expected `init`, `cook`, `create-demo`, `create-road-demo`, `create-mountain-fixture`, `create-hill-fixture`, `export-vegetation`, `import-vegetation` or `import-assets`"
        );
    }

    let options = CookOptions::parse(arguments)?;
    let materials = options.load_materials()?;
    let project_path = &options.project;
    let runtime_path = &options.runtime;

    if command == "init" && !project_path.exists() {
        yarra_world_cook::create_world_project(project_path)
            .with_context(|| format!("could not initialize {}", project_path.display()))?;
        println!("created authoring database: {}", project_path.display());
    }
    if !project_path.exists() {
        bail!(
            "project {} does not exist; initialize a new world with `cargo run -p yarra-world-cook -- init` or supply an existing PROJECT_DB",
            project_path.display()
        );
    }
    let report = options.cook(materials.as_ref())?;
    let manifest = report.manifest;
    println!(
        "published runtime generation {}: {}",
        manifest.generation_id,
        runtime_path.display()
    );
    if let Some(materials) = report.materials {
        println!(
            "Ground composites: {} tiles, {} GPU bytes/tile; peak {} filtering cores ({} pixels, excludes codecs/SQLite scratch)",
            materials.tiles,
            materials.tile_gpu_bytes,
            materials.peak_filter_cores,
            materials.peak_core_pixels
        );
    }
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

/// Normal publications always contain distant ground materials. Geometry-only
/// cooking is reserved for fixtures and explicit renderer diagnostics.
struct CookOptions {
    project: PathBuf,
    runtime: PathBuf,
    bake_root: Option<PathBuf>,
}
impl CookOptions {
    fn parse(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self> {
        let mut arguments = arguments.into_iter();
        let mut paths = Vec::new();
        let mut bake_root = Some(repository_root().join("assets"));
        let mut material_option = false;
        while let Some(argument) = arguments.next() {
            if argument == "--terrain-materials" || argument == "--geometry-only" {
                if material_option {
                    bail!("choose either --terrain-materials ASSET_ROOT or --geometry-only once");
                }
                material_option = true;
                bake_root = if argument == "--geometry-only" {
                    None
                } else {
                    let root = arguments
                        .next()
                        .context("expected ASSET_ROOT after --terrain-materials")?;
                    if root.to_string_lossy().starts_with("--") {
                        bail!("expected ASSET_ROOT after --terrain-materials");
                    }
                    Some(PathBuf::from(root))
                };
            } else if argument.to_string_lossy().starts_with("--") {
                bail!("unknown cook option {argument:?}");
            } else {
                paths.push(PathBuf::from(argument));
            }
        }
        if paths.len() > 2 {
            bail!(
                "expected [PROJECT_DB] [RUNTIME_DB] [--terrain-materials ASSET_ROOT | --geometry-only]"
            );
        }
        let mut paths = paths.into_iter();
        Ok(Self {
            project: paths.next().unwrap_or_else(default_project_path),
            runtime: paths.next().unwrap_or_else(default_runtime_path),
            bake_root,
        })
    }
    fn load_materials(&self) -> Result<Option<yarra_world_cook::TerrainBakeLibrary>> {
        self.bake_root.as_ref().map(|root| {
            yarra_world_cook::TerrainBakeLibrary::load(root).with_context(|| format!(
                "cannot load terrain bake assets from {}; prepare them with `python3 tools/prepare_terrain_bake.py` after preparing the terrain texture pack", root.display()
            ))
        }).transpose()
    }
    fn cook(
        &self,
        materials: Option<&yarra_world_cook::TerrainBakeLibrary>,
    ) -> Result<yarra_world_cook::CookReport> {
        match materials {
            Some(materials) => yarra_world_cook::cook_project_with_materials(
                &self.project,
                &self.runtime,
                materials,
            ),
            None => yarra_world_cook::cook_project_with_report(&self.project, &self.runtime),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(args: &[&str]) -> Result<CookOptions> {
        CookOptions::parse(args.iter().map(std::ffi::OsString::from))
    }
    #[test]
    fn normal_cooks_include_materials_with_default_or_explicit_paths() {
        let default = options(&[]).unwrap();
        assert_eq!(default.project, default_project_path());
        assert_eq!(default.runtime, default_runtime_path());
        assert_eq!(default.bake_root, Some(repository_root().join("assets")));
        let custom = options(&["source.sqlite", "runtime.sqlite"]).unwrap();
        assert_eq!(custom.project, PathBuf::from("source.sqlite"));
        assert_eq!(custom.runtime, PathBuf::from("runtime.sqlite"));
        assert_eq!(custom.bake_root, default.bake_root);
        assert_eq!(
            options(&["--terrain-materials", "custom-assets"])
                .unwrap()
                .bake_root,
            Some(PathBuf::from("custom-assets"))
        );
    }
    #[test]
    fn geometry_only_is_explicit_and_conflicting_options_are_rejected() {
        assert!(options(&["--geometry-only"]).unwrap().bake_root.is_none());
        for args in [
            vec!["--terrain-materials"],
            vec!["--terrain-materials", "--geometry-only"],
            vec!["--geometry-only", "--terrain-materials", "assets"],
            vec!["--terrain-materials", "assets", "--geometry-only"],
            vec!["--geometry-only", "--geometry-only"],
            vec!["--unknown"],
            vec!["one", "two", "three"],
        ] {
            assert!(options(&args).is_err(), "{args:?}");
        }
    }
}
