use std::path::PathBuf;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .unwrap_or_else(|| "demo".into());
    if command == "sync-demo-vegetation" {
        let project_path = arguments
            .next()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("content/demo.project.sqlite"));
        if arguments.next().is_some() {
            bail!("too many arguments; usage: yarra-world-cook sync-demo-vegetation [PROJECT_DB]");
        }
        if !project_path.exists() {
            yarra_world_cook::create_demo_project(&project_path)
                .with_context(|| format!("could not initialize {}", project_path.display()))?;
        } else {
            let mut writer = world_db::ProjectWriter::open(&project_path)
                .with_context(|| format!("could not open {}", project_path.display()))?;
            writer
                .replace_vegetation_catalog(&vegetation::fixtures::reference_catalog())
                .with_context(|| {
                    format!(
                        "could not synchronize the demo vegetation catalog in {}",
                        project_path.display()
                    )
                })?;
        }
        println!(
            "synchronized code-authored vegetation catalog: {}",
            project_path.display()
        );
        return Ok(());
    }
    if command != "demo" {
        bail!("unknown command {command:?}; expected `demo` or `sync-demo-vegetation`");
    }

    let project_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("content/demo.project.sqlite"));
    let runtime_path = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/generated/demo.runtime.sqlite"));
    if arguments.next().is_some() {
        bail!("too many arguments; usage: yarra-world-cook demo [PROJECT_DB] [RUNTIME_DB]");
    }

    if !project_path.exists() {
        yarra_world_cook::create_demo_project(&project_path)
            .with_context(|| format!("could not initialize {}", project_path.display()))?;
        println!("created authoring database: {}", project_path.display());
    } else if world_db::migrate_project_database(&project_path)
        .with_context(|| format!("could not migrate {}", project_path.display()))?
    {
        println!("migrated authoring database: {}", project_path.display());
    }
    let manifest = yarra_world_cook::cook_project(&project_path, &runtime_path)?;
    println!(
        "published runtime generation {}: {}",
        manifest.generation_id,
        runtime_path.display()
    );
    Ok(())
}
