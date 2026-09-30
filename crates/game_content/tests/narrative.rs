use game_types::*;
use gameplay::actors::RelationshipKey;
use gameplay::dialogue::InteractionKey;
use gameplay::quests::{Status, Transition};
use gameplay::{
    Command, GameEvent, GameSession,
    fixtures::{HERO, key},
};
use gameplay::{dialogue, inventory};
use rusqlite::{Connection, params};
use save::{SaveDirectory, SaveSlot};
use yarra_game_content::*;
const GUARD: ActorId = ActorId::named("guard");
mod support;
use support::{Temp, read, write};

const GATE: QuestId = QuestId::named("guard/gate");
const SUPPLIES: QuestId = QuestId::named("guard/supplies");
const READY: PredicateId = PredicateId::named("guard/ready");
const REWARD: DialogueId = DialogueId::named("guard/reward");
const DUTY: DialogueId = DialogueId::named("guard/duty");
const INTERACTION: InteractionKey = InteractionKey {
    participant: HERO,
    speaker: GUARD,
};
const ATTITUDE: RelationshipKey = RelationshipKey {
    from: GUARD,
    to: HERO,
};
fn setup(temp: &Temp) -> (LoadedProject, RuntimeSession) {
    let project =
        LoadedProject::load_directory_with_scenario(temp.source(), "scenarios/guard.ron").unwrap();
    let session = support::runtime(temp, &project);
    (project, session)
}
fn talk(topic: Option<&str>) -> Command {
    Command::Talk {
        bindings: Default::default(),
        participant: HERO,
        speaker: GUARD,
        topic: topic.map(key),
    }
}
fn choose() -> Command {
    Command::Choose {
        expected: dialogue::Token { run: 1, step: 2 },
        dialogue: REWARD,
        participant: HERO,
        speaker: GUARD,
        choice: key("return-key"),
    }
}
fn selection(session: &RuntimeSession) -> dialogue::Selection {
    session.state().selection(INTERACTION).unwrap().clone()
}
fn start_quest(session: &mut RuntimeSession, quest: QuestId) {
    session
        .apply(Command::Quest {
            quest,
            transition: Transition::Start,
        })
        .unwrap();
}

#[test]
fn guard_scenario_rewards_and_new_state_survive_checkpoint_restore() {
    let temp = Temp::new();
    let (project, mut session) = setup(&temp);
    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }
    let state = session.state().clone();
    assert_eq!(state.quest(GATE).status, Status::Completed);
    assert!(state.quest(GATE).completed.contains(&key("deliver")));
    assert_eq!(state.quest(SUPPLIES).status, Status::Active);
    assert_eq!(state.relationship(ATTITUDE).attitude, 5);
    assert_eq!(state.actor(HERO).unwrap().skills[&key("persuasion")], 10);
    assert!(
        !state
            .carried(HERO)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.definition == inventory::fixtures::KEY)
    );
    assert_eq!(selection(&session).dialogue, REWARD);
    let before = session.state().clone();
    assert!(session.apply(choose()).is_err());
    assert_eq!(session.state().clone(), before);
    let saves = SaveDirectory::new(temp.0.join("slots"), 2).unwrap();
    saves.quicksave(&session).unwrap();
    let library = ContentLibrary::new(temp.0.join("retained")).unwrap();
    library.retain(temp.0.join("content.sqlite")).unwrap();
    std::fs::remove_dir_all(temp.0.join("source")).unwrap();
    let loaded = library.load(&saves, SaveSlot::Quick).unwrap();
    // Restoring reads no dialogue graph: the saved conversation had finished.
    assert!(loaded.content().game.dialogues.is_empty());
    assert_eq!(loaded.state(), &state);
    assert_eq!(
        loaded
            .preview_interaction(HERO, GUARD)
            .unwrap()
            .opening()
            .unwrap()
            .rule,
        key("welcome")
    );
}

#[test]
fn preview_reports_quest_priority_and_independent_topics_without_writing_or_rolling() {
    let temp = Temp::new();
    let (_, mut session) = setup(&temp);
    let before = session.state().clone();
    let preview = session.preview_interaction(HERO, GUARD).unwrap();
    assert_eq!(preview.opening().unwrap().rule, key("welcome"));
    assert_eq!(preview.topics().count(), 0);
    let reward = preview
        .candidates
        .iter()
        .find(|r| r.rule == key("reward"))
        .unwrap();
    assert!(!reward.evaluation.matched);
    assert!(
        reward
            .evaluation
            .checks
            .iter()
            .any(|c| c.observed == gameplay::Observed::Quest(Status::NotStarted))
    );
    assert!(
        reward
            .evaluation
            .checks
            .iter()
            .any(|c| c.observed == gameplay::Observed::Quantity(1) && c.matched)
    );
    assert_eq!(session.state().clone(), before);
    session
        .apply(Command::AdjustRelationship {
            key: ATTITUDE,
            amount: -20,
        })
        .unwrap();
    assert_eq!(
        session
            .preview_interaction(HERO, GUARD)
            .unwrap()
            .opening()
            .unwrap()
            .rule,
        key("hostile")
    );
    start_quest(&mut session, GATE);
    start_quest(&mut session, SUPPLIES);
    let header = session.header();
    let preview = session.preview_interaction(HERO, GUARD).unwrap();
    assert_eq!(preview.opening().unwrap().rule, key("reward"));
    assert_eq!(
        preview
            .topics()
            .map(|r| r.rule.as_str())
            .collect::<Vec<_>>(),
        ["gate-topic", "supply-topic"]
    );
    assert_eq!(session.header(), header);
    session.apply(talk(Some("supply-topic"))).unwrap();
    assert_eq!(selection(&session).dialogue, DUTY);
}

#[test]
fn weighted_selection_replays_after_restore_and_active_conversation_resumes() {
    let temp = Temp::new();
    let (_, mut session) = setup(&temp);
    let saves = SaveDirectory::new(temp.0.join("slots"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let before = session.header();
    session.apply(talk(None)).unwrap();
    let chosen = selection(&session);
    let after = session.header();
    assert_ne!(before.narrative_random, after.narrative_random);
    assert_eq!(before.random, after.random);
    assert!(
        [
            DialogueId::named("guard/welcome_a"),
            DialogueId::named("guard/welcome_b")
        ]
        .contains(&chosen.dialogue)
    );
    let mut replay = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    replay.apply(talk(None)).unwrap();
    assert_eq!(selection(&replay), chosen);
    assert_eq!(replay.header(), after);
    // A new higher-priority quest must not silently replace a conversation in progress.
    start_quest(&mut session, GATE);
    let before_resume = session.header();
    let outcome = session.apply(talk(None)).unwrap();
    assert_eq!(
        outcome.events,
        [GameEvent::DialogueResumed {
            dialogue: chosen.dialogue
        }]
    );
    assert_eq!(
        outcome.header.narrative_random,
        before_resume.narrative_random
    );
    assert_eq!(selection(&session), chosen);
    saves.quicksave(&session).unwrap();
    let mut restored = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    assert_eq!(selection(&restored), chosen);
    let before = restored.header();
    assert!(restored.apply(talk(Some("gate-topic"))).is_err());
    assert_eq!(restored.header(), before);
    assert_eq!(restored.apply(talk(None)).unwrap().events, outcome.events);
}

#[test]
fn directed_relationships_have_explicit_neutral_defaults_and_clamp_adjustments() {
    let temp = Temp::new();
    let (_, mut session) = setup(&temp);
    let inverse = RelationshipKey {
        from: HERO,
        to: GUARD,
    };
    // Reads of records that were never written return defaults and store nothing.
    assert_eq!(session.state().relationship(ATTITUDE).attitude, 0);
    assert_eq!(session.state().quest(GATE).status, Status::NotStarted);
    assert!(session.state().relationships.is_empty());
    session
        .apply(Command::AdjustRelationship {
            key: ATTITUDE,
            amount: i16::MIN,
        })
        .unwrap();
    assert_eq!(session.state().relationship(ATTITUDE).attitude, -100);
    assert_eq!(session.state().relationship(inverse).attitude, 0);
    assert_eq!(session.state().relationships.len(), 1);
    session
        .apply(Command::AdjustRelationship {
            key: ATTITUDE,
            amount: i16::MAX,
        })
        .unwrap();
    assert_eq!(session.state().relationship(ATTITUDE).attitude, 100);
    let before = session.state().clone();
    assert!(
        session
            .apply(Command::AdjustRelationship {
                key: RelationshipKey {
                    from: HERO,
                    to: HERO
                },
                amount: 10
            })
            .is_err()
    );
    assert!(
        session
            .apply(Command::Quest {
                quest: QuestId([99; 16]),
                transition: Transition::Start
            })
            .is_err()
    );
    assert_eq!(session.state(), &before);
}

#[test]
fn only_the_selected_dialogue_graph_is_read() {
    let temp = Temp::new();
    let (project, session) = setup(&temp);
    drop(session);
    let bundle = temp.0.join("content.sqlite");
    // Damage to a candidate graph cannot affect opening, inspection or another selection.
    let db = Connection::open(&bundle).unwrap();
    db.execute(
        "UPDATE assets SET hash=zeroblob(32) WHERE kind=?1 AND id=?2",
        params![AssetKind::Dialogue as i64, REWARD.raw()],
    )
    .unwrap();
    let open = || {
        GameSession::new(
            ContentRepository::open(&bundle).unwrap(),
            project.start().unwrap().into_state(),
        )
        .unwrap()
    };
    let mut session = open();
    session.preview_interaction(HERO, GUARD).unwrap();
    assert!(session.content().game.dialogues.is_empty());
    session.apply(talk(None)).unwrap();
    assert_eq!(selection(&session).rule, key("welcome"));
    assert_eq!(session.content().game.dialogues.len(), 1);
    // The damaged graph fails only when the quest selects it, and nothing is changed.
    let mut fresh = open();
    start_quest(&mut fresh, GATE);
    let before = fresh.state().clone();
    assert!(fresh.apply(talk(None)).is_err());
    assert_eq!(fresh.state(), &before);
}

#[test]
fn invalid_profile_predicate_and_quest_references_fail_authoring() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/profile.ron");
    let original: gameplay::InteractionProfile = read(&path);
    for change in 0..4 {
        let mut profile = original.clone();
        match change {
            0 => {
                profile.rules[1].priority = profile.rules[0].priority;
                profile.rules[1].order = profile.rules[0].order;
            }
            1 => profile.rules[0].variants[0].weight = 0,
            2 => profile.rules[0].variants[0].dialogue = DialogueId([99; 16]),
            _ => {
                profile.rules[0].condition = gameplay::Condition::ObjectiveCompleted {
                    quest: GATE,
                    objective: key("missing"),
                }
            }
        }
        write(&path, &profile);
        assert!(LoadedProject::load_directory(&root).is_err());
    }
    write(&path, &original);
    let path = root.join("packages/guard/ready.predicate.ron");
    let mut predicate: gameplay::NamedPredicate = read(&path);
    predicate.condition = gameplay::Condition::Named(READY);
    write(path, &predicate);
    assert!(
        LoadedProject::load_directory(&root)
            .err()
            .unwrap()
            .to_string()
            .contains("cycle")
    );
}

#[test]
fn cli_runs_named_guard_exercise_and_rejects_paths_outside_project() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_yarra-game-content"))
        .args([
            "scenario",
            support::sample().to_str().unwrap(),
            "scenarios/guard.ron",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Scenario passed: scenarios/guard.ron")
    );
    assert!(
        LoadedProject::load_directory_with_scenario(support::sample(), "../scenario.ron").is_err()
    );
}

#[test]
fn opening_checks_references_between_always_loaded_definitions() {
    let temp = Temp::new();
    let (project, session) = setup(&temp);
    drop(session);
    let mut predicate = project.content().game.predicates[0].clone();
    if let gameplay::Condition::All(children) = &mut predicate.condition {
        children.push(gameplay::Condition::ObjectiveCompleted {
            quest: GATE,
            objective: key("missing"),
        });
    } else {
        panic!("expected composite guard predicate");
    }
    // A well-formed record with a valid checksum that refers to something that is not there.
    let payload = serde_json::to_vec(&Asset::Predicate(predicate)).unwrap();
    let bundle = temp.0.join("content.sqlite");
    let db = Connection::open(&bundle).unwrap();
    db.execute(
        "UPDATE assets SET payload=?1,byte_len=?2,hash=?3 WHERE kind=?4 AND id=?5",
        params![
            payload,
            payload.len() as i64,
            blake3::hash(&payload).as_bytes().as_slice(),
            AssetKind::Predicate as i64,
            READY.raw()
        ],
    )
    .unwrap();
    let error = GameSession::new(
        ContentRepository::open(bundle).unwrap(),
        project.start().unwrap().into_state(),
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("unknown quest objective"), "{error}");
}

#[test]
fn scripts_are_checked_at_publication_and_run_from_the_published_bundle() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/scripts/guard.luau");
    let original = std::fs::read_to_string(&path).unwrap();
    let failure = |source: &str| {
        std::fs::write(&path, source).unwrap();
        LoadedProject::load_directory(&root)
            .err()
            .unwrap()
            .to_string()
    };
    // A syntax error, a missing function and a module that is not a table all stop the build.
    assert!(failure("local guard = {").contains("script guard"));
    assert!(failure("return {}").contains("unknown script guard.take_key"));
    assert!(failure("return 1").contains("table of functions"));
    std::fs::write(&path, &original).unwrap();

    let (project, mut session) = setup(&temp);
    std::fs::remove_dir_all(temp.0.join("source")).unwrap();
    assert_eq!(session.content().game.scripts.len(), 1);
    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }
    // The scripted reward took the claim, the key and completed the quest.
    let state = session.state();
    assert_eq!(state.quest(GATE).status, Status::Completed);
    assert_eq!(state.claims.len(), 1);
    assert!(
        !state
            .object(session.content(), ObjectId::named("guard/old_gate"))
            .unwrap()
            .locked
    );
}

/// Runs only where the analyzer is available; set YARRA_LUAU_ANALYZE or install Luau.
#[test]
fn the_analyzer_reports_type_errors_at_their_authored_lines() {
    let temp = Temp::new();
    let root = temp.source();
    let project = LoadedProject::load_directory(&root).unwrap();
    let ScriptAnalysis::Checked { modules, warnings } = project.analyze_scripts().unwrap() else {
        eprintln!("luau-analyze not available; skipped");
        return;
    };
    assert_eq!((modules, warnings), (1, vec![]));
    let path = root.join("packages/guard/scripts/guard.luau");
    let source = std::fs::read_to_string(&path).unwrap();
    // Still compiles and still exports take_key, so only the analyzer can object.
    let broken = source.replace(
        "game.complete_quest(\"guard/gate\")",
        "game.complete_qest(\"guard/gate\")\n    game.consume_item(scene.playr, \"old_gate_key\", \"one\")",
    );
    assert_ne!(broken, source);
    std::fs::write(&path, &broken).unwrap();
    let line = broken
        .lines()
        .position(|l| l.contains("complete_qest"))
        .unwrap()
        + 1;
    let error = LoadedProject::load_directory(&root)
        .unwrap()
        .analyze_scripts()
        .unwrap_err()
        .to_string();
    assert!(error.contains(&format!("guard.luau({line},")), "{error}");
    assert!(error.contains("complete_qest"), "{error}");
    assert!(
        error.contains(&format!("guard.luau({},", line + 1)) && error.contains("playr"),
        "{error}"
    );
}
