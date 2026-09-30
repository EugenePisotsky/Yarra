use actors::RelationshipKey;
use dialogue::InteractionKey;
use game_types::*;
use gameplay::{
    Command, GameEvent, GameSession, StateRequest, StateStore, StateTransaction,
    fixtures::{HERO, MERCHANT, key},
};
use quests::{Status, Transition};
use rusqlite::{Connection, params};
use save::{SaveDirectory, SaveSlot, WorkingStore};
use yarra_game_content::*;
mod support;
use support::{Temp, read, write};

const GATE: QuestId = QuestId([0x20; 16]);
const SUPPLIES: QuestId = QuestId([0x21; 16]);
const PROFILE: InteractionProfileId = InteractionProfileId([0x22; 16]);
const READY: PredicateId = PredicateId([0x23; 16]);
const REWARD: DialogueId = DialogueId([0x30; 16]);
const DUTY: DialogueId = DialogueId([0x31; 16]);
const INTERACTION: InteractionKey = InteractionKey {
    participant: HERO,
    speaker: MERCHANT,
};
const ATTITUDE: RelationshipKey = RelationshipKey {
    from: MERCHANT,
    to: HERO,
};
fn setup(temp: &Temp) -> (LoadedProject, RuntimeSession) {
    let project =
        LoadedProject::load_directory_with_scenario(temp.source(), "scenarios/guard.ron").unwrap();
    let seed = project
        .start()
        .unwrap()
        .store()
        .export_for_tools(100_000)
        .unwrap();
    let bundle = temp.0.join("content.sqlite");
    project.build(&bundle).unwrap();
    let store = WorkingStore::create(temp.0.join("live.sqlite"), project.content(), &seed).unwrap();
    let session = GameSession::new(
        store,
        ContentRepository::open(bundle, Default::default()).unwrap(),
    )
    .unwrap();
    (project, session)
}
fn talk(topic: Option<&str>) -> Command {
    Command::Talk {
        bindings: Default::default(),
        participant: HERO,
        speaker: MERCHANT,
        topic: topic.map(key),
    }
}
fn choose() -> Command {
    Command::Choose {
        expected: dialogue::Token { run: 1, step: 2 },
        dialogue: REWARD,
        participant: HERO,
        speaker: MERCHANT,
        choice: key("return-key"),
    }
}
fn selection(session: &mut RuntimeSession) -> dialogue::Selection {
    session
        .query(StateRequest {
            interactions: [INTERACTION].into(),
            ..Default::default()
        })
        .unwrap()
        .interaction(INTERACTION)
        .unwrap()
        .current
        .clone()
        .unwrap()
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
    let state = session.store().export_for_tools(100_000).unwrap();
    assert_eq!(state.quest(GATE).unwrap().status, Status::Completed);
    assert!(
        state
            .quest(GATE)
            .unwrap()
            .completed
            .contains(&key("deliver"))
    );
    assert_eq!(state.quest(SUPPLIES).unwrap().status, Status::Active);
    assert_eq!(state.relationship(ATTITUDE).unwrap().attitude, 5);
    assert_eq!(state.actor(HERO).unwrap().skills[&key("persuasion")], 10);
    assert!(
        !state
            .carried(HERO)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.definition == inventory::fixtures::KEY)
    );
    assert_eq!(selection(&mut session).dialogue, REWARD);
    let before = session.store().export_for_tools(100_000).unwrap();
    assert!(session.apply(choose()).is_err());
    assert_eq!(session.store().export_for_tools(100_000).unwrap(), before);
    let saves = SaveDirectory::new(temp.0.join("slots"), 2).unwrap();
    saves.quicksave(&session).unwrap();
    let library = ContentLibrary::new(temp.0.join("retained"), Default::default()).unwrap();
    library.retain(temp.0.join("content.sqlite")).unwrap();
    std::fs::remove_dir_all(temp.0.join("source")).unwrap();
    let mut loaded = library
        .load(&saves, SaveSlot::Quick, temp.0.join("restored.sqlite"))
        .unwrap();
    assert_eq!(loaded.store().stats().decoded_records, 0);
    assert_eq!(loaded.content_source().stats().decoded_assets, 0);
    assert_eq!(loaded.store().export_for_tools(100_000).unwrap(), state);
    assert_eq!(
        loaded
            .preview_interaction(HERO, MERCHANT)
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
    let before = session.store().export_for_tools(100_000).unwrap();
    let preview = session.preview_interaction(HERO, MERCHANT).unwrap();
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
    assert_eq!(session.store().export_for_tools(100_000).unwrap(), before);
    session
        .apply(Command::AdjustRelationship {
            key: ATTITUDE,
            amount: -20,
        })
        .unwrap();
    assert_eq!(
        session
            .preview_interaction(HERO, MERCHANT)
            .unwrap()
            .opening()
            .unwrap()
            .rule,
        key("hostile")
    );
    start_quest(&mut session, GATE);
    start_quest(&mut session, SUPPLIES);
    let header = session.header().unwrap();
    let preview = session.preview_interaction(HERO, MERCHANT).unwrap();
    assert_eq!(preview.opening().unwrap().rule, key("reward"));
    assert_eq!(
        preview
            .topics()
            .map(|r| r.rule.as_str())
            .collect::<Vec<_>>(),
        ["gate-topic", "supply-topic"]
    );
    assert_eq!(session.header().unwrap(), header);
    session.apply(talk(Some("supply-topic"))).unwrap();
    assert_eq!(selection(&mut session).dialogue, DUTY);
}

#[test]
fn weighted_selection_replays_after_restore_and_active_conversation_resumes() {
    let temp = Temp::new();
    let (_, mut session) = setup(&temp);
    let saves = SaveDirectory::new(temp.0.join("slots"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let before = session.header().unwrap();
    session.apply(talk(None)).unwrap();
    let chosen = selection(&mut session);
    let after = session.header().unwrap();
    assert_ne!(before.narrative_random, after.narrative_random);
    assert_eq!(before.random, after.random);
    assert!([DialogueId([0x32; 16]), DialogueId([0x33; 16])].contains(&chosen.dialogue));
    let mut replay = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite"), Default::default()).unwrap(),
            temp.0.join("replay.sqlite"),
        )
        .unwrap();
    replay.apply(talk(None)).unwrap();
    assert_eq!(selection(&mut replay), chosen);
    assert_eq!(replay.header().unwrap(), after);
    // A new higher-priority quest must not silently replace a conversation in progress.
    start_quest(&mut session, GATE);
    let before_resume = session.header().unwrap();
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
    assert_eq!(selection(&mut session), chosen);
    saves.quicksave(&session).unwrap();
    let mut restored = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite"), Default::default()).unwrap(),
            temp.0.join("active.sqlite"),
        )
        .unwrap();
    assert_eq!(selection(&mut restored), chosen);
    let before = restored.header().unwrap();
    assert!(restored.apply(talk(Some("gate-topic"))).is_err());
    assert_eq!(restored.header().unwrap(), before);
    assert_eq!(restored.apply(talk(None)).unwrap().events, outcome.events);
}

#[test]
fn directed_relationships_have_explicit_neutral_defaults_and_clamp_adjustments() {
    let temp = Temp::new();
    let (_, mut session) = setup(&temp);
    let inverse = RelationshipKey {
        from: HERO,
        to: MERCHANT,
    };
    let empty = session.query(StateRequest::default()).unwrap();
    assert!(empty.relationship(ATTITUDE).is_err());
    assert!(empty.quest(GATE).is_err());
    session
        .apply(Command::AdjustRelationship {
            key: ATTITUDE,
            amount: i16::MIN,
        })
        .unwrap();
    let state = session
        .query(StateRequest {
            relationships: [ATTITUDE, inverse].into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(state.relationship(ATTITUDE).unwrap().attitude, -100);
    assert_eq!(state.relationship(inverse).unwrap().attitude, 0);
    session
        .apply(Command::AdjustRelationship {
            key: ATTITUDE,
            amount: i16::MAX,
        })
        .unwrap();
    assert_eq!(
        session
            .query(StateRequest {
                relationships: [ATTITUDE].into(),
                ..Default::default()
            })
            .unwrap()
            .relationship(ATTITUDE)
            .unwrap()
            .attitude,
        100
    );
    let before = session.header().unwrap();
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
            .query(StateRequest {
                quests: [QuestId([99; 16])].into(),
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(session.header().unwrap(), before);
}

#[test]
fn storage_failures_roll_back_selection_rng_and_all_quest_rewards() {
    let temp = Temp::new();
    let (_, mut session) = setup(&temp);
    let db = Connection::open(temp.0.join("live.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_interaction BEFORE INSERT ON records WHEN NEW.kind=7 BEGIN SELECT RAISE(ABORT,'test interaction failure'); END;").unwrap();
    let before = session.store().export_for_tools(100_000).unwrap();
    assert!(
        session
            .apply(talk(None))
            .unwrap_err()
            .to_string()
            .contains("test interaction failure")
    );
    assert_eq!(session.store().export_for_tools(100_000).unwrap(), before);
    db.execute_batch("DROP TRIGGER reject_interaction;")
        .unwrap();
    start_quest(&mut session, GATE);
    session.apply(talk(None)).unwrap();
    for step in 0..2 {
        session
            .apply(Command::AdvanceLine {
                key: gameplay::ConversationKey {
                    dialogue: REWARD,
                    participant: HERO,
                    speaker: MERCHANT,
                },
                expected: dialogue::Token { run: 1, step },
            })
            .unwrap();
    }
    db.execute_batch("CREATE TRIGGER reject_quest BEFORE UPDATE ON records WHEN NEW.kind=5 BEGIN SELECT RAISE(ABORT,'test quest failure'); END;").unwrap();
    let before = session.store().export_for_tools(100_000).unwrap();
    assert!(
        session
            .apply(choose())
            .unwrap_err()
            .to_string()
            .contains("test quest failure")
    );
    assert_eq!(session.store().export_for_tools(100_000).unwrap(), before);
    db.execute_batch("DROP TRIGGER reject_quest;").unwrap();
    session.apply(choose()).unwrap();
    assert_eq!(
        session
            .query(StateRequest {
                quests: [GATE].into(),
                ..Default::default()
            })
            .unwrap()
            .quest(GATE)
            .unwrap()
            .status,
        Status::Completed
    );
}

#[test]
fn profiles_load_predicates_but_only_selected_dialogue_graphs_are_read() {
    let temp = Temp::new();
    let (_, session) = setup(&temp);
    drop(session);
    let bundle = temp.0.join("content.sqlite");
    // Corruption in a candidate graph cannot affect profile inspection or another selection.
    let db = Connection::open(&bundle).unwrap();
    db.execute(
        "UPDATE assets SET hash=zeroblob(32) WHERE kind=?1 AND id=?2",
        params![AssetKind::Dialogue as i64, REWARD.to_string()],
    )
    .unwrap();
    let mut session = GameSession::new(
        WorkingStore::open(temp.0.join("live.sqlite")).unwrap(),
        ContentRepository::open(&bundle, Default::default()).unwrap(),
    )
    .unwrap();
    let mut repo = ContentRepository::open(&bundle, Default::default()).unwrap();
    let set = repo.load(&[AssetId::Profile(PROFILE)]).unwrap();
    assert!(set.get(&AssetId::Predicate(READY)).is_ok());
    assert!(set.get(&AssetId::Quest(GATE)).is_ok());
    assert!(!set.iter().any(|(id, _)| id.kind() == AssetKind::Dialogue));
    session.preview_interaction(HERO, MERCHANT).unwrap();
    session.apply(talk(None)).unwrap();
    assert_eq!(selection(&mut session).rule, key("welcome"));
    assert!(repo.load(&[AssetId::Dialogue(REWARD)]).is_err());
    // A fresh session sees the same failed graph when the quest selects it.
    let project =
        LoadedProject::load_directory_with_scenario(temp.0.join("source"), "scenarios/guard.ron")
            .unwrap();
    let seed = project
        .start()
        .unwrap()
        .store()
        .export_for_tools(100_000)
        .unwrap();
    let mut fresh = GameSession::new(
        WorkingStore::in_memory(project.content(), &seed).unwrap(),
        ContentRepository::open(bundle, Default::default()).unwrap(),
    )
    .unwrap();
    start_quest(&mut fresh, GATE);
    let before = fresh.store().export_for_tools(100_000).unwrap();
    assert!(fresh.apply(talk(None)).is_err());
    assert_eq!(fresh.store().export_for_tools(100_000).unwrap(), before);
}

#[test]
fn absent_state_records_are_budgeted_and_not_inserted_by_reads() {
    let temp = Temp::new();
    let (_, mut session) = setup(&temp);
    // Defaults are issued by the store after an indexed absence lookup, not by domain accessors.
    let before = session.store().export_for_tools(100_000).unwrap();
    let mut store = WorkingStore::open(temp.0.join("live.sqlite")).unwrap();
    {
        let mut tx = store.begin().unwrap();
        let mut request = StateRequest {
            quests: (0..64u8).map(|i| QuestId([i; 16])).collect(),
            ..Default::default()
        };
        let batch = tx.load(&request).unwrap();
        assert_eq!(batch.versions.len(), 64);
        assert!(batch.versions.values().all(|v| *v == 0));
        assert!(
            batch
                .state
                .quests
                .iter()
                .all(|q| q.status == Status::NotStarted)
        );
        request.quests.insert(QuestId([64; 16]));
        assert!(tx.load(&request).is_err());
    }
    assert_eq!(store.stats().peak_working_records, 64);
    assert_eq!(session.store().export_for_tools(100_000).unwrap(), before);
    assert!(
        session
            .query(StateRequest {
                relationships: (0..64u8)
                    .map(|i| RelationshipKey {
                        from: HERO,
                        to: ActorId([i; 16])
                    })
                    .collect(),
                ..Default::default()
            })
            .is_err()
    );
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
fn indexed_reader_checks_objective_references_after_checksums_and_dependencies() {
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
            READY.to_string()
        ],
    )
    .unwrap();
    let mut repo = ContentRepository::open(bundle, Default::default()).unwrap();
    let error = repo
        .load(&[AssetId::Profile(PROFILE)])
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("unknown quest objective"), "{error}");
}
