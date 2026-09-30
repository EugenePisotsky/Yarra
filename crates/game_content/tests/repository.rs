use game_types::{ItemDefinitionId, TextResourceId};
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
fn item_ids(path: &Path) -> Vec<AssetId> {
    open(path)
        .headers(AssetKind::Item)
        .unwrap()
        .into_iter()
        .map(|h| h.id)
        .collect()
}

#[test]
fn opening_reads_the_manifest_only_and_damage_is_reported_by_the_read_that_meets_it() {
    let temp = Temp::new();
    let path = build(&temp);
    let ids = item_ids(&path);
    let db = Connection::open(&path).unwrap();
    db.execute("UPDATE tool_scenario SET payload=x'00'", [])
        .unwrap();
    db.execute(
        "UPDATE assets SET hash=zeroblob(32) WHERE kind=?1 AND id=?2",
        params![ids[1].kind() as i64, ids[1].key()],
    )
    .unwrap();
    // The runtime never reads the tool scenario, and opening decodes no asset.
    let mut repo = open(&path);
    assert!(matches!(repo.read(&ids[0]).unwrap(), Asset::Item(_)));
    assert!(matches!(
        repo.read(&ids[1]),
        Err(ContentError::CorruptAsset { .. })
    ));
    assert!(matches!(
        repo.read(&AssetId::Item(ItemDefinitionId::new())),
        Err(ContentError::MissingAsset(_))
    ));
    // Items are always-loaded definitions, so a session refuses the damaged bundle.
    assert!(repo.core().is_err());
    assert!(LoadedProject::materialize_bundle_for_tools(path).is_err());
}

#[test]
fn a_record_under_the_wrong_identity_is_rejected_even_with_a_valid_checksum() {
    let temp = Temp::new();
    let path = build(&temp);
    let ids = item_ids(&path);
    let db = Connection::open(&path).unwrap();
    let payload: Vec<u8> = db
        .query_row(
            "SELECT payload FROM assets WHERE kind=2 AND id=?1",
            [ids[1].key()],
            |r| r.get(0),
        )
        .unwrap();
    db.execute(
        "UPDATE assets SET payload=?1,byte_len=?2,hash=?3 WHERE kind=2 AND id=?4",
        params![
            payload,
            payload.len() as i64,
            blake3::hash(&payload).as_bytes(),
            ids[0].key()
        ],
    )
    .unwrap();
    assert!(
        open(&path)
            .read(&ids[0])
            .err()
            .unwrap()
            .to_string()
            .contains("identity mismatch")
    );
}

#[test]
fn a_session_over_a_large_catalog_loads_definitions_once_and_graphs_on_demand() {
    let temp = Temp::new();
    let source = temp.source();
    let mut items: ItemCatalog = read(source.join("packages/core/items.ron"));
    for i in items.items.len()..5_000 {
        let mut item = items.items[0].clone();
        item.id = ItemDefinitionId((100_000 + i as u128).to_be_bytes());
        items.items.push(item);
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
    assert!(session.content().game.dialogues.is_empty());
    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }
    assert_eq!(session.content().game.dialogues.len(), 1);
    assert!(project.content().game.dialogues.len() > 1);
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
    let mut project: ResourceFile = read(source.join("packages/core/messages.ron"));
    let resource = project.contract.id;
    fs::rename(
        source.join(&project.locales[0].path),
        source.join("renamed.ftl"),
    )
    .unwrap();
    project.locales[0].path = "renamed.ftl".into();
    write(source.join("packages/core/messages.ron"), &project);
    let renamed = temp.0.join("renamed.sqlite");
    LoadedProject::load_directory(&source)
        .unwrap()
        .build(&renamed)
        .unwrap();
    assert_eq!(
        open(&original).manifest().publication_hash,
        open(&renamed).manifest().publication_hash
    );
    let mut repo = open(&original);
    assert!(
        matches!(repo.read(&AssetId::Text(resource)).unwrap(), Asset::Text(t) if t.id == resource)
    );
    assert!(matches!(
        repo.read(&AssetId::Text(TextResourceId::new())),
        Err(ContentError::MissingAsset(_))
    ));
    // Sessions never load text contracts; formatting asks for them separately.
    assert!(repo.core().unwrap().text.is_empty());
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
    let id = items.items[0].id;
    let original_weight = items.items[0].weight_grams;
    items.items[0].weight_grams += 10;
    write(source.join("packages/core/items.ron"), &items);
    let second = temp.0.join("second.sqlite");
    LoadedProject::load_directory(&source)
        .unwrap()
        .build(&second)
        .unwrap();
    let (a, b) = (open(&first), open(&second));
    assert_ne!(a.manifest().publication_hash, b.manifest().publication_hash);
    let weight = |repo: &ContentRepository| match repo.read(&AssetId::Item(id)).unwrap() {
        Asset::Item(item) => item.weight_grams,
        _ => unreachable!(),
    };
    assert_eq!(weight(&a), original_weight);
    assert_eq!(weight(&b), original_weight + 10);
}

#[test]
fn cli_inspect_reads_a_requested_item() {
    let temp = Temp::new();
    let path = build(&temp);
    let executable = env!("CARGO_BIN_EXE_yarra-game-content");
    let id = item_ids(&path)[0].key();
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
fn language_resource_declarations_reject_duplicate_or_orphaned_ids() {
    let temp = Temp::new();
    let source = temp.source();
    let mut resource: ResourceFile = read(source.join("packages/core/messages.ron"));
    let original = resource.clone();
    resource.locales.remove(0);
    write(source.join("packages/core/messages.ron"), &resource);
    assert!(LoadedProject::load_directory(&source).is_err());
    resource = original;
    let mut duplicate = resource.locales[0].clone();
    fs::copy(source.join(&duplicate.path), source.join("duplicate.ftl")).unwrap();
    duplicate.path = "duplicate.ftl".into();
    resource.locales.push(duplicate);
    write(source.join("packages/core/messages.ron"), &resource);
    assert!(
        LoadedProject::load_directory(&source)
            .err()
            .unwrap()
            .to_string()
            .contains("duplicate locale resource identity")
    );
}

#[test]
fn failure_mid_publication_cleans_staging_and_preserves_previous_generation() {
    let temp = Temp::new();
    let previous = build(&temp);
    let previous_hash = open(&previous).manifest().publication_hash;
    let source = temp.source();
    // Source FTL fits its 2 MiB authoring limit; its encoded record exceeds the publication limit.
    let path = source.join("packages/core/en.ftl");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n# ");
    text.push_str(&"x".repeat(MAX_ASSET_BYTES - text.len()));
    fs::write(path, text).unwrap();
    let project = LoadedProject::load_directory(source).unwrap();
    let failed = temp.0.join("failed.sqlite");
    assert!(
        project
            .build_language_pack("en", &failed)
            .err()
            .unwrap()
            .to_string()
            .contains("publication limit")
    );
    assert!(!failed.exists());
    assert_eq!(open(&previous).manifest().publication_hash, previous_hash);
    assert!(fs::read_dir(&temp.0).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".building")
    }));
}
