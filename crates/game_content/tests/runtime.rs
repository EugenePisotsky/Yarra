use game_types::*;
use gameplay::{Command, Driver, GameSession};
use save::{SaveDirectory, SaveSlot};
use yarra_game_content::*;
mod support;
use support::Temp;

#[test]
fn retained_generation_restores_without_sources_and_missing_generation_fails() {
    let temp = Temp::new();
    let project = LoadedProject::load_directory(temp.source()).unwrap();
    let bundle = temp.0.join("bundle.sqlite");
    project.build(&bundle).unwrap();
    let library = ContentLibrary::new(temp.0.join("retained")).unwrap();
    let identity = library.retain(&bundle).unwrap();
    assert_eq!(library.retain(&bundle).unwrap(), identity);
    let mut session = GameSession::new(
        library.open(&identity).unwrap(),
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
    std::fs::remove_file(bundle).unwrap();
    drop(session);
    drop(project);
    let loaded = library.load(&saves, SaveSlot::Manual(1)).unwrap();
    assert_eq!(loaded.state(), &expected);
    drop(loaded);
    std::fs::remove_dir_all(temp.0.join("retained")).unwrap();
    assert!(library.load(&saves, SaveSlot::Manual(1)).is_err());
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
    let mut driver = Driver::new(session, 20, 3).unwrap();
    assert_eq!(
        driver
            .advance_until(10, |s| s.time.0 >= time.0 + 100)
            .unwrap(),
        5
    );
    assert_eq!(driver.trace().len(), 3);
    assert!(driver.advance_until(2, |_| false).is_err());
    let previous = driver.session().header();
    let trace = driver.trace().len();
    assert!(
        driver
            .submit(Command::UseItem {
                actor: ActorId([99; 16]),
                item: ItemId([99; 16])
            })
            .is_err()
    );
    assert_eq!(driver.session().header(), previous);
    assert_eq!(driver.trace().len(), trace);
}
