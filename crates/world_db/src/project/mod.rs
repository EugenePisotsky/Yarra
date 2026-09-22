//! Mutable source databases: bounded editor access and explicit whole-document import/export.
mod document;
pub(crate) mod query;
mod reader;
mod writer;

pub use document::{read_project_database, write_project_database};
pub use reader::ProjectReader;
pub use writer::ProjectWriter;
