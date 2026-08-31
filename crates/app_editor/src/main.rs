//! Yarra editor executable composition root.

mod derived_jobs;
mod domain_editing;
mod editing;
mod journal;
mod navigation;
mod overview;
mod preview;
mod project_store;
mod publication;
mod saving;
mod shell;
mod tools;
mod vegetation_authoring;
mod workspaces;

fn main() {
    shell::run();
}
