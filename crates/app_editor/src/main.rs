//! Yarra editor executable composition root.

mod catalog_editing;
mod derived_jobs;
mod domain_editing;
mod editing;
mod ground_cover_catalog;
mod ground_cover_editing;
mod ground_cover_preview;
mod journal;
mod navigation;
mod overview;
mod preview;
mod project_store;
mod publication;
mod saving;
mod shell;
mod tools;
mod workspaces;

fn main() {
    shell::run();
}
