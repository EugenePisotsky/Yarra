use actors::ActorRole;
use game_types::{CategoryId, Key, TextRef};
use inventory::ItemCatalog;
use localization::Arguments;
use rusqlite::Connection;
use save::{SaveDirectory, SaveSlot};
use std::{fs, path::Path, process::Command};
use yarra_game_content::*;
mod support;
use support::{Temp, read, sample, write};
fn failure(root: &Path) -> String {
    match LoadedProject::load_directory(root) {
        Ok(_) => panic!("invalid project was accepted"),
        Err(error) => error.to_string(),
    }
}
fn player_health(project: &LoadedProject) -> u32 {
    project
        .run_scenario()
        .unwrap()
        .store()
        .export_for_tools(100_000)
        .unwrap()
        .actors
        .iter()
        .find(|a| a.role == ActorRole::Player)
        .unwrap()
        .health
}
#[test]
fn authored_source_and_sqlite_bundle_use_the_same_validated_content() {
    let temp = Temp::new();
    let project = LoadedProject::load_directory(sample()).unwrap();
    assert!(project.warnings().is_empty());
    assert_eq!(player_health(&project), 75);
    let source_id = project.content().fingerprint().unwrap();
    let file = temp.0.join("content.sqlite");
    project.build(&file).unwrap();
    let en = temp.0.join("en.sqlite");
    let uk = temp.0.join("uk.sqlite");
    project.build_language_pack("en", &en).unwrap();
    project.build_language_pack("uk", &uk).unwrap();
    let bundle = LoadedProject::materialize_with_languages_for_tools(&file, &[en, uk]).unwrap();
    assert_eq!(bundle.content(), project.content());
    assert_eq!(bundle.scenario(), project.scenario());
    assert_eq!(bundle.content().fingerprint().unwrap(), source_id);
    assert_eq!(player_health(&bundle), 75);
    let name = &bundle
        .content()
        .items
        .item_by_key("healing_potion")
        .unwrap()
        .name;
    assert_eq!(
        bundle
            .localization()
            .format("uk", name, &Arguments::new())
            .unwrap()
            .value,
        "Лікувальне зілля"
    );
    let session = bundle.run_scenario().unwrap();
    let saves = SaveDirectory::new(temp.0.join("saves"), 2).unwrap();
    saves.quicksave(&session).unwrap();
    assert_eq!(
        saves
            .load(
                SaveSlot::Quick,
                gameplay::ToolContent::new(project.content().clone()).unwrap(),
                temp.0.join("working.sqlite")
            )
            .unwrap()
            .store()
            .export_for_tools(100_000)
            .unwrap(),
        session.store().export_for_tools(100_000).unwrap()
    );
    let first = project.start().unwrap();
    let second = project.start().unwrap();
    assert_ne!(
        first.store().export_for_tools(100_000).unwrap().playthrough,
        second
            .store()
            .export_for_tools(100_000)
            .unwrap()
            .playthrough
    );
    assert_ne!(
        first.store().export_for_tools(100_000).unwrap().inventories[0].entries[0].id,
        second
            .store()
            .export_for_tools(100_000)
            .unwrap()
            .inventories[0]
            .entries[0]
            .id
    );
    assert_eq!(
        first.store().export_for_tools(100_000).unwrap().actors[0].id,
        second.store().export_for_tools(100_000).unwrap().actors[0].id
    );
}
#[test]
fn editing_data_changes_mechanics_without_changing_rust_or_old_bundles() {
    let temp = Temp::new();
    let root = temp.source();
    let original = LoadedProject::load_directory(&root).unwrap();
    let path = temp.0.join("pinned.sqlite");
    original.build(&path).unwrap();
    let mut items: ItemCatalog = read(root.join("packages/core/items.ron"));
    items.items[0].mechanics.on_use = vec![rules::Effect::Heal(40)];
    write(root.join("packages/core/items.ron"), &items);
    let changed = LoadedProject::load_directory(&root).unwrap();
    assert_eq!(player_health(&changed), 90);
    assert_ne!(
        changed.content().fingerprint().unwrap(),
        original.content().fingerprint().unwrap()
    );
    assert_eq!(
        player_health(&LoadedProject::materialize_bundle_for_tools(path).unwrap()),
        75
    );
    let mut templates: Vec<actors::ActorTemplate> = read(root.join("packages/core/actors.ron"));
    templates[0].base.insert(Key::new("strength").unwrap(), 17);
    write(root.join("packages/core/actors.ron"), &templates);
    let project = LoadedProject::load_directory(&root).unwrap();
    let session = project.run_scenario().unwrap();
    let state = session.store().export_for_tools(100_000).unwrap();
    let player = state
        .actors
        .iter()
        .find(|a| a.role == ActorRole::Player)
        .unwrap();
    assert_eq!(
        session
            .store()
            .export_for_tools(100_000)
            .unwrap()
            .derived(project.content(), player.id)
            .unwrap()[&Key::new("strength").unwrap()],
        17
    );
}
#[test]
fn translation_updates_do_not_change_mechanical_save_compatibility() {
    let temp = Temp::new();
    let root = temp.source();
    let before = LoadedProject::load_directory(&root).unwrap();
    let path = root.join("packages/core/uk.ftl");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("Лікувальне зілля", "Цілюще зілля")).unwrap();
    let after = LoadedProject::load_directory(&root).unwrap();
    assert_eq!(
        before.content().fingerprint().unwrap(),
        after.content().fingerprint().unwrap()
    );
    let saves = SaveDirectory::new(temp.0.join("saves"), 1).unwrap();
    let session = before.run_scenario().unwrap();
    saves.quicksave(&session).unwrap();
    assert!(
        saves
            .load(
                SaveSlot::Quick,
                gameplay::ToolContent::new(after.content().clone()).unwrap(),
                temp.0.join("working.sqlite")
            )
            .is_ok()
    );
}
#[test]
fn category_mechanics_bindings_and_scenario_errors_are_rejected() {
    let temp = Temp::new();
    let root = temp.source();
    let original: ItemCatalog = read(root.join("packages/core/items.ron"));
    let mut items = original.clone();
    items.items[0].category = CategoryId::new();
    write(root.join("packages/core/items.ron"), &items);
    assert!(failure(&root).contains("category"));
    let mut items = original.clone();
    items.items[1].mechanics.modifiers[0].attribute = Key::new("unknown-attribute").unwrap();
    write(root.join("packages/core/items.ron"), &items);
    assert!(failure(&root).contains("unknown attribute"));
    write(root.join("packages/core/items.ron"), &original);
    let original: Bindings = read(root.join("packages/old_gate/conversations/gate/bindings.ron"));
    let mut bindings = original.clone();
    bindings.actions.remove(&Key::new("consume-key").unwrap());
    write(
        root.join("packages/old_gate/conversations/gate/bindings.ron"),
        &bindings,
    );
    assert!(failure(&root).contains("unknown dialogue action"));
    write(
        root.join("packages/old_gate/conversations/gate/bindings.ron"),
        &original,
    );
    let mut scenario: Scenario = read(root.join("scenario.ron"));
    scenario
        .wallets
        .iter_mut()
        .find(|w| w.owner.kind == "party")
        .unwrap()
        .balance = inventory::Money::ZERO;
    write(root.join("scenario.ron"), &scenario);
    let error = failure(&root);
    assert!(error.contains("scenario step 5 (Trade)"), "{error}");
    assert!(error.contains("funds"));
}
#[test]
fn malformed_ron_unknown_fields_invalid_uuid_and_versions_have_diagnostics() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/core/items.ron");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, original.replace("weight_grams:", "wieght_grams:")).unwrap();
    let error = failure(&root);
    assert!(error.contains("packages/core/items.ron"));
    assert!(error.contains("wieght_grams"));
    fs::write(
        &path,
        original.replacen("01010101-0101-0101-0101-010101010101", "bad-id", 1),
    )
    .unwrap();
    assert!(failure(&root).contains("UUID"));
    fs::write(&path, "(broken syntax").unwrap();
    let error = failure(&root);
    assert!(error.contains("packages/core/items.ron"));
    assert!(error.contains("1:"));
    fs::write(&path, original).unwrap();
    let mut project: ProjectFile = read(root.join("project.ron"));
    project.format = 999;
    write(root.join("project.ron"), &project);
    assert!(matches!(
        LoadedProject::load_directory(&root),
        Err(ContentError::SourceVersion(999))
    ));
}
#[test]
fn missing_source_messages_broken_fluent_references_and_syntax_fail() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/core/en.ftl");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace("category-keys = Keys", "different = Keys"),
    )
    .unwrap();
    assert!(failure(&root).contains("undeclared message different"));
    fs::write(
        &path,
        original.replace("category-keys = Keys", "category-keys = { nonexistent }"),
    )
    .unwrap();
    assert!(failure(&root).contains("nonexistent"));
    fs::write(
        &path,
        original.replace("category-keys = Keys", "category-keys = { $missing }"),
    )
    .unwrap();
    assert!(failure(&root).contains("undeclared argument"));
    fs::write(&path, "broken = {\n").unwrap();
    let error = failure(&root);
    assert!(error.contains("Fluent"));
}
#[test]
fn partial_translations_report_fallback_and_starting_names_are_validated() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/core/uk.ftl");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(path, original.replace("category-keys = Ключі", "")).unwrap();
    let project = LoadedProject::load_directory(&root).unwrap();
    assert_eq!(project.warnings().len(), 1);
    assert!(project.warnings()[0].contains("category-keys"));
    let mut scenario: Scenario = read(root.join("scenario.ron"));
    scenario.actors[0].name =
        Some(TextRef::message(game_types::TextResourceId([8; 16]), "missing-actor-name").unwrap());
    write(root.join("scenario.ron"), &scenario);
    assert!(failure(&root).contains("missing-actor-name"));
}
#[test]
fn resource_paths_and_read_budgets_are_bounded() {
    let temp = Temp::new();
    let root = temp.source();
    let mut project: ResourceFile = read(root.join("packages/core/messages.ron"));
    let original = project.clone();
    project.locales[0].path = "../outside.ftl".into();
    write(root.join("packages/core/messages.ron"), &project);
    assert!(failure(&root).contains("relative"));
    write(root.join("packages/core/messages.ron"), &original);
    fs::write(
        root.join("packages/core/en.ftl"),
        "x".repeat(2 * 1024 * 1024 + 1),
    )
    .unwrap();
    assert!(failure(&root).contains("exceeds"));
}
#[cfg(unix)]
#[test]
fn resource_symlinks_cannot_escape_project() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/core/en.ftl");
    fs::remove_file(&path).unwrap();
    let outside = temp.0.join("outside.ftl");
    fs::write(&outside, "outside = Content").unwrap();
    std::os::unix::fs::symlink(outside, path).unwrap();
    assert!(failure(&root).contains("symlink escapes"));
}
#[test]
fn bundle_publication_does_not_clobber_and_failed_publication_cleans_staging() {
    let temp = Temp::new();
    let project = LoadedProject::load_directory(sample()).unwrap();
    let output = temp.0.join("keep.sqlite");
    fs::write(&output, "sentinel").unwrap();
    assert!(project.build(&output).is_err());
    assert_eq!(fs::read_to_string(&output).unwrap(), "sentinel");
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
    assert!(project.build(temp.0.join("absent/content.sqlite")).is_err());
}
#[test]
fn bundle_readers_reject_wrong_kind_schema_and_tampered_content() {
    let temp = Temp::new();
    let project = LoadedProject::load_directory(sample()).unwrap();
    let output = temp.0.join("game.sqlite");
    project.build(&output).unwrap();
    let connection = Connection::open(&output).unwrap();
    connection
        .pragma_update(None, "application_id", 123)
        .unwrap();
    assert!(matches!(
        LoadedProject::materialize_bundle_for_tools(&output),
        Err(ContentError::WrongDatabase)
    ));
    connection
        .pragma_update(None, "application_id", BUNDLE_APPLICATION_ID)
        .unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    assert!(matches!(
        LoadedProject::materialize_bundle_for_tools(&output),
        Err(ContentError::BundleVersion(99))
    ));
    connection
        .pragma_update(None, "user_version", BUNDLE_SCHEMA_VERSION)
        .unwrap();
    connection
        .execute("UPDATE assets SET hash=zeroblob(32) WHERE kind=2", [])
        .unwrap();
    assert!(
        LoadedProject::materialize_bundle_for_tools(&output)
            .err()
            .unwrap()
            .to_string()
            .contains("checksum mismatch")
    );
}
#[test]
fn cli_validates_builds_and_runs_from_bundle_with_locale_and_saves() {
    let temp = Temp::new();
    let executable = env!("CARGO_BIN_EXE_yarra-game-content");
    let root = sample();
    let bundle = temp.0.join("built.sqlite");
    let output = Command::new(executable)
        .arg("build")
        .arg(&root)
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(executable)
        .arg("validate")
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("8 scenario steps"));
    let en = temp.0.join("en.sqlite");
    let uk = temp.0.join("uk.sqlite");
    let project = LoadedProject::load_directory(&root).unwrap();
    project.build_language_pack("en", &en).unwrap();
    project.build_language_pack("uk", &uk).unwrap();
    let saves = temp.0.join("saves");
    let output = Command::new(executable)
        .arg("demo")
        .arg(&bundle)
        .arg("--language")
        .arg(&en)
        .arg("--language")
        .arg(&uk)
        .args(["--locale", "uk", "--save-dir"])
        .arg(&saves)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("health 75"));
    assert!(text.contains("Лікувальне зілля"));
    assert_eq!(
        SaveDirectory::new(&saves, 3).unwrap().list().unwrap().len(),
        3
    );
    let root = temp.source();
    fs::write(root.join("packages/core/rules.ron"), "invalid").unwrap();
    let output = Command::new(executable)
        .arg("validate")
        .arg(root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("packages/core/rules.ron"));
}

#[test]
fn duplicate_action_and_attribute_keys_are_not_silently_replaced() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/old_gate/conversations/gate/bindings.ron");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace(
            "actions: {",
            "actions: {\n\"consume-key\": SetFact(key: \"gate-rewarded\", value: false),",
        ),
    )
    .unwrap();
    let error = failure(&root);
    assert!(error.contains("duplicate semantic key consume-key"));
    assert!(error.contains("packages/old_gate/conversations/gate/bindings.ron"));
    fs::write(&path, original).unwrap();
    let path = root.join("packages/core/actors.ron");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace("base: {", "base: {\"strength\": 20,"),
    )
    .unwrap();
    assert!(failure(&root).contains("duplicate semantic key strength"));
}
