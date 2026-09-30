use game_types::*;
use gameplay::actors::Position;
use gameplay::dialogue::{self, Mode};
use gameplay::{
    fixtures::{COMPANION, HERO},
    *,
};
use gameplay::{inventory, quests};
use save::{SaveDirectory, SaveSlot};
use std::collections::BTreeSet;
use yarra_game_content::*;
const GUARD: ActorId = ActorId::named("guard");
mod support;
use support::{Temp, read, write};
const GATE: ObjectId = ObjectId::named("guard/old_gate");
const CHEST: ObjectId = ObjectId::named("guard/chest");
const APPROACH: AreaId = AreaId::named("guard/approach");
const POST: AreaId = AreaId::named("guard/gate_post");
const ESCORT: TriggerId = TriggerId::named("guard/escort");
const OPEN_GATE: TriggerId = TriggerId::named("guard/open_gate");
const QUEST: QuestId = QuestId::named("guard/gate");
const CLAIM: ClaimId = ClaimId::named("guard/reward_claim");
const BANTER: DialogueId = DialogueId::named("guard/banter");
const COUNTER: VariableId = VariableId::named("guard/entries");

fn load(temp: &Temp, root: &std::path::Path) -> (LoadedProject, gameplay::GameSession) {
    let project =
        LoadedProject::load_directory_with_scenario(root, "scenarios/guard-gate.ron").unwrap();
    let session = support::runtime(temp, &project);
    assert!(session.content().graphs.loaded() == 0);
    (project, session)
}
fn world(session: &mut gameplay::GameSession, command: WorldCommand) -> Vec<WorldEvent> {
    world_events(session.apply(Command::World(command)).unwrap().events)
}
fn world_events(events: Vec<GameEvent>) -> Vec<WorldEvent> {
    events
        .into_iter()
        .filter_map(|e| match e {
            GameEvent::World(event) => Some(event),
            _ => None,
        })
        .collect()
}
/// The engine's report of where an actor stands, and everything it set off.
fn report(session: &mut gameplay::GameSession, actor: ActorId, areas: &[AreaId]) -> Vec<GameEvent> {
    let command = WorldCommand::Observe {
        actor,
        position: Position {
            millimetres: [areas.len() as i64 * 1000, 0, 0],
        },
        areas: areas.iter().copied().collect(),
    };
    let events = session.apply(Command::World(command)).unwrap().events;
    // Whatever the report set off was carried out before the command returned.
    assert!(!session.world_work_pending());
    events
}
fn observe(
    session: &mut gameplay::GameSession,
    actor: ActorId,
    areas: &[AreaId],
) -> Vec<WorldEvent> {
    world_events(report(session, actor, areas))
}
fn activate(session: &mut gameplay::GameSession) {
    session
        .apply(Command::Quest {
            quest: QUEST,
            transition: quests::Transition::Start,
        })
        .unwrap();
}
fn walking(session: &gameplay::GameSession) -> Option<&Movement> {
    session.state().world.movements.get(&GUARD)
}
/// Quest running, hero in the approach, guard asked to walk.
fn prepare(session: &mut gameplay::GameSession) -> Movement {
    activate(session);
    observe(session, HERO, &[APPROACH]);
    walking(session).expect("guard is walking").clone()
}
fn locked(session: &mut gameplay::GameSession) {
    for object in [GATE, CHEST] {
        assert!(
            session
                .state()
                .object(session.content(), object)
                .unwrap()
                .locked
        );
        assert!(
            session
                .apply(Command::World(WorldCommand::Open { object }))
                .is_err()
        );
    }
    assert!(session.container_contents(CHEST).is_err());
}
fn key_item(session: &gameplay::GameSession, actor: ActorId) -> Option<ItemId> {
    let bag = session.state().carried(actor).unwrap();
    bag.entries
        .iter()
        .find(|i| i.definition == inventory::fixtures::KEY)
        .map(|i| i.id)
}
fn give(session: &mut gameplay::GameSession, item: ItemId, from: ActorId, to: ActorId) {
    session
        .apply(Command::Transfer {
            source: InventoryId(from.0),
            destination: InventoryId(to.0),
            item,
            quantity: 1,
        })
        .unwrap();
}
fn rewarded(session: &gameplay::GameSession) -> bool {
    session.state().claimed(dialogue::ClaimKey {
        claim: CLAIM,
        scope: dialogue::Scope::Playthrough,
    })
}

#[test]
fn the_escort_runs_headlessly_from_entering_the_approach_to_an_open_gate() {
    let temp = Temp::new();
    let (project, session) = load(&temp, &temp.source());
    let mut driver = Driver::new(session, 50, 64).unwrap();
    driver
        .submit(Command::Quest {
            quest: QUEST,
            transition: quests::Transition::Start,
        })
        .unwrap();
    let approach: BTreeSet<AreaId> = [APPROACH].into();
    let entered = driver
        .submit(Command::World(WorldCommand::Observe {
            actor: HERO,
            position: Position {
                millimetres: [1500, 0, 0],
            },
            areas: approach.clone(),
        }))
        .unwrap();
    // Entering sets off the escort before the command returns.
    let entered = world_events(entered.events);
    assert_eq!(
        entered[0],
        WorldEvent::Entered {
            actor: HERO,
            area: APPROACH
        }
    );
    assert!(matches!(entered[1], WorldEvent::MoveRequested(_)));
    assert_eq!(entered[2], WorldEvent::TriggerFired(ESCORT));
    let state = driver.state();
    let movement = &state.world.movements[&GUARD];
    assert_eq!(movement.to, POST);
    assert!(movement.deadline.is_some());
    assert_eq!(state.actor(HERO).unwrap().position.millimetres[0], 1500);
    assert_eq!(state.areas(HERO), &approach);
    assert!(driver.container_contents(CHEST).is_err());
    // Walking is the engine's business; gameplay hears about it when the guard is there.
    let arrived = driver
        .submit(Command::World(WorldCommand::Observe {
            actor: GUARD,
            position: Position {
                millimetres: [1800, 0, 0],
            },
            areas: [POST].into(),
        }))
        .unwrap();
    let arrived = world_events(arrived.events);
    assert!(arrived.contains(&WorldEvent::Arrived {
        actor: GUARD,
        area: POST
    }));
    assert!(arrived.contains(&WorldEvent::TriggerFired(OPEN_GATE)));
    for object in [GATE, CHEST] {
        driver
            .submit(Command::World(WorldCommand::Open { object }))
            .unwrap();
    }
    // The chest is filled from its loot table the first time it is opened: two potions.
    let contents = driver.container_contents(CHEST).unwrap();
    assert_eq!(
        (contents.entries.len(), contents.entries[0].quantity),
        (1, 2)
    );
    let state = driver.state();
    assert_eq!(state.party.experience, 120);
    assert_eq!(state.quest(QUEST).status, quests::Status::Completed);
    assert_eq!(state.trigger(OPEN_GATE).fired, 1);
    assert!(state.world.movements.is_empty() && state.world.pending.is_empty());
    // The source exercise is also validated independently of these Rust observations.
    project.run_scenario().unwrap();
}
#[test]
fn a_pending_walk_resumes_from_every_kind_of_slot() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    let saves = SaveDirectory::new(temp.0.join("saves"), 2).unwrap();
    let library = ContentLibrary::new(temp.0.join("retained")).unwrap();
    library.retain(temp.0.join("content.sqlite")).unwrap();
    activate(&mut session);
    saves
        .save(SaveSlot::Manual(1), &session, "before entering")
        .unwrap();
    observe(&mut session, HERO, &[APPROACH]);
    let movement = walking(&session).unwrap().clone();
    saves.quicksave(&session).unwrap();
    let auto = saves.autosave(&session).unwrap();
    std::fs::remove_dir_all(temp.0.join("source")).unwrap();
    for slot in [SaveSlot::Manual(1), SaveSlot::Quick, auto.slot] {
        let mut restored = library.load(&saves, slot).unwrap();
        locked(&mut restored);
        let entered = observe(&mut restored, HERO, &[APPROACH]);
        // Standing where the save left the hero is not entering the area again; the manual
        // slot was saved outside it, so the walk is asked for now.
        assert_eq!(entered.is_empty(), slot != SaveSlot::Manual(1));
        assert_eq!(walking(&restored).map(|m| m.to), Some(movement.to));
        observe(&mut restored, GUARD, &[POST]);
        assert!(walking(&restored).is_none());
        world(&mut restored, WorldCommand::Open { object: CHEST });
        assert!(restored.container_contents(CHEST).is_ok());
        assert_eq!(restored.state().actor(HERO).unwrap().level, 2);
    }
}
#[test]
fn a_walk_that_fails_or_runs_out_of_time_is_announced_and_can_be_asked_for_again() {
    for timed_out in [false, true] {
        let temp = Temp::new();
        let (_, mut session) = load(&temp, &temp.source());
        let first = prepare(&mut session);
        let events = if timed_out {
            session
                .apply(Command::AdvanceTime { millis: 10_000 })
                .unwrap()
                .events
        } else {
            let failed = session
                .apply(Command::World(WorldCommand::MoveFailed {
                    actor: GUARD,
                    request: first.request,
                }))
                .unwrap();
            failed.events
        };
        assert!(events.contains(&GameEvent::World(WorldEvent::MoveFailed { actor: GUARD })));
        assert!(walking(&session).is_none());
        // A second report about the same walk is about nothing.
        let before = session.state().clone();
        assert!(
            session
                .apply(Command::World(WorldCommand::MoveFailed {
                    actor: GUARD,
                    request: first.request
                }))
                .is_err()
        );
        assert_eq!(session.state(), &before);
        locked(&mut session);
        assert!(!rewarded(&session));
        // Walking out and back in asks the guard again.
        observe(&mut session, HERO, &[]);
        observe(&mut session, HERO, &[APPROACH]);
        let second = walking(&session).unwrap().clone();
        assert!(second.request > first.request);
        observe(&mut session, GUARD, &[POST]);
        world(&mut session, WorldCommand::Open { object: GATE });
        assert!(rewarded(&session));
    }
}
#[test]
fn whichever_listened_event_comes_last_sets_the_guard_walking() {
    // Already inside the approach when the quest starts.
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    observe(&mut session, HERO, &[APPROACH]);
    assert!(walking(&session).is_none());
    activate(&mut session);
    assert!(walking(&session).is_some());

    // Inside with the quest running, but the key arrives later.
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    let item = key_item(&session, HERO).unwrap();
    give(&mut session, item, HERO, COMPANION);
    activate(&mut session);
    observe(&mut session, HERO, &[APPROACH]);
    assert!(walking(&session).is_none());
    give(&mut session, item, COMPANION, HERO);
    assert!(walking(&session).is_some());
}
#[test]
fn occupancy_is_reported_by_the_engine_and_changes_only_on_edges() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    assert!(session.state().areas(HERO).is_empty());
    assert_eq!(
        observe(&mut session, HERO, &[APPROACH, POST]).len(),
        2,
        "entering two areas at once"
    );
    // The same report again changes nothing and queues nothing.
    let queued = session.state().world.pending.len();
    assert!(observe(&mut session, HERO, &[APPROACH, POST]).is_empty());
    assert_eq!(session.state().world.pending.len(), queued);
    assert_eq!(
        observe(&mut session, HERO, &[POST]),
        [WorldEvent::Exited {
            actor: HERO,
            area: APPROACH
        }]
    );
    // Gameplay only accepts areas the content declares, and actors it knows.
    let before = session.state().clone();
    for (actor, area) in [
        (HERO, AreaId::named("guard/nowhere")),
        (ActorId::named("nobody"), POST),
    ] {
        let command = WorldCommand::Observe {
            actor,
            position: Position::default(),
            areas: [area].into(),
        };
        assert!(session.apply(Command::World(command)).is_err());
    }
    assert_eq!(session.state(), &before);
    // An actor already standing in the area it is sent to has arrived at once.
    activate(&mut session);
    observe(&mut session, GUARD, &[POST]);
    let events = observe(&mut session, HERO, &[APPROACH]);
    assert!(events.contains(&WorldEvent::Arrived {
        actor: GUARD,
        area: POST
    }));
    assert!(walking(&session).is_none());
    assert!(rewarded(&session));
}
#[test]
fn a_trigger_whose_actions_fail_changes_nothing_and_stays_armed() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    prepare(&mut session);
    // The hero hands the key away while the guard is walking.
    let item = key_item(&session, HERO).unwrap();
    give(&mut session, item, HERO, COMPANION);
    let events = observe(&mut session, GUARD, &[POST]);
    let failure = events.iter().find_map(|e| match e {
        WorldEvent::TriggerFailed { trigger, reason } => Some((trigger, reason)),
        _ => None,
    });
    let (trigger, reason) = failure.expect("the arrival could not take the key");
    assert_eq!(*trigger, OPEN_GATE);
    assert!(reason.contains("guard.take_key"), "{reason}");
    // Nothing of the failed trigger stuck: no claim, no experience, the chest it would have
    // unlocked after the script is still locked. The guard did arrive.
    assert!(!rewarded(&session));
    assert_eq!(session.state().party.experience, 0);
    assert_eq!(session.state().trigger(OPEN_GATE).fired, 0);
    assert!(session.state().areas(GUARD).contains(&POST));
    locked(&mut session);
    assert!(session.state().world.pending.is_empty());
    // Getting the key back asks the guard again; being at the gate already, he has arrived.
    give(&mut session, item, COMPANION, HERO);
    assert!(rewarded(&session));
    assert_eq!(session.state().trigger(OPEN_GATE).fired, 1);
    world(&mut session, WorldCommand::Open { object: CHEST });
}
/// Adds triggers that count entries into the approach, one per repeat policy, and one that
/// always fails.
fn counting_source(root: &std::path::Path) {
    let mut package: PackageFile = read(root.join("packages/guard/package.ron"));
    let variable = |name: &str| VariableDefinition {
        id: VariableId::try_from(name.to_owned()).unwrap(),
        initial: Value::Int(0),
        scope: Default::default(),
    };
    for (name, repeat) in [
        ("once", TriggerRepeat::Once),
        ("always", TriggerRepeat::Always),
        ("cooldown", TriggerRepeat::Cooldown { millis: 5000 }),
    ] {
        let counter = format!("guard/entries_{name}");
        package.variables.push(variable(&counter));
        let trigger = TriggerDefinition {
            id: TriggerId::try_from(format!("guard/count_{name}")).unwrap(),
            player: HERO,
            speaker: None,
            on: [WorldSignal::Entered {
                actor: HERO,
                area: APPROACH,
            }]
            .into(),
            condition: None,
            actions: vec![Action::Add {
                variable: VariableId::try_from(counter).unwrap(),
                of: None,
                amount: 1,
            }],
            repeat,
        };
        let path = format!("packages/guard/world/count_{name}.ron");
        write(root.join(&path), &trigger);
        package.triggers.push(path);
    }
    package.variables.push(variable("guard/entries"));
    let failing = TriggerDefinition {
        // Named to run before the counters, which still run.
        id: TriggerId::try_from("guard/a_failing".to_owned()).unwrap(),
        player: HERO,
        speaker: None,
        on: [WorldSignal::Entered {
            actor: HERO,
            area: APPROACH,
        }]
        .into(),
        condition: None,
        actions: vec![
            Action::Add {
                variable: COUNTER,
                of: None,
                amount: 1,
            },
            Action::ConsumeItem {
                definition: inventory::fixtures::KEY,
                quantity: 5,
            },
        ],
        repeat: TriggerRepeat::Always,
    };
    write(root.join("packages/guard/world/failing.ron"), &failing);
    package
        .triggers
        .push("packages/guard/world/failing.ron".into());
    write(root.join("packages/guard/package.ron"), &package);
}
#[test]
fn triggers_repeat_once_always_or_after_a_cooldown_and_one_failure_does_not_stop_the_rest() {
    let temp = Temp::new();
    let root = temp.source();
    counting_source(&root);
    let (_, mut session) = load(&temp, &root);
    let count = |session: &gameplay::GameSession, name: &str| {
        let id = VariableId::try_from(format!("guard/entries_{name}")).unwrap();
        session.state().variable(session.content(), id).unwrap()
    };
    let mut failures = 0;
    for visit in 0..3 {
        let events = observe(&mut session, HERO, &[APPROACH]);
        failures += events
            .iter()
            .filter(|e| matches!(e, WorldEvent::TriggerFailed { .. }))
            .count();
        observe(&mut session, HERO, &[]);
        if visit == 1 {
            session
                .apply(Command::AdvanceTime { millis: 5000 })
                .unwrap();
        }
    }
    assert_eq!(count(&session, "once"), Value::Int(1));
    assert_eq!(count(&session, "always"), Value::Int(3));
    // The second visit came before five seconds had passed.
    assert_eq!(count(&session, "cooldown"), Value::Int(2));
    // The failing trigger counted and then failed each time, so its count was undone.
    assert_eq!(failures, 3);
    let content = session.content();
    assert_eq!(
        session.state().variable(content, COUNTER).unwrap(),
        Value::Int(0)
    );
    // The cooldown survives a save.
    let saves = SaveDirectory::new(temp.0.join("saves"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let mut restored = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    observe(&mut restored, HERO, &[APPROACH]);
    assert_eq!(count(&restored, "cooldown"), Value::Int(2));
    assert_eq!(count(&restored, "always"), Value::Int(4));
}
#[test]
fn triggers_that_keep_setting_each_other_off_are_cut_short_and_the_game_goes_on() {
    let temp = Temp::new();
    let root = temp.source();
    let mut package: PackageFile = read(root.join("packages/guard/package.ron"));
    let echoes = VariableId::try_from("guard/echoes".to_owned()).unwrap();
    package.variables.push(VariableDefinition {
        id: echoes,
        initial: Value::Int(0),
        scope: Default::default(),
    });
    // Each echo changes what it listens for.
    let echo = |actor| TriggerDefinition {
        id: TriggerId::try_from("guard/echo".to_owned()).unwrap(),
        player: HERO,
        speaker: None,
        on: [
            WorldSignal::Entered {
                actor,
                area: APPROACH,
            },
            WorldSignal::VariableChanged(echoes),
        ]
        .into(),
        condition: None,
        actions: vec![Action::Add {
            variable: echoes,
            of: None,
            amount: 1,
        }],
        repeat: TriggerRepeat::Always,
    };
    let path = root.join("packages/guard/world/echo.ron");
    write(&path, &echo(HERO));
    package
        .triggers
        .push("packages/guard/world/echo.ron".into());
    write(root.join("packages/guard/package.ron"), &package);
    // A scenario that sets it off fails the build.
    let error = LoadedProject::load_directory_with_scenario(&root, "scenarios/guard-gate.ron")
        .err()
        .expect("looping content accepted")
        .to_string();
    assert!(error.contains("kept setting each other off"), "{error}");
    // Set off by someone the scenario leaves alone, it reaches the game.
    write(&path, &echo(COMPANION));
    let (_, mut session) = load(&temp, &root);
    let count =
        |session: &gameplay::GameSession| match session.state().variable(session.content(), echoes)
        {
            Ok(Value::Int(count)) => count,
            other => panic!("{other:?}"),
        };
    // The command is accepted and returns; the rest waits for the next one.
    let command = WorldCommand::Observe {
        actor: COMPANION,
        position: Position::default(),
        areas: [APPROACH].into(),
    };
    // Watched by a trigger, the companion is observed without being in the party.
    assert!(session.observed(COMPANION) && !session.state().party.members.contains(&COMPANION));
    session.apply(Command::World(command)).unwrap();
    let first = count(&session);
    assert!(first > 1 && session.world_work_pending());
    session.apply(Command::AdvanceTime { millis: 100 }).unwrap();
    assert!(count(&session) > first);
    assert_eq!(session.state().world.pending.len(), 1);
}
#[test]
fn the_engine_reports_the_party_walkers_and_whoever_a_trigger_watches() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    assert!(session.observed(HERO));
    assert!(!session.observed(COMPANION) && !session.observed(GUARD));
    prepare(&mut session);
    assert!(session.observed(GUARD));
    observe(&mut session, GUARD, &[POST]);
    assert!(!session.observed(GUARD));
    join(&mut session, COMPANION);
    assert!(session.observed(COMPANION));
}
#[test]
fn recording_where_everyone_stands_moves_no_one_into_an_area() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    activate(&mut session);
    let at = Position {
        millimetres: [6000, 250, -2600],
    };
    let positions = [(HERO, at.clone()), (GUARD, at.clone())].into();
    assert!(world(&mut session, WorldCommand::Record { positions }).is_empty());
    let state = session.state();
    assert_eq!(state.actor(GUARD).unwrap().position, at);
    assert!(state.areas(HERO).is_empty() && walking(&session).is_none());
    let positions = [(ActorId::named("nobody"), at)].into();
    let command = Command::World(WorldCommand::Record { positions });
    assert!(session.apply(command).is_err());
}
#[test]
fn unknown_and_destroyed_objects_fail_explicitly() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    assert!(
        session
            .apply(Command::World(WorldCommand::Open {
                object: ObjectId([99; 16])
            }))
            .is_err()
    );
    world(&mut session, WorldCommand::Destroy { object: CHEST });
    assert!(
        session
            .apply(Command::World(WorldCommand::SetLocked {
                object: CHEST,
                locked: false
            }))
            .is_err()
    );
    assert!(session.container_contents(CHEST).is_err());
    // A rejected command leaves the generation where it was.
    let generation = session.header().generation;
    assert!(
        session
            .apply(Command::World(WorldCommand::Open { object: CHEST }))
            .is_err()
    );
    assert_eq!(session.header().generation, generation);
}
#[test]
fn a_large_catalog_of_unrelated_triggers_does_not_disturb_dispatch() {
    let temp = Temp::new();
    let root = temp.source();
    let mut package: PackageFile = read(root.join("packages/guard/package.ron"));
    let mut definition: TriggerDefinition = read(root.join("packages/guard/world/escort.ron"));
    definition.on = [WorldSignal::Exited {
        actor: COMPANION,
        area: APPROACH,
    }]
    .into();
    for n in 0..1000u64 {
        let mut id = [0x90; 16];
        id[..8].copy_from_slice(&n.to_le_bytes());
        definition.id = TriggerId(id);
        let path = format!("packages/guard/world/unrelated-{n}.ron");
        write(root.join(&path), &definition);
        package.triggers.push(path);
    }
    write(root.join("packages/guard/package.ron"), &package);
    let (_, mut session) = load(&temp, &root);
    let started = std::time::Instant::now();
    prepare(&mut session);
    // Dispatch consults the subscription index; it never evaluates the unrelated triggers.
    assert!(started.elapsed().as_millis() < 250);
    assert_eq!(session.state().world.triggers.len(), 1);
    assert!(session.state().trigger(ESCORT).fired >= 1);
    assert!(walking(&session).is_some());
}
fn join(session: &mut gameplay::GameSession, actor: ActorId) {
    session
        .apply(Command::Party {
            actor,
            member: true,
        })
        .unwrap();
}
#[test]
fn walking_into_an_area_starts_an_ambient_conversation_with_a_companion() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    // Without the companion in the party nothing is said.
    assert!(
        !report(&mut session, HERO, &[APPROACH])
            .iter()
            .any(|e| matches!(e, GameEvent::DialogueStarted { .. }))
    );
    assert!(session.content().graphs.loaded() == 0);
    observe(&mut session, HERO, &[]);

    join(&mut session, HERO);
    join(&mut session, COMPANION);
    let banter = ConversationKey {
        dialogue: BANTER,
        participant: HERO,
        speaker: COMPANION,
    };
    let events = report(&mut session, HERO, &[APPROACH]);
    assert!(events.contains(&GameEvent::DialogueStarted {
        key: banter,
        mode: Mode::Ambient
    }));
    // The graph was read when the conversation started, not before.
    assert_eq!(session.content().graphs.loaded(), 1);
    let mut speakers = Vec::new();
    loop {
        let view = session.conversation_view(banter).unwrap();
        let Some(line) = view.line else {
            assert!(view.choices.is_empty());
            break;
        };
        speakers.push(line.speaker);
        session
            .apply(Command::AdvanceLine {
                key: banter,
                expected: view.token,
            })
            .unwrap();
    }
    assert_eq!(speakers, [COMPANION, HERO]);
    let run = session.state().conversation(banter).unwrap();
    assert_eq!(run.status, dialogue::RunStatus::Completed);
    // Said once: coming back does not start it again.
    observe(&mut session, HERO, &[]);
    assert!(
        !report(&mut session, HERO, &[APPROACH])
            .iter()
            .any(|e| matches!(e, GameEvent::DialogueStarted { .. }))
    );
}
#[test]
fn a_conversation_that_cannot_start_is_refused_and_the_queue_moves_on() {
    let temp = Temp::new();
    let (project, session) = load(&temp, &temp.source());
    drop(session);
    let bundle = temp.0.join("content.sqlite");
    let db = rusqlite::Connection::open(&bundle).unwrap();
    db.execute(
        "UPDATE assets SET hash=zeroblob(32) WHERE kind=?1 AND id=?2",
        rusqlite::params![AssetKind::Dialogue as i64, BANTER.raw()],
    )
    .unwrap();
    let mut session = GameSession::new(
        ContentRepository::open(&bundle).unwrap(),
        project.start().unwrap().into_state(),
    )
    .unwrap();
    join(&mut session, HERO);
    join(&mut session, COMPANION);
    activate(&mut session);
    let events = report(&mut session, HERO, &[APPROACH]);
    let refused = events.iter().any(|e| {
        matches!(e, GameEvent::World(WorldEvent::DialogueRefused { dialogue, reason })
            if *dialogue == BANTER && reason.contains("checksum"))
    });
    assert!(refused, "{events:?}");
    assert!(session.state().conversations.is_empty());
    // The rest of the event's work was not held up.
    assert!(session.state().world.pending.is_empty());
    assert!(walking(&session).is_some());
}
#[test]
fn authoring_rejects_ambient_choices_undeclared_areas_and_unknown_dialogues() {
    let temp = Temp::new();
    let root = temp.source();
    let failure = |root: &std::path::Path| {
        LoadedProject::load_directory(root)
            .err()
            .expect("invalid project accepted")
            .to_string()
    };
    let path = root.join("packages/guard/conversations/reward/graph.ron");
    let original: dialogue::Dialogue = read(&path);
    let mut graph = original.clone();
    graph.mode = Mode::Ambient;
    write(&path, &graph);
    assert!(failure(&root).contains("cannot offer choices"));
    write(&path, &original);

    let path = root.join("packages/guard/world/banter.ron");
    let original: TriggerDefinition = read(&path);
    let mut trigger = original.clone();
    trigger.on = [WorldSignal::Entered {
        actor: HERO,
        area: AreaId::named("guard/undeclared"),
    }]
    .into();
    write(&path, &trigger);
    assert!(failure(&root).contains("unknown area"));
    let mut trigger = original.clone();
    trigger.actions = vec![Action::StartDialogue {
        dialogue: DialogueId::named("guard/unwritten"),
        speaker: None,
    }];
    write(&path, &trigger);
    assert!(failure(&root).contains("unknown dialogue"));
    let mut trigger = original.clone();
    trigger.on.clear();
    write(&path, &trigger);
    assert!(failure(&root).contains("listens for"));
}
#[test]
fn asking_again_for_a_walk_under_way_changes_nothing() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    activate(&mut session);
    // Entering twice while the guard walks satisfies the escort twice; he is asked once.
    let mut events = observe(&mut session, HERO, &[APPROACH]);
    observe(&mut session, HERO, &[]);
    events.extend(observe(&mut session, HERO, &[APPROACH]));
    let requests = events
        .iter()
        .filter(|e| matches!(e, WorldEvent::MoveRequested(_)))
        .count();
    assert_eq!(requests, 1);
    assert_eq!(session.state().trigger(ESCORT).fired, 2);
}
