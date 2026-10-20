//! Yarra editor executable composition root.

mod activity;
mod domain_editing;
mod editing;
mod grass;
mod journal;
mod listings;
mod project_store;
mod publication;
mod saving;
mod shell;
mod startup;
mod tools;
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
