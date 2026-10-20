use game_types::{DialogueId, ItemDefinitionId, TextResourceId};
use gameplay::inventory::ItemCatalog;
use gameplay::{ContentSource, GameSession};
use rusqlite::{Connection, params};
use std::{fs, path::Path};
use yarra_game_content::*;

mod support;

use support::{Temp, read, sample, write};

fn build(temp: &Temp) -> std::path::PathBuf {
    let path = temp.0.join("content.sqlite");
    LoadedProject::load_directory(sample())
        .unwrap()
        .build(&path)
        .unwrap();
    path
}

fn open(path: &Path) -> ContentRepository {
    ContentRepository::open(path).unwrap()
}

fn dialogue_ids(path: &Path) -> Vec<String> {
    Connection::open(path)
        .unwrap()
        .prepare("SELECT id FROM dialogues ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

#[test]
fn opening_reads_the_manifest_only_and_damage_is_reported_by_the_read_that_meets_it() {
    let temp = Temp::new();
    let path = build(&temp);
    let ids = dialogue_ids(&path);
    let db = Connection::open(&path).unwrap();
    db.execute("UPDATE core SET payload=x'00'", []).unwrap();
    db.execute(
        "UPDATE dialogues SET payload=x'00' WHERE id=?1",
        params![ids[1]],
    )
    .unwrap();

    // Opening decodes nothing but the manifest; each read meets its own damage.
    let mut repo = open(&path);
    let first = DialogueId::try_from(ids[0].clone()).unwrap();
    assert_eq!(repo.read_dialogue(first).unwrap().id, first);
    assert!(
        repo.read_dialogue(DialogueId::try_from(ids[1].clone()).unwrap())
            .is_err()
    );
    assert!(matches!(
        repo.read_dialogue(DialogueId::random()),
        Err(ContentError::NotInBundle(_))
    ));

    // The core is what a session reads up front, so a session refuses the damaged bundle.
    assert!(repo.core().is_err());
}

#[test]
fn a_graph_under_another_identity_is_refused_when_it_is_read() {
    let temp = Temp::new();
    let path = build(&temp);
    let ids = dialogue_ids(&path);
    let db = Connection::open(&path).unwrap();
    let payload: Vec<u8> = db
        .query_row(
            "SELECT payload FROM dialogues WHERE id=?1",
            [&ids[1]],
            |r| r.get(0),
        )
        .unwrap();
    db.execute(
        "UPDATE dialogues SET payload=?1 WHERE id=?2",
        params![payload, ids[0]],
    )
    .unwrap();
    let error = open(&path).check().err().unwrap().to_string();
    assert!(error.contains("dialogue contract mismatch"), "{error}");
}

#[test]
fn a_session_over_a_large_catalog_loads_definitions_once_and_graphs_on_demand() {
    let temp = Temp::new();
    let source = temp.source();
    let mut items: ItemCatalog = read(source.join("packages/core/items.ron"));

    for i in items.items.len()..5_000 {
        let mut item = items.items.values().next().unwrap().clone();
        item.id = ItemDefinitionId((100_000 + i as u128).to_be_bytes());
        items.items.insert(item.id, item);
    }

    write(source.join("packages/core/items.ron"), &items);
    let project = LoadedProject::load_directory(&source).unwrap();
    let bundle = temp.0.join("large.sqlite");
    project.build(&bundle).unwrap();

    // The runtime cannot fall back to reading authored files.
    let seed = project.start().unwrap().into_state();
    fs::remove_dir_all(source).unwrap();
    let mut session = GameSession::new(open(&bundle), seed).unwrap();
    assert_eq!(session.content().items.items.len(), 5_000);
    assert!(session.content().graphs.loaded() == 0);

    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }

    assert_eq!(session.content().graphs.loaded(), 1);
    assert!(project.authored().dialogues.len() > 1);
}

#[test]
fn text_contracts_have_stable_ids_and_are_read_explicitly() {
    let temp = Temp::new();
    let source = temp.source();
    let original = temp.0.join("original.sqlite");
    LoadedProject::load_directory(&source)
        .unwrap()
        .build(&original)
        .unwrap();

    // Where a package keeps its text is not part of any identity.
    let resource = TextResourceId::named("core");
    fs::create_dir(source.join("packages/core/text")).unwrap();

    for locale in ["en", "uk"] {
        let file = format!("packages/core/{locale}.ftl");
        fs::rename(
            source.join(&file),
            source.join(format!("packages/core/text/{locale}.ftl")),
        )
        .unwrap();
    }

    let renamed = temp.0.join("renamed.sqlite");
    LoadedProject::load_directory(&source)
        .unwrap()
        .build(&renamed)
        .unwrap();
    assert_eq!(open(&original).manifest(), open(&renamed).manifest());
    let repo = open(&original);
    assert_eq!(repo.read_text(resource).unwrap().id, resource);
    assert!(matches!(
        repo.read_text(TextResourceId::random()),
        Err(ContentError::NotInBundle(_))
    ));
}

#[test]
fn separate_publications_are_read_independently() {
    let temp = Temp::new();
    let source = temp.source();
    let first = temp.0.join("first.sqlite");
    LoadedProject::load_directory(&source)
        .unwrap()
        .build(&first)
        .unwrap();
    let mut items: ItemCatalog = read(source.join("packages/core/items.ron"));
    let id = *items.items.keys().next().unwrap();
    let original_weight = items.items[&id].weight_grams;
    items.items.get_mut(&id).unwrap().weight_grams += 10;
    write(source.join("packages/core/items.ron"), &items);
    let second = temp.0.join("second.sqlite");
    LoadedProject::load_directory(&source)
        .unwrap()
        .build(&second)
        .unwrap();
    let (a, b) = (open(&first), open(&second));
    assert_ne!(a.manifest().fingerprint, b.manifest().fingerprint);
    let weight = |repo: &ContentRepository| {
        let core = repo.read_core().unwrap();
        core.items.item(id).unwrap().weight_grams
    };
    assert_eq!(weight(&a), original_weight);
    assert_eq!(weight(&b), original_weight + 10);
}

#[test]
fn cli_inspect_reads_a_requested_item() {
    let temp = Temp::new();
    let path = build(&temp);
    let executable = env!("CARGO_BIN_EXE_yarra-game-content");
    let project = LoadedProject::load_directory(sample()).unwrap();
    let id = project.content().items.items.keys().next().unwrap().raw();
    let output = std::process::Command::new(executable)
        .arg("inspect")
        .arg(&path)
        .args(["--item", &id])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("Publication: demo") && text.contains("Item "),
        "{text}"
    );
}

#[test]
fn source_and_bundle_versions_are_replaced_without_compatibility_loading() {
    let temp = Temp::new();
    let source = temp.source();
    let mut project: ProjectFile = read(source.join("project.ron"));
    project.format = 1;
    write(source.join("project.ron"), &project);
    assert!(matches!(
        LoadedProject::load_directory(source),
        Err(ContentError::SourceVersion(1))
    ));
    let path = build(&temp);
    let db = Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 1).unwrap();
    assert!(matches!(
        ContentRepository::open(path),
        Err(ContentError::BundleVersion(1))
    ));
}

#[test]
fn a_package_needs_its_source_text_and_each_message_once() {
    let temp = Temp::new();
    let source = temp.source();
    let english = source.join("packages/core/en.ftl");
    let original = fs::read(&english).unwrap();
    fs::remove_file(&english).unwrap();
    let error = LoadedProject::load_directory(&source)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("translations but no en.ftl"), "{error}");

    // Several files of one locale are joined; a message defined in two is an error.
    fs::write(&english, &original).unwrap();
    fs::create_dir(source.join("packages/core/more")).unwrap();
    fs::write(source.join("packages/core/more/en.ftl"), &original).unwrap();
    let error = LoadedProject::load_directory(&source)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("duplicate Fluent key"), "{error}");
}

#[test]
fn a_failed_publication_leaves_nothing_behind_and_the_previous_one_as_it_was() {
    let temp = Temp::new();
    let previous = build(&temp);
    let manifest = open(&previous).manifest().clone();
    let project = LoadedProject::load_directory(sample()).unwrap();

    // The pack is written in full, then refused where a file already is.
    assert!(project.build_language_pack("en", &previous).is_err());
    assert_eq!(open(&previous).manifest(), &manifest);
    assert!(fs::read_dir(&temp.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".building")
    }));
}
