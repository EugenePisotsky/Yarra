//! Every project shader parses as WESL, and every variant in `entries.ron` composes and validates.
use yarra_shader_check::{Shaders, entries};

#[test]
fn every_entry_variant_composes_and_validates() {
    let shaders = Shaders::get();
    let jobs = entries::load(shaders).unwrap();
    assert!(!jobs.is_empty());
    let failures: Vec<String> = jobs
        .iter()
        .filter_map(|(shader, defs)| {
            shaders
                .check(shader, defs)
                .err()
                .map(|error| format!("{shader} {defs:?}\n{error}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn every_shader_file_parses() {
    let files = Shaders::get().parse_all();
    assert!(!files.is_empty());
    let failures: Vec<String> = files
        .iter()
        .filter_map(|(file, error)| error.as_ref().map(|e| format!("{}: {e}", file.display())))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
