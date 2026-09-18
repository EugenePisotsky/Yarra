//! Validate the selected source/runtime pair before creating a renderer or a partial editor.
use std::path::Path;
use world_db::{ProjectReader, RuntimeReader};

pub(crate) fn validate_databases(project: &Path, runtime: &Path) -> Result<(), String> {
    let source = ProjectReader::open_read_only(project).map_err(|e| {
        format!("Project {}: {e}\nUse a compatible project via --project-db. For a new default world, run `cargo run -p yarra-world-cook -- init`. Existing projects are never reset automatically.", project.display())
    })?;
    source
        .read_environment_catalog()
        .map_err(|e| format!("Cannot load the environment in {}: {e}", project.display()))?;
    let cooked = RuntimeReader::open_immutable(runtime).map_err(|e| {
        format!("Runtime {}: {e}\nCook this project with `cargo run -p yarra-world-cook -- cook PROJECT_DB RUNTIME_DB`, or choose its matching runtime via --world-db.", runtime.display())
    })?;
    let source = source.manifest();
    let cooked = cooked.manifest();
    if source.default_world_space != cooked.default_world_space
        || source.world_spaces.len() != cooked.world_spaces.len()
        || source.world_spaces.iter().any(|s| {
            cooked
                .world_space(s.id)
                .is_none_or(|r| r.name != s.name || r.cell_size.to_bits() != s.cell_size.to_bits())
        })
    {
        return Err(format!(
            "Project {} and runtime {} describe different worlds. Cook the selected project or supply its matching --world-db.",
            project.display(),
            runtime.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_accepts_the_current_world_and_rejects_missing_or_mismatched_databases() {
        let folder = std::env::temp_dir().join(format!("yarra-startup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&folder).unwrap();
        let project = folder.join("world.project.sqlite");
        let runtime = folder.join("world.runtime.sqlite");
        assert!(
            validate_databases(&project, &runtime)
                .unwrap_err()
                .contains("-- init")
        );
        world_cook::create_world_project(&project).unwrap();
        assert!(
            validate_databases(&project, &runtime)
                .unwrap_err()
                .contains("-- cook")
        );
        world_cook::cook_project(&project, &runtime).unwrap();
        validate_databases(&project, &runtime).unwrap();
        let other = folder.join("other.project.sqlite");
        world_cook::create_demo_project(&other).unwrap();
        assert!(
            validate_databases(&other, &runtime)
                .unwrap_err()
                .contains("different worlds")
        );
        std::fs::remove_dir_all(folder).unwrap();
    }
}
