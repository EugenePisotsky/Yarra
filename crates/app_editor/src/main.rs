//! Yarra editor executable composition root.

mod atmosphere_authoring;

mod canopy;
mod derived_jobs;
mod domain_editing;
mod editing;
mod environment_paint;
mod journal;
mod navigation;
mod overview;
mod preview;
mod project_store;
mod publication;
mod road_authoring;
mod saving;
mod shell;
mod startup;
mod tools;
mod vegetation_authoring;
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
