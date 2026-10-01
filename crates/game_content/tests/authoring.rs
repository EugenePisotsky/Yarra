use game_types::*;
use gameplay::{
    Command, Condition,
    fixtures::{GATE_DIALOGUE, HERO, MERCHANT},
};
use gameplay::{dialogue, inventory};
use localization::{Arguments, Localization};
use std::{fs, path::Path};
use yarra_game_content::*;
mod support;
use support::{Temp, read, write};
const GATE: &str = "packages/old_gate/conversations/gate.dialogue.ron";
fn error(path: &Path) -> String {
    LoadedProject::load_directory(path)
        .err()
        .expect("invalid project accepted")
        .to_string()
}
#[test]
fn conversations_reuse_text_keys_and_node_names_without_sharing_logic() {
    let temp = Temp::new();
    let root = temp.source();
    let second_id = DialogueId([2; 16]);
    // Another package, with text of its own under the same keys.
    let text_id = TextResourceId::try_from("second".to_owned()).unwrap();
    let mut graph: dialogue::Dialogue = read(root.join(GATE));
    graph.id = second_id;
    graph.nodes[0].text = TextRef::message(text_id, "greeting").unwrap();
    graph.nodes[1].text = TextRef::message(text_id, "return-key").unwrap();
    // Same node names as the first conversation, a stricter condition.
    graph.nodes[1].condition = Some(Condition::HasItem {
        definition: inventory::fixtures::KEY,
        quantity: 2,
    });
    let folder = "packages/second";
    fs::create_dir_all(root.join(folder)).unwrap();
    write(root.join(format!("{folder}/gate.dialogue.ron")), &graph);
    for locale in ["en", "uk"] {
        let text = fs::read_to_string(root.join(format!("packages/old_gate/{locale}.ftl")))
            .unwrap()
            .replace("Did you find the key?", "I need two keys.");
        fs::write(root.join(format!("{folder}/{locale}.ftl")), text).unwrap();
    }
    let mut manifest: ProjectFile = read(root.join("project.ron"));
    manifest.packages.push(folder.into());
    write(root.join("project.ron"), &manifest);
    let project = LoadedProject::load_directory(&root).unwrap();
    assert_eq!(project.content().game.dialogues.len(), 9);
    let mut session = project.start().unwrap();
    for dialogue in [GATE_DIALOGUE, second_id] {
        session
            .apply(Command::StartDialogue {
                bindings: Default::default(),
                dialogue,
                participant: HERO,
                speaker: MERCHANT,
            })
            .unwrap();
        session
            .apply(Command::AdvanceLine {
                key: gameplay::ConversationKey {
                    dialogue,
                    participant: HERO,
                    speaker: MERCHANT,
                },
                expected: dialogue::Token { run: 1, step: 0 },
            })
            .unwrap();
    }
    assert_eq!(
        session
            .available_choices(GATE_DIALOGUE, HERO, MERCHANT)
            .unwrap()
            .len(),
        1
    );
    assert!(
        session
            .available_choices(second_id, HERO, MERCHANT)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        project
            .localization()
            .format("en", &graph.nodes[0].text, &Arguments::new())
            .unwrap()
            .value,
        "I need two keys."
    );
    let path = temp.0.join("mechanics.sqlite");
    project.build(&path).unwrap();
    // Each conversation is published whole, with its own conditions.
    let mut repo = ContentRepository::open(path).unwrap();
    let second = gameplay::ContentSource::dialogue(&mut repo, second_id).unwrap();
    let first = gameplay::ContentSource::dialogue(&mut repo, GATE_DIALOGUE).unwrap();
    assert_eq!(second, graph);
    assert_eq!(first.nodes[1].id, second.nodes[1].id);
    assert_ne!(first.nodes[1].condition, second.nodes[1].condition);
}
#[test]
fn moving_conversation_and_reordering_packages_preserves_publication_identity() {
    let temp = Temp::new();
    let root = temp.source();
    let before = LoadedProject::load_directory(&root).unwrap();
    let first = temp.0.join("first.sqlite");
    before.build(&first).unwrap();
    // Where a file sits in its package, and the order of packages, are no identity.
    fs::create_dir(root.join("packages/old_gate/moved")).unwrap();
    fs::rename(
        root.join(GATE),
        root.join("packages/old_gate/moved/gate.dialogue.ron"),
    )
    .unwrap();
    fs::rename(
        root.join("packages/old_gate/uk.ftl"),
        root.join("packages/old_gate/moved/uk.ftl"),
    )
    .unwrap();
    let mut manifest: ProjectFile = read(root.join("project.ron"));
    manifest.packages.reverse();
    write(root.join("project.ron"), &manifest);
    let after = LoadedProject::load_directory(root).unwrap();
    let second = temp.0.join("second.sqlite");
    after.build(&second).unwrap();
    assert_eq!(
        before.content().fingerprint().unwrap(),
        after.content().fingerprint().unwrap()
    );
    let a = ContentRepository::open(first).unwrap();
    let b = ContentRepository::open(second).unwrap();
    assert_eq!(a.manifest().publication_hash, b.manifest().publication_hash);
}
#[test]
fn independent_language_updates_work_with_retained_content_and_saved_state() {
    let temp = Temp::new();
    let root = temp.source();
    let project = LoadedProject::load_directory(&root).unwrap();
    let content = temp.0.join("mechanics.sqlite");
    project.build(&content).unwrap();
    let en = temp.0.join("en.sqlite");
    project.build_language_pack("en", &en).unwrap();
    let uk1 = temp.0.join("uk1.sqlite");
    project.build_language_pack("uk", &uk1).unwrap();
    let library = ContentLibrary::new(temp.0.join("library")).unwrap();
    let identity = library.retain(&content).unwrap();
    let saves = save::SaveDirectory::new(temp.0.join("saves"), 1).unwrap();
    saves.quicksave(&project.run_scenario().unwrap()).unwrap();
    let path = root.join("packages/core/uk.ftl");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("Лікувальне зілля", "Цілюще зілля"),
    )
    .unwrap();
    let updated = LoadedProject::load_directory(&root).unwrap();
    let uk2 = temp.0.join("uk2.sqlite");
    updated.build_language_pack("uk", &uk2).unwrap();
    assert_eq!(
        updated.content().fingerprint().unwrap(),
        identity.fingerprint
    );
    let name = project
        .content()
        .items
        .item(ItemDefinitionId::named("healing_potion"))
        .unwrap()
        .name
        .clone();
    fs::remove_dir_all(root).unwrap();
    fs::remove_file(content).unwrap();
    let render = |pack| {
        Localization::with_source(
            "en",
            Box::new(
                LanguageSource::new(
                    library.open(&identity).unwrap(),
                    vec![
                        LanguageRepository::open(&en).unwrap(),
                        LanguageRepository::open(pack).unwrap(),
                    ],
                )
                .unwrap(),
            ),
        )
        .unwrap()
    };
    let old = render(&uk1);
    let new = render(&uk2);
    assert_eq!(
        new.format("uk", &name, &Arguments::new()).unwrap().value,
        "Цілюще зілля"
    );
    assert_eq!(
        old.format("uk", &name, &Arguments::new()).unwrap().value,
        "Лікувальне зілля"
    );
    library.load(&saves, save::SaveSlot::Quick).unwrap();
}
#[test]
fn new_wording_changes_no_identity_and_missing_translations_fall_back() {
    let temp = Temp::new();
    let root = temp.source();
    let before = LoadedProject::load_directory(&root).unwrap();
    assert!(before.warnings().is_empty());
    let english = root.join("packages/old_gate/en.ftl");
    fs::write(
        &english,
        fs::read_to_string(&english)
            .unwrap()
            .replace("Did you find the key?", "Have you brought the key?"),
    )
    .unwrap();
    let ukrainian = root.join("packages/old_gate/uk.ftl");
    fs::write(&ukrainian, "return-key = Ось ключ.\n").unwrap();
    let after = LoadedProject::load_directory(&root).unwrap();
    assert_eq!(
        before.content().fingerprint().unwrap(),
        after.content().fingerprint().unwrap()
    );
    assert_eq!(after.warnings().len(), 1);
    assert!(after.warnings()[0].contains("uk: old_gate/greeting falls back to en"));
}
#[test]
fn misnamed_files_hidden_fluent_errors_and_extra_translations_fail_publication() {
    let temp = Temp::new();
    let root = temp.source();
    let stray = root.join("packages/core/item.ron");
    fs::write(&stray, "()").unwrap();
    assert!(error(&root).contains("no kind of content is named like this"));
    fs::remove_file(stray).unwrap();
    let path = root.join("packages/core/en.ftl");
    let original = fs::read_to_string(&path).unwrap();
    for extra in [
        "\n-unused = { -unused }",
        "\n-unused = { BAD() }",
        "\n-unused = { unknown }",
        "\nwelcome = Duplicate",
    ] {
        fs::write(&path, format!("{original}{extra}")).unwrap();
        assert!(LoadedProject::load_directory(&root).is_err(), "{extra}");
    }
    fs::write(&path, &original).unwrap();
    // A translation says what the source language says, not more.
    let ukrainian = root.join("packages/core/uk.ftl");
    let text = fs::read_to_string(&ukrainian).unwrap();
    fs::write(&ukrainian, format!("{text}\nunheard-of = Нове")).unwrap();
    assert!(error(&root).contains("unheard-of is not in the source-language text"));
}
#[test]
fn language_packs_are_indexed_lazy_and_reject_requested_corruption() {
    let temp = Temp::new();
    let project = LoadedProject::load_directory(temp.source()).unwrap();
    let path = temp.0.join("uk.sqlite");
    project.build_language_pack("uk", &path).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let plan: String = db
        .query_row(
            "EXPLAIN QUERY PLAN SELECT payload FROM resources WHERE id=?1",
            [TextResourceId::named("core").raw()],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("PRIMARY KEY"));
    db.execute(
        "UPDATE resources SET hash=zeroblob(32) WHERE id=?1",
        [TextResourceId::named("old_gate").raw()],
    )
    .unwrap();
    drop(db);
    let mut pack = LanguageRepository::open(&path).unwrap();
    assert_eq!(pack.stats(), &LanguageStats::default());
    assert!(
        pack.load(TextResourceId::named("core"))
            .unwrap()
            .unwrap()
            .source
            .contains("Лікувальне зілля")
    );
    assert_eq!(pack.stats().decoded_resources, 1);
    assert!(
        pack.load(TextResourceId::named("old_gate"))
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    let before = fs::read(&path).unwrap();
    assert!(project.build_language_pack("uk", &path).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn argument_contract_changes_change_mechanical_identity() {
    let temp = Temp::new();
    let root = temp.source();
    let before = LoadedProject::load_directory(&root).unwrap();
    // The English text choosing on the name makes it a choice between named variants.
    let path = root.join("packages/core/en.ftl");
    let text = fs::read_to_string(&path).unwrap().replace(
        "welcome = Welcome, { $name }!",
        "welcome = { $name ->\n    [ada] Welcome, Ada!\n   *[lin] Welcome, Lin!\n}",
    );
    fs::write(&path, text).unwrap();
    let after = LoadedProject::load_directory(root).unwrap();
    assert_ne!(
        before.content().fingerprint().unwrap(),
        after.content().fingerprint().unwrap()
    );
}
