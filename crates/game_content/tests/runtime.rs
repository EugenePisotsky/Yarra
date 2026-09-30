use game_types::*;
use gameplay::{Command, ContentSource, GameSession, HeadlessDriver, StateRequest};
use inventory::{Inventory, ItemCatalog};
use save::{SaveDirectory, SaveSlot, StateStats, WorkingStore};
use yarra_game_content::*;
mod support;
use support::{Temp, read, write};

fn setup(
    temp: &Temp,
    definitions: usize,
    population: usize,
) -> (LoadedProject, gameplay::SessionState, std::path::PathBuf) {
    let source = temp.source();
    let mut catalog: ItemCatalog = read(source.join("packages/core/items.ron"));
    let sample = catalog.items[0].clone();
    for i in catalog.items.len()..definitions {
        let mut item = sample.clone();
        item.id = ItemDefinitionId((i as u128 + 1000).to_le_bytes());
        item.key = format!("extra_{i}");
        catalog.items.push(item);
    }
    write(source.join("packages/core/items.ron"), &catalog);
    let project = LoadedProject::load_directory(&source).unwrap();
    let mut seed = project
        .start()
        .unwrap()
        .store()
        .export_for_tools(100_000)
        .unwrap();
    for i in seed.actors.len()..population {
        let mut actor = seed.actors[1].clone();
        actor.id = ActorId((i as u128 + 1000).to_le_bytes());
        actor.equipment.clear();
        actor.effects.clear();
        let mut bag = Inventory::new(OwnerRef::actor(actor.id), "carried").unwrap();
        bag.id = InventoryId((i as u128 + 1000).to_le_bytes());
        seed.actors.push(actor);
        seed.inventories.push(bag);
    }
    let bundle = temp.0.join("bundle.sqlite");
    project.build(&bundle).unwrap();
    (project, seed, bundle)
}
fn profile(definitions: usize, population: usize) -> (StateStats, RepositoryStats) {
    let temp = Temp::new();
    let (project, seed, bundle) = setup(&temp, definitions, population);
    let store = WorkingStore::create(temp.0.join("live.sqlite"), project.content(), &seed).unwrap();
    let repository = ContentRepository::open(bundle, RepositoryLimits::default()).unwrap();
    let mut session = GameSession::new(store, repository).unwrap();
    assert_eq!(session.store().stats().decoded_records, 0);
    assert_eq!(session.content_source().stats().decoded_assets, 0);
    let player = project
        .scenario()
        .actors
        .iter()
        .find(|a| a.role == actors::ActorRole::Player)
        .unwrap()
        .id;
    let bag = seed.carried(player).unwrap();
    let potion = bag
        .entries
        .iter()
        .find(|e| {
            !project
                .content()
                .items
                .item(e.definition)
                .unwrap()
                .mechanics
                .on_use
                .is_empty()
        })
        .unwrap()
        .id;
    session
        .apply(Command::UseItem {
            actor: player,
            item: potion,
        })
        .unwrap();
    let state_stats = session.store().stats();
    let content_stats = session.content_source().stats();
    let saves = SaveDirectory::new(temp.0.join("slots"), 2).unwrap();
    saves.quicksave(&session).unwrap();
    assert_eq!(session.store().stats(), state_stats);
    assert_eq!(session.content_source().stats(), content_stats);
    let library =
        ContentLibrary::new(temp.0.join("retained"), RepositoryLimits::default()).unwrap();
    library.retain(temp.0.join("bundle.sqlite")).unwrap();
    let mut loaded = library
        .load(&saves, SaveSlot::Quick, temp.0.join("restored.sqlite"))
        .unwrap();
    assert_eq!(loaded.store().stats().decoded_records, 0);
    assert_eq!(loaded.content_source().stats().decoded_assets, 0);
    let remote = seed.actors.last().unwrap().id;
    assert_eq!(
        loaded
            .query(StateRequest {
                actors: [remote].into(),
                ..Default::default()
            })
            .unwrap()
            .actor(remote)
            .unwrap(),
        seed.actor(remote).unwrap()
    );
    (state_stats, content_stats)
}
#[test]
fn command_and_checkpoint_working_sets_do_not_grow_with_catalog_or_population() {
    let small = profile(16, 3);
    let large = profile(10_000, 5_000);
    println!("small={small:?}\nlarge={large:?}");
    assert_eq!(small, large);
    assert_eq!(small.0.decoded_records, 2);
    assert_eq!(small.0.written_records, 2);
}
#[test]
fn retained_generation_restores_without_sources_and_missing_generation_fails() {
    let temp = Temp::new();
    let (project, seed, bundle) = setup(&temp, 16, 10);
    let library =
        ContentLibrary::new(temp.0.join("retained"), RepositoryLimits::default()).unwrap();
    let identity = library.retain(&bundle).unwrap();
    assert_eq!(library.retain(&bundle).unwrap(), identity);
    let store = WorkingStore::create(temp.0.join("live.sqlite"), project.content(), &seed).unwrap();
    let mut session = GameSession::new(store, library.open(&identity).unwrap()).unwrap();
    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }
    let expected = session.store().export_for_tools(100_000).unwrap();
    let saves = SaveDirectory::new(temp.0.join("slots"), 2).unwrap();
    saves
        .save(SaveSlot::Manual(1), &session, "Original generation")
        .unwrap();
    std::fs::remove_dir_all(temp.0.join("source")).unwrap();
    std::fs::remove_file(bundle).unwrap();
    drop(session);
    drop(project);
    let loaded = library
        .load(&saves, SaveSlot::Manual(1), temp.0.join("restored.sqlite"))
        .unwrap();
    assert_eq!(loaded.store().export_for_tools(100_000).unwrap(), expected);
    drop(loaded);
    std::fs::remove_dir_all(temp.0.join("retained")).unwrap();
    assert!(
        library
            .load(&saves, SaveSlot::Manual(1), temp.0.join("missing.sqlite"))
            .is_err()
    );
    assert!(!temp.0.join("missing.sqlite").exists());
}
#[test]
fn all_authored_operations_run_through_indexed_content_and_shared_driver() {
    let temp = Temp::new();
    let (project, seed, bundle) = setup(&temp, 16, 3);
    let store = WorkingStore::in_memory(project.content(), &seed).unwrap();
    let mut session = GameSession::new(
        store,
        ContentRepository::open(bundle, RepositoryLimits::default()).unwrap(),
    )
    .unwrap();
    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }
    let time = session.header().unwrap().time;
    let mut driver = HeadlessDriver::new(session, 20, 3).unwrap();
    assert_eq!(
        driver
            .advance_until(StateRequest::default(), 10, |s| s.time.0 >= time.0 + 100)
            .unwrap(),
        5
    );
    assert_eq!(driver.trace().len(), 3);
    assert!(
        driver
            .advance_until(StateRequest::default(), 2, |_| false)
            .is_err()
    );
    let previous = driver.session().header().unwrap();
    let trace = driver.trace().len();
    assert!(
        driver
            .submit(Command::UseItem {
                actor: ActorId([99; 16]),
                item: ItemId([99; 16])
            })
            .is_err()
    );
    assert_eq!(driver.session().header().unwrap(), previous);
    assert_eq!(driver.trace().len(), trace);
}
#[test]
fn insufficient_content_budget_rolls_back_before_any_mutation() {
    let temp = Temp::new();
    let (project, seed, bundle) = setup(&temp, 16, 3);
    let store = WorkingStore::in_memory(project.content(), &seed).unwrap();
    let before = store.export_for_tools(100_000).unwrap();
    let repository = ContentRepository::open(
        bundle,
        RepositoryLimits {
            max_resolved_assets: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut session = GameSession::new(store, repository).unwrap();
    assert!(
        session
            .query(StateRequest {
                actors: [seed.actors[0].id].into(),
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(session.store().export_for_tools(100_000).unwrap(), before);
    assert_eq!(
        session.content_source().identity(),
        session.identity().clone()
    );
}
