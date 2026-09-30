use gameplay::inventory::fixtures::POTION;
use gameplay::{Command, GameSession, ToolContent, fixtures::*};
use yarra_save::{SaveDirectory, SaveError, SaveSlot};

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "yarra-save-{}",
            game_types::PlaythroughId::random()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn session() -> GameSession {
    GameSession::new(ToolContent::new(content()).unwrap(), state()).unwrap()
}
fn source() -> ToolContent {
    ToolContent::new(content()).unwrap()
}
fn use_potion(session: &mut GameSession) {
    let item = session
        .state()
        .carried(HERO)
        .unwrap()
        .entries
        .iter()
        .find(|e| e.definition == POTION)
        .unwrap()
        .id;
    session
        .apply(Command::UseItem { actor: HERO, item })
        .unwrap();
}

#[test]
fn slots_are_independent_and_loading_discards_unsaved_progress() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(&temp.0, 2).unwrap();
    let mut session = session();
    saves.save(SaveSlot::Manual(1), &session, "Start").unwrap();
    let start = session.state().clone();
    use_potion(&mut session);
    saves.quicksave(&session).unwrap();
    let quick = session.state().clone();
    use_potion(&mut session);

    let loaded = saves.load(SaveSlot::Manual(1), source()).unwrap();
    assert_eq!(loaded.state(), &start);
    let loaded = saves.load(SaveSlot::Quick, source()).unwrap();
    assert_eq!(loaded.state(), &quick);
    assert_ne!(session.state(), &quick);

    let listed = saves.list().unwrap();
    assert_eq!(
        listed.iter().map(|i| i.slot).collect::<Vec<_>>(),
        vec![SaveSlot::Manual(1), SaveSlot::Quick]
    );
    assert_eq!(listed[0].title, "Start");
    assert_eq!(listed[1].generation, quick.generation);
}
#[test]
fn a_restored_playthrough_continues_with_the_same_random_stream_and_item_identities() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(&temp.0, 1).unwrap();
    let mut original = session();
    use_potion(&mut original);
    saves.quicksave(&original).unwrap();
    let mut restored = saves.load(SaveSlot::Quick, source()).unwrap();
    for session in [&mut original, &mut restored] {
        session
            .apply(Command::StartDialogue {
                bindings: Default::default(),
                dialogue: GATE_DIALOGUE,
                participant: HERO,
                speaker: MERCHANT,
            })
            .unwrap();
        let conversation = gameplay::ConversationKey {
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        };
        let expected = session.conversation_view(conversation).unwrap().token;
        session
            .apply(Command::AdvanceLine {
                key: conversation,
                expected,
            })
            .unwrap();
        let expected = session.conversation_view(conversation).unwrap().token;
        session
            .apply(Command::Choose {
                expected,
                dialogue: GATE_DIALOGUE,
                participant: HERO,
                speaker: MERCHANT,
                choice: key("return-key"),
            })
            .unwrap();
    }
    assert_eq!(original.state(), restored.state());
}
#[test]
fn autosaves_rotate_by_sequence_and_leave_other_slots_alone() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(&temp.0, 2).unwrap();
    let mut session = session();
    saves.save(SaveSlot::Manual(0), &session, "Keep").unwrap();
    let mut slots = Vec::new();
    for _ in 0..3 {
        use_potion(&mut session);
        slots.push(saves.autosave(&session).unwrap());
    }
    assert_eq!(
        slots.iter().map(|i| i.slot).collect::<Vec<_>>(),
        vec![SaveSlot::Auto(0), SaveSlot::Auto(1), SaveSlot::Auto(0)]
    );
    assert_eq!(
        slots.iter().map(|i| i.sequence).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(saves.save(SaveSlot::Auto(2), &session, "x").is_err());
    assert_eq!(saves.list().unwrap().len(), 3);
    assert_eq!(saves.list().unwrap()[0].title, "Keep");
}
#[test]
fn other_content_loads_while_foreign_files_and_state_that_does_not_fit_are_errors() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(&temp.0, 1).unwrap();
    let session = session();
    saves.quicksave(&session).unwrap();

    // Another publication of the content loads; the slot tells the game it changed.
    let mut other = content();
    other.manifest.revision += 1;
    let restored = saves
        .load(SaveSlot::Quick, ToolContent::new(other).unwrap())
        .unwrap();
    assert_eq!(restored.state(), session.state());
    let saved_with = saves.content_identity(SaveSlot::Quick).unwrap();
    assert_eq!(saved_with, *session.identity());
    assert_ne!(saved_with, *restored.identity());

    let path = saves.path(SaveSlot::Quick);
    let good = std::fs::read(&path).unwrap();
    let text = String::from_utf8(good.clone()).unwrap();
    // A resource beyond what the rules allow is brought back within its cap...
    let damaged = text.replacen("\"health\":50", "\"health\":5000", 1);
    assert_ne!(damaged, text);
    std::fs::write(&path, damaged).unwrap();
    let restored = saves.load(SaveSlot::Quick, source()).unwrap();
    for actor in restored.state().actors.values() {
        assert!(actor.resources[&key("health")] <= actor.stats[&key("max-health")]);
    }
    // ...while state the rules cannot bring in line is refused with the reason.
    let damaged = text.replacen("\"level\":1,", "\"level\":1000,", 1);
    assert_ne!(damaged, text);
    std::fs::write(&path, damaged).unwrap();
    let error = saves.load(SaveSlot::Quick, source()).err().unwrap();
    assert!(
        error.to_string().contains("level cannot be reached"),
        "{error}"
    );

    std::fs::write(&path, b"not a save\n{}").unwrap();
    assert!(matches!(
        saves.load(SaveSlot::Quick, source()),
        Err(SaveError::NotASave)
    ));
    std::fs::write(&path, b"{\"yarra_save\":1}\n{}").unwrap();
    assert!(matches!(
        saves.load(SaveSlot::Quick, source()),
        Err(SaveError::Format(1))
    ));
    // A failed save leaves no staging file behind and the slot is replaced whole.
    std::fs::write(&path, &good).unwrap();
    saves.quicksave(&session).unwrap();
    let names: Vec<_> = std::fs::read_dir(&temp.0)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["quick.save"]);
}
#[test]
fn slots_this_build_cannot_read_do_not_stop_saving() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(&temp.0, 2).unwrap();
    let session = session();
    saves.save(SaveSlot::Manual(1), &session, "Kept").unwrap();
    // What a format bump leaves behind, and a stray file.
    for slot in [SaveSlot::Quick, SaveSlot::Auto(0), SaveSlot::Manual(2)] {
        std::fs::write(saves.path(slot), b"{\"yarra_save\":1}\n{}").unwrap();
    }
    std::fs::write(saves.path(SaveSlot::Auto(1)), b"not a save").unwrap();
    assert!(matches!(
        saves.load(SaveSlot::Quick, source()),
        Err(SaveError::Format(1))
    ));
    let listed = |saves: &SaveDirectory| -> Vec<SaveSlot> {
        saves.list().unwrap().into_iter().map(|i| i.slot).collect()
    };
    assert_eq!(listed(&saves), [SaveSlot::Manual(1)]);
    assert_eq!(saves.quicksave(&session).unwrap().sequence, 1);
    // Unreadable autosaves are the first to be replaced.
    assert_eq!(saves.autosave(&session).unwrap().slot, SaveSlot::Auto(0));
    let second = saves.autosave(&session).unwrap();
    assert_eq!((second.slot, second.sequence), (SaveSlot::Auto(1), 2));
    assert_eq!(
        listed(&saves),
        [
            SaveSlot::Manual(1),
            SaveSlot::Quick,
            SaveSlot::Auto(0),
            SaveSlot::Auto(1)
        ]
    );
    saves.load(SaveSlot::Quick, source()).unwrap();
}
#[test]
fn a_save_is_brought_in_line_with_rules_that_changed_since() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(&temp.0, 1).unwrap();
    saves.quicksave(&session()).unwrap();
    // Level 2 now takes no experience at all.
    let mut changed = content();
    changed.game.rules.levels[0] = 0;
    let restored = saves
        .load(SaveSlot::Quick, ToolContent::new(changed).unwrap())
        .unwrap();
    let hero = restored.state().actor(HERO).unwrap();
    assert_eq!(hero.level, 2);
    // Worked out again for the new level; what the hero had left stays.
    assert_eq!(hero.stats[&key("max-health")], 110);
    assert_eq!(hero.resources[&key("health")], 50);
}
