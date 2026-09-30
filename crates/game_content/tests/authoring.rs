use game_types::*;
use gameplay::{
    Command, Condition,
    fixtures::{GATE_DIALOGUE, HERO, MERCHANT},
};
use gameplay::{dialogue, inventory};
use localization::{Arguments, Localization, LocalizationLimits};
use std::{fs, path::Path};
use yarra_game_content::*;
mod support;
use support::{Temp, read, write};
const CORE: &str = "packages/core/messages.ron";
const GATE: &str = "packages/old_gate/conversations/gate";
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
    let text_id = TextResourceId([10; 16]);
    let mut graph: dialogue::Dialogue = read(root.join(format!("{GATE}/graph.ron")));
    graph.id = second_id;
    graph.nodes[0].text = TextRef::message(text_id, "greeting").unwrap();
    graph.nodes[1].text = TextRef::message(text_id, "return-key").unwrap();
    // Same node names as the first conversation, a stricter condition.
    graph.nodes[1].condition = Some(Condition::HasItem {
        definition: inventory::fixtures::KEY,
        quantity: 2,
    });
    let folder = "packages/old_gate/conversations/second";
    fs::create_dir_all(root.join(folder)).unwrap();
    write(root.join(format!("{folder}/graph.ron")), &graph);
    let mut resource: ResourceFile = read(root.join(format!("{GATE}/messages.ron")));
    resource.contract.id = text_id;
    for locale in &mut resource.locales {
        let text = fs::read_to_string(root.join(&locale.path))
            .unwrap()
            .replace("Did you find the key?", "I need two keys.");
        locale.path = format!("{folder}/{}.ftl", locale.locale);
        fs::write(root.join(&locale.path), text).unwrap();
    }
    write(root.join(format!("{folder}/messages.ron")), &resource);
    write(
        root.join(format!("{folder}/conversation.ron")),
        &ConversationFile {
            graph: format!("{folder}/graph.ron"),
            resources: vec![format!("{folder}/messages.ron")],
        },
    );
    let mut package: PackageFile = read(root.join("packages/old_gate/package.ron"));
    package
        .conversations
        .push(format!("{folder}/conversation.ron"));
    write(root.join("packages/old_gate/package.ron"), &package);
    let project = LoadedProject::load_directory(&root).unwrap();
    assert_eq!(project.content().game.dialogues.len(), 7);
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
    fs::rename(root.join(GATE), root.join("moved-conversation")).unwrap();
    for name in ["conversation.ron", "messages.ron"] {
        let path = root.join("moved-conversation").join(name);
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace(GATE, "moved-conversation"),
        )
        .unwrap();
    }
    let path = root.join("packages/old_gate/package.ron");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace(GATE, "moved-conversation"),
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
            LocalizationLimits::default(),
        )
        .unwrap()
    };
    let old = render(&uk1);
    let new = render(&uk2);
    assert_eq!(old.cached_scopes(), 0);
    assert_eq!(
        new.format("uk", &name, &Arguments::new()).unwrap().value,
        "Цілюще зілля"
    );
    assert_eq!(
        old.format("uk", &name, &Arguments::new()).unwrap().value,
        "Лікувальне зілля"
    );
    assert_eq!(new.cached_scopes(), 1);
    library.load(&saves, save::SaveSlot::Quick).unwrap();
}
#[test]
fn source_revisions_mark_only_affected_translations_and_shipping_requires_review() {
    let temp = Temp::new();
    let root = temp.source();
    let before = LoadedProject::load_directory(&root).unwrap();
    let path = root.join(format!("{GATE}/en.ftl"));
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("Did you find the key?", "Have you brought the key?"),
    )
    .unwrap();
    let after = LoadedProject::load_directory(&root).unwrap();
    assert_eq!(
        before.content().fingerprint().unwrap(),
        after.content().fingerprint().unwrap()
    );
    let stale: Vec<_> = after
        .translation_reviews()
        .iter()
        .filter(|r| !r.reviewed)
        .collect();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].key.as_str(), "greeting");
    let mut project: ProjectFile = read(root.join("project.ron"));
    project.shipping_locales.insert("uk".into());
    write(root.join("project.ron"), &project);
    assert!(error(&root).contains("stale translations"));
    let mut resource: ResourceFile = read(root.join(format!("{GATE}/messages.ron")));
    resource
        .locales
        .iter_mut()
        .find(|r| r.locale == "uk")
        .unwrap()
        .reviewed
        .insert(stale[0].key.clone(), stale[0].source_revision.clone());
    write(root.join(format!("{GATE}/messages.ron")), &resource);
    assert!(
        LoadedProject::load_directory(root)
            .unwrap()
            .warnings()
            .is_empty()
    );
}
#[test]
fn package_dependencies_import_cycles_and_hidden_fluent_errors_fail_publication() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/old_gate/package.ron");
    let original: PackageFile = read(&path);
    let mut package = original.clone();
    package.dependencies.clear();
    write(&path, &package);
    assert!(error(&root).contains("package dependency"));
    write(&path, &original);
    let resource_path = root.join(CORE);
    let original: ResourceFile = read(&resource_path);
    let mut resource = original.clone();
    resource
        .contract
        .imports
        .insert(TextResourceId::named("old_gate/gate/text"));
    write(&resource_path, &resource);
    assert!(error(&root).contains("package dependency"));
    write(&resource_path, &original);
    let path = root.join("packages/core/en.ftl");
    let original = fs::read_to_string(&path).unwrap();
    for extra in [
        "\n-unused = { -unused }",
        "\n-unused = { BAD() }",
        "\n-unused = { unknown }",
        "\nwelcome = Duplicate",
    ] {
        fs::write(&path, format!("{original}{extra}")).unwrap();
        assert!(LoadedProject::load_directory(&root).is_err());
    }
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
            [TextResourceId::named("core/text").raw()],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("PRIMARY KEY"));
    db.execute(
        "UPDATE resources SET hash=zeroblob(32) WHERE id=?1",
        [TextResourceId::named("old_gate/gate/text").raw()],
    )
    .unwrap();
    drop(db);
    let mut pack = LanguageRepository::open(&path).unwrap();
    assert_eq!(pack.stats(), &LanguageStats::default());
    assert!(
        pack.load(TextResourceId::named("core/text"))
            .unwrap()
            .unwrap()
            .source
            .contains("Лікувальне зілля")
    );
    assert_eq!(pack.stats().decoded_resources, 1);
    assert!(
        pack.load(TextResourceId::named("old_gate/gate/text"))
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
    let path = root.join(CORE);
    let mut contract: ResourceFile = read(&path);
    contract
        .contract
        .messages
        .get_mut(&TextKey::new("welcome").unwrap())
        .unwrap()
        .arguments
        .insert(
            "name".into(),
            ArgumentType::Select(["Ada".into(), "Lin".into()].into()),
        );
    write(path, &contract);
    let after = LoadedProject::load_directory(root).unwrap();
    assert_ne!(
        before.content().fingerprint().unwrap(),
        after.content().fingerprint().unwrap()
    );
}
