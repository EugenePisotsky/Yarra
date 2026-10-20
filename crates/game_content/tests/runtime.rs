use game_types::*;
use gameplay::{Command, Driver, GameSession};
use save::{SaveDirectory, SaveSlot};
use yarra_game_content::*;

mod support;

use support::Temp;

#[test]
fn a_bundle_restores_a_save_without_sources_and_a_missing_bundle_fails() {
    let temp = Temp::new();
    let project = LoadedProject::load_directory(temp.source()).unwrap();
    let bundle = temp.0.join("bundle.sqlite");
    project.build(&bundle).unwrap();
    let mut session = GameSession::new(
        ContentRepository::open(&bundle).unwrap(),
        project.start().unwrap().into_state(),
    )
    .unwrap();

    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }

    let expected = session.state().clone();
    let saves = SaveDirectory::new(temp.0.join("slots"), 2).unwrap();
    saves
        .save(SaveSlot::Manual(1), &session, "Original generation")
        .unwrap();
    std::fs::remove_dir_all(temp.0.join("source")).unwrap();
    drop(session);
    drop(project);
    let repository = ContentRepository::open(&bundle).unwrap();
    let loaded = saves.load(SaveSlot::Manual(1), repository).unwrap();
    assert_eq!(loaded.state(), &expected);
    drop(loaded);
    std::fs::remove_file(&bundle).unwrap();
    assert!(ContentRepository::open(&bundle).is_err());
}

#[test]
fn authored_operations_run_through_published_content_and_the_shared_driver() {
    let temp = Temp::new();
    let project = LoadedProject::load_directory(temp.source()).unwrap();
    let mut session = support::runtime(&temp, &project);

    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }

    let time = session.header().time;
    let mut driver = Driver::new(session, 20).unwrap();
    assert_eq!(
        driver
            .advance_until(10, |s| s.time.0 >= time.0 + 100)
            .unwrap(),
        5
    );
    assert!(driver.advance_until(2, |_| false).is_err());
    let previous = driver.session().header();
    assert!(
        driver
            .submit(Command::UseItem {
                actor: ActorId([99; 16]),
                item: ItemId([99; 16])
            })
            .is_err()
    );
    assert_eq!(driver.session().header(), previous);
}
