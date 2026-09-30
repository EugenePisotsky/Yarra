use game_types::{ItemDefinitionId, TextResourceId};
use inventory::ItemCatalog;
use rusqlite::{Connection, params};
use std::{collections::BTreeSet, fs, path::Path};
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
    ContentRepository::open(path, RepositoryLimits::default()).unwrap()
}
fn item_ids(path: &Path) -> Vec<AssetId> {
    open(path)
        .list(AssetKind::Item, None, 128)
        .unwrap()
        .into_iter()
        .map(|h| h.id)
        .collect()
}
fn large_project(temp: &Temp, count: usize) -> std::path::PathBuf {
    let source = temp.source();
    let mut items: ItemCatalog = read(source.join("packages/core/items.ron"));
    for i in items.items.len()..count {
        let mut item = items.items[0].clone();
        item.id = ItemDefinitionId((100_000 + i as u128).to_be_bytes());
        item.key = format!("generated_{i:05}");
        items.items.push(item);
    }
    write(source.join("packages/core/items.ron"), &items);
    let output = temp.0.join("large.sqlite");
    LoadedProject::load_directory(&source)
        .unwrap()
        .build(&output)
        .unwrap();
    // Runtime cannot accidentally fall back to reading authored files.
    fs::remove_dir_all(source).unwrap();
    output
}

#[test]
fn fixed_inventory_working_set_is_independent_of_catalog_size() {
    let small = Temp::new();
    let large = Temp::new();
    let small_path = large_project(&small, 16);
    let large_path = large_project(&large, 10_000);
    let authored = LoadedProject::load_directory(sample()).unwrap();
    let roots: Vec<_> = authored.content().items.items[..2]
        .iter()
        .map(|i| AssetId::Item(i.id))
        .collect();
    let mut small = open(&small_path);
    let mut large = open(&large_path);
    assert_eq!(small.stats(), RepositoryStats::default());
    assert_eq!(large.stats(), RepositoryStats::default());
    let small_set = small.load(&roots).unwrap();
    let large_set = large.load(&roots).unwrap();
    assert_eq!(small_set.len(), 5); // two definitions, two categories and the bounded common rules
    assert_eq!(small_set.len(), large_set.len());
    assert_eq!(small.stats(), large.stats());
    assert_eq!(large.stats().decoded_assets, 5);
    assert_eq!(large.stats().payload_queries, 5);
    assert!(large.stats().payload_bytes_read < 16 * 1024);
    assert!(
        large_set
            .iter()
            .all(|(_, a)| !matches!(a, Asset::Text(_) | Asset::Dialogue(_)))
    );
    eprintln!(
        "16 vs 10,000 definitions, same inventory: {:?}",
        large.stats()
    );
    let warm = large.load(&roots).unwrap();
    assert_eq!(warm.len(), 5);
    assert_eq!(large.stats().payload_queries, 5);
    assert_eq!(large.stats().cache_hits, 5);
}

#[test]
fn runtime_open_ignores_scenario_and_unrequested_corruption() {
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
    let mut repo = open(&path);
    assert_eq!(repo.stats().decoded_assets, 0);
    repo.load(&ids[..1]).unwrap();
    assert!(matches!(
        repo.load(&ids[1..2]),
        Err(ContentError::CorruptAsset { .. })
    ));
    assert!(LoadedProject::materialize_bundle_for_tools(path).is_err());
}

#[test]
fn dialogue_resolves_only_its_mechanical_dependency_closure() {
    let temp = Temp::new();
    let path = build(&temp);
    let mut repo = open(&path);
    let dialogue = repo.list(AssetKind::Dialogue, None, 1).unwrap()[0]
        .id
        .clone();
    assert_eq!(repo.stats().decoded_assets, 0);
    let set = repo.load(&[dialogue.clone(), dialogue.clone()]).unwrap();
    let kinds: BTreeSet<_> = set.iter().map(|(id, _)| id.kind()).collect();
    assert!(kinds.contains(&AssetKind::Action) && kinds.contains(&AssetKind::Condition));
    assert!(kinds.contains(&AssetKind::Text) && kinds.contains(&AssetKind::DialogueContract));
    assert!(!kinds.contains(&AssetKind::Actor));
    assert!(repo.stats().decoded_assets < 20);
    if let AssetId::Dialogue(id) = dialogue {
        assert_eq!(set.dialogue(id).unwrap().id, id);
    }
}

#[test]
fn paging_is_bounded_and_indexed_without_loading_payloads() {
    let temp = Temp::new();
    let path = build(&temp);
    let mut repo = open(&path);
    let mut cursor = None;
    let mut ids = BTreeSet::new();
    loop {
        let page = repo.list(AssetKind::Item, cursor.as_deref(), 1).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(ids.insert(page[0].id.clone()));
        cursor = Some(page[0].id.key());
    }
    assert_eq!(ids.len(), 3);
    assert_eq!(repo.stats().decoded_assets, 0);
    assert!(repo.list(AssetKind::Item, None, 129).is_err());
    let db = Connection::open(&path).unwrap();
    for query in [
        "EXPLAIN QUERY PLAN SELECT position,byte_len,hash FROM assets WHERE kind=2 AND id='x'",
        "EXPLAIN QUERY PLAN SELECT id,position,byte_len,hash FROM assets WHERE kind=2 AND id>'x' ORDER BY id LIMIT 128",
        "EXPLAIN QUERY PLAN SELECT dependency FROM asset_dependencies WHERE kind=2 AND id='x' ORDER BY dependency LIMIT 257",
    ] {
        let plan = db
            .prepare(query)
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            plan.contains("SEARCH") && plan.contains("PRIMARY KEY"),
            "{plan}"
        );
        assert!(
            !plan.contains("SCAN") && !plan.contains("TEMP B-TREE"),
            "{plan}"
        );
    }
}

#[test]
fn leases_pin_cache_entries_and_release_them_for_eviction() {
    let temp = Temp::new();
    let path = build(&temp);
    let ids = item_ids(&path);
    let limits = RepositoryLimits {
        max_cached_assets: 3,
        ..Default::default()
    };
    let mut repo = ContentRepository::open(&path, limits).unwrap();
    let held = repo.load(&ids[..1]).unwrap();
    assert_eq!(held.len(), 3);
    assert!(matches!(
        repo.load(&ids[1..2]),
        Err(ContentError::Budget(_))
    ));
    assert!(held.get(&ids[0]).is_ok());
    assert_eq!(repo.stats().cached_assets, 3);
    drop(held);
    let next = repo.load(&ids[1..2]).unwrap();
    assert_eq!(next.len(), 3);
    assert!(repo.stats().cache_evictions > 0);
    assert!(
        repo.stats().peak_charged_cache_bytes <= RepositoryLimits::default().cache_charge_bytes
    );
}

#[test]
fn root_closure_payload_and_cache_byte_budgets_are_enforced() {
    let temp = Temp::new();
    let path = build(&temp);
    let ids = item_ids(&path);
    for limits in [
        RepositoryLimits {
            max_roots: 1,
            ..Default::default()
        },
        RepositoryLimits {
            max_resolved_assets: 1,
            ..Default::default()
        },
        RepositoryLimits {
            max_asset_bytes: 16,
            ..Default::default()
        },
        RepositoryLimits {
            cache_charge_bytes: 4096,
            ..Default::default()
        },
    ] {
        let budget = limits.cache_charge_bytes;
        let mut repo = ContentRepository::open(&path, limits).unwrap();
        assert!(matches!(repo.load(&ids[..2]), Err(ContentError::Budget(_))));
        assert!(repo.stats().peak_charged_cache_bytes <= budget);
    }
}

#[test]
fn missing_dependencies_and_tampered_dependency_indexes_fail_explicitly() {
    let temp = Temp::new();
    let path = build(&temp);
    let id = item_ids(&path)[0].clone();
    let db = Connection::open(&path).unwrap();
    db.pragma_update(None, "foreign_keys", false).unwrap();
    db.execute("DELETE FROM assets WHERE kind=4", []).unwrap();
    let mut repo = open(&path);
    assert!(matches!(
        repo.load(std::slice::from_ref(&id)),
        Err(ContentError::MissingAsset(AssetId::Rules))
    ));
    drop(repo);
    db.execute(
        "DELETE FROM asset_dependencies WHERE kind=?1 AND id=?2",
        params![id.kind() as i64, id.key()],
    )
    .unwrap();
    assert!(matches!(
        open(&path).load(&[id]),
        Err(ContentError::CorruptAsset { .. })
    ));
}

#[test]
fn identity_and_length_checks_precede_returning_corrupt_assets() {
    let temp = Temp::new();
    let path = build(&temp);
    let ids = item_ids(&path);
    let db = Connection::open(&path).unwrap();
    // A correctly checksummed payload under the wrong ID must still fail.
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
            .load(&ids[..1])
            .err()
            .unwrap()
            .to_string()
            .contains("identity mismatch")
    );
    db.execute_batch(
        "PRAGMA ignore_check_constraints=ON; UPDATE assets SET byte_len=1 WHERE kind=2;",
    )
    .unwrap();
    assert!(
        open(&path)
            .load(&ids[1..2])
            .err()
            .unwrap()
            .to_string()
            .contains("length mismatch")
    );
}

#[test]
fn text_contracts_have_stable_ids_and_are_loaded_explicitly() {
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
    let text_id = AssetId::Text(resource);
    let mut repo = open(&original);
    let set = repo.load(std::slice::from_ref(&text_id)).unwrap();
    assert_eq!(set.len(), 1);
    assert!(matches!(set.get(&text_id).unwrap(), Asset::Text(t) if t.id == resource));
    assert_eq!(repo.stats().decoded_assets, 1);
    assert!(matches!(
        repo.load(&[AssetId::Text(TextResourceId::new())]),
        Err(ContentError::MissingAsset(_))
    ));
}

#[test]
fn separate_publications_never_share_stale_cached_assets() {
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
    let mut a = open(&first);
    let mut b = open(&second);
    assert_ne!(a.manifest().publication_hash, b.manifest().publication_hash);
    let a_set = a.load(&[AssetId::Item(id)]).unwrap();
    let b_set = b.load(&[AssetId::Item(id)]).unwrap();
    assert_ne!(a_set.publication_hash(), b_set.publication_hash());
    assert_eq!(a_set.item(id).unwrap().weight_grams, original_weight);
    assert_eq!(b_set.item(id).unwrap().weight_grams, original_weight + 10);
    assert_eq!(a_set.item(id).unwrap().weight_grams, original_weight);
}

#[test]
fn cli_inspect_opens_without_decoding_and_fetches_a_requested_item() {
    let temp = Temp::new();
    let path = build(&temp);
    let executable = env!("CARGO_BIN_EXE_yarra-game-content");
    let output = std::process::Command::new(executable)
        .arg("inspect")
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("decoded_assets: 0"));
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
    assert!(String::from_utf8_lossy(&output.stdout).contains("decoded_assets: 3"));
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
        ContentRepository::open(path, Default::default()),
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
