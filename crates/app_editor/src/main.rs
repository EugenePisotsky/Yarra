//! Yarra editor executable composition root.

mod area_authoring;
mod atmosphere_authoring;

mod canopy;
mod domain_editing;
mod editing;
mod environment_paint;
mod journal;
mod listings;
mod project_store;
mod publication;
mod road_authoring;
mod saving;
mod shell;
mod startup;
mod tools;
mod vegetation_authoring;
mod worker;
mod workspaces;

fn main() -> std::process::ExitCode {
    match shell::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Cannot open editor:\n{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
