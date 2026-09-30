use game_types::*;
use gameplay::actors::Position;
use gameplay::{dialogue, inventory, quests};
use gameplay::{
    fixtures::{COMPANION, HERO, MERCHANT, key},
    *,
};
use save::{SaveDirectory, SaveSlot};
use yarra_game_content::*;
mod support;
use support::{Temp, read, write};
const GATE: ObjectId = ObjectId([0x51; 16]);
const CHEST: ObjectId = ObjectId([0x52; 16]);
const AREA: AreaId = AreaId([0x53; 16]);
const TRIGGER: TriggerId = TriggerId([0x54; 16]);
const QUEST: QuestId = QuestId([0x20; 16]);
fn point(x: i64) -> Position {
    Position {
        millimetres: [x, 0, 0],
    }
}
fn load(temp: &Temp, root: &std::path::Path) -> (LoadedProject, RuntimeSession) {
    let project =
        LoadedProject::load_directory_with_scenario(root, "scenarios/guard-gate.ron").unwrap();
    let session = support::runtime(temp, &project);
    assert!(session.content().game.dialogues.is_empty());
    (project, session)
}
fn world(session: &mut RuntimeSession, cmd: WorldCommand) -> CommandOutcome {
    session.apply(Command::World(cmd)).unwrap()
}
fn pump(session: &mut RuntimeSession) {
    for _ in 0..256 {
        if !session.world_work_pending() {
            return;
        }
        world(session, WorldCommand::ProcessNext);
    }
    panic!("queue did not drain");
}
fn enter(session: &mut RuntimeSession, n: u64, x: i64) {
    world(
        session,
        WorldCommand::ObservePosition {
            actor: HERO,
            observation: n,
            position: point(x),
        },
    );
}
fn activate(session: &mut RuntimeSession) {
    session
        .apply(Command::Quest {
            quest: QUEST,
            transition: quests::Transition::Start,
        })
        .unwrap();
}
fn pending(session: &mut RuntimeSession) -> MoveRequest {
    session.next_movement(None).unwrap()
}
fn prepare(session: &mut RuntimeSession) -> MoveRequest {
    activate(session);
    enter(session, 1, 1500);
    pump(session);
    pending(session)
}
fn locked(session: &mut RuntimeSession) {
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
fn no_reward(session: &mut RuntimeSession) {
    let s = session.state();
    assert_eq!(s.actor(HERO).unwrap().skills.get(&key("persuasion")), None);
    assert!(
        s.carried(HERO)
            .unwrap()
            .entries
            .iter()
            .any(|i| i.definition == inventory::fixtures::KEY)
    );
}
fn arrive(session: &mut RuntimeSession, m: &MoveRequest) -> CommandOutcome {
    world(
        session,
        WorldCommand::FinishMove {
            id: m.id,
            result: MoveResult::Arrived {
                position: m.destination.clone(),
            },
        },
    )
}
#[test]
fn authored_guard_gate_runs_through_the_same_headless_commands_and_normal_access() {
    let temp = Temp::new();
    let (project, session) = load(&temp, &temp.source());
    let mut driver = HeadlessDriver::new(session, 50, 64).unwrap();
    driver
        .submit(Command::Quest {
            quest: QUEST,
            transition: quests::Transition::Start,
        })
        .unwrap();
    driver
        .submit(Command::World(WorldCommand::ObservePosition {
            actor: HERO,
            observation: 1,
            position: point(1500),
        }))
        .unwrap();
    assert!(!driver.pump_world(1).unwrap());
    assert!(driver.pump_world(256).unwrap());
    let m = driver.next_movement(None).unwrap();
    assert_eq!(m.phase, MovementPhase::Accepted);
    assert_eq!(
        driver.state().actor(MERCHANT).unwrap().position,
        Position {
            millimetres: [100000000, 0, 100000000]
        }
    );
    assert!(driver.container_contents(CHEST).is_err());
    driver
        .submit(Command::World(WorldCommand::StartMove { id: m.id }))
        .unwrap();
    driver
        .submit(Command::World(WorldCommand::FinishMove {
            id: m.id,
            result: MoveResult::Arrived {
                position: m.destination.clone(),
            },
        }))
        .unwrap();
    driver
        .submit(Command::World(WorldCommand::Open { object: GATE }))
        .unwrap();
    driver
        .submit(Command::World(WorldCommand::Open { object: CHEST }))
        .unwrap();
    assert_eq!(driver.container_contents(CHEST).unwrap().entries.len(), 0);
    let s = driver.state();
    assert_eq!(s.actor(HERO).unwrap().skills[&key("persuasion")], 10);
    assert_eq!(s.actor(MERCHANT).unwrap().position, m.destination);
    assert_eq!(s.quest(QUEST).status, quests::Status::Completed);
    assert_eq!(s.trigger(TRIGGER).successes, 1);
    assert!(driver.pump_world(256).unwrap());
    assert!(driver.next_movement(None).is_none());
    // The source exercise is also validated independently of synthetic Rust observations.
    project.run_scenario().unwrap();
}
#[test]
fn queued_events_and_running_actions_resume_from_all_slot_types_without_sources() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    let saves = SaveDirectory::new(temp.0.join("saves"), 2).unwrap();
    let library = ContentLibrary::new(temp.0.join("retained")).unwrap();
    library.retain(temp.0.join("content.sqlite")).unwrap();
    activate(&mut session);
    enter(&mut session, 1, 1500);
    saves
        .save(SaveSlot::Manual(1), &session, "before event dispatch")
        .unwrap();
    pump(&mut session);
    let m = pending(&mut session);
    saves.quicksave(&session).unwrap();
    world(&mut session, WorldCommand::StartMove { id: m.id });
    let auto = saves.autosave(&session).unwrap();
    std::fs::remove_dir_all(temp.0.join("source")).unwrap();
    for slot in [SaveSlot::Manual(1), SaveSlot::Quick, auto.slot] {
        let mut restored = library.load(&saves, slot).unwrap();
        locked(&mut restored);
        pump(&mut restored);
        assert_eq!(pending(&mut restored).id, m.id);
        arrive(&mut restored, &m);
        assert!(arrive(&mut restored, &m).events.is_empty());
        assert!(
            restored
                .apply(Command::World(WorldCommand::FinishMove {
                    id: m.id,
                    result: MoveResult::Cancelled
                }))
                .is_err()
        );
        world(&mut restored, WorldCommand::Open { object: CHEST });
        assert!(restored.container_contents(CHEST).is_ok());
        let s = restored.state();
        assert_eq!(s.actor(HERO).unwrap().skills[&key("persuasion")], 10);
    }
}
#[test]
fn failed_cancelled_and_timed_out_moves_stop_before_rewards_and_allow_fresh_attempts() {
    for result in [
        MoveResult::Failed {
            reason: key("unreachable"),
        },
        MoveResult::Cancelled,
        MoveResult::TimedOut,
    ] {
        let temp = Temp::new();
        let (_, mut session) = load(&temp, &temp.source());
        let first = prepare(&mut session);
        locked(&mut session);
        world(&mut session, WorldCommand::StartMove { id: first.id });
        if result == MoveResult::TimedOut {
            session
                .apply(Command::AdvanceTime { millis: 10000 })
                .unwrap();
            pump(&mut session);
        } else {
            world(
                &mut session,
                WorldCommand::FinishMove {
                    id: first.id,
                    result: result.clone(),
                },
            );
        }
        assert!(
            world(
                &mut session,
                WorldCommand::FinishMove {
                    id: first.id,
                    result: result.clone()
                }
            )
            .events
            .is_empty()
        );
        assert!(session.next_movement(None).is_none());
        locked(&mut session);
        no_reward(&mut session);
        enter(&mut session, 2, 0);
        pump(&mut session);
        enter(&mut session, 3, 1500);
        pump(&mut session);
        let second = pending(&mut session);
        assert_eq!(second.id.run, 2);
        assert!(
            session
                .apply(Command::World(WorldCommand::FinishMove {
                    id: first.id,
                    result
                }))
                .is_err()
        );
        arrive(&mut session, &second);
        world(&mut session, WorldCommand::Open { object: GATE });
    }
}
#[test]
fn maintained_conditions_reconcile_existing_items_inside_area_and_later_acquisition() {
    for acquire_later in [false, true] {
        let temp = Temp::new();
        let (_, mut session) = load(&temp, &temp.source());
        let id = session
            .state()
            .carried(HERO)
            .unwrap()
            .entries
            .iter()
            .find(|i| i.definition == inventory::fixtures::KEY)
            .unwrap()
            .id;
        if acquire_later {
            session
                .apply(Command::Transfer {
                    source: InventoryId(HERO.0),
                    destination: InventoryId(MERCHANT.0),
                    item: id,
                    quantity: 1,
                })
                .unwrap();
        }
        enter(&mut session, 1, 1500);
        pump(&mut session);
        assert!(session.next_movement(None).is_none());
        activate(&mut session);
        pump(&mut session);
        if acquire_later {
            assert!(session.next_movement(None).is_none());
            session
                .apply(Command::Transfer {
                    source: InventoryId(MERCHANT.0),
                    destination: InventoryId(HERO.0),
                    item: id,
                    quantity: 1,
                })
                .unwrap();
            pump(&mut session);
        }
        assert_eq!(pending(&mut session).id.run, 1);
    }
}
#[test]
fn ordered_position_observations_are_idempotent_and_use_half_open_area_bounds() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    activate(&mut session);
    enter(&mut session, 1, 2000);
    pump(&mut session);
    assert!(session.next_movement(None).is_none());
    enter(&mut session, 2, 1000);
    let before = session.state().clone();
    assert!(
        world(
            &mut session,
            WorldCommand::ObservePosition {
                actor: HERO,
                observation: 2,
                position: point(1000)
            }
        )
        .events
        .is_empty()
    );
    assert!(
        session
            .apply(Command::World(WorldCommand::ObservePosition {
                actor: HERO,
                observation: 1,
                position: point(2000)
            }))
            .is_err()
    );
    assert!(
        session
            .apply(Command::World(WorldCommand::ObservePosition {
                actor: HERO,
                observation: 2,
                position: point(1500)
            }))
            .is_err()
    );
    let after = session.state().clone();
    assert_eq!(before.world, after.world);
    pump(&mut session);
    let m = pending(&mut session);
    assert!(
        session
            .apply(Command::World(WorldCommand::FinishMove {
                id: m.id,
                result: MoveResult::Arrived {
                    position: point(1799)
                }
            }))
            .is_err()
    );
    locked(&mut session);
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
    definition.activation = TriggerActivation::Events(
        [WorldSignal::Exited {
            actor: COMPANION,
            area: AREA,
        }]
        .into(),
    );
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
    let m = prepare(&mut session);
    // Dispatch consults the subscription index; it never evaluates the unrelated triggers.
    assert!(started.elapsed().as_millis() < 250);
    assert_eq!(m.id.trigger, TRIGGER);
    assert_eq!(session.state().world.triggers.len(), 1);
}

#[test]
fn arrival_continuation_failure_preserves_arrival_but_rolls_back_the_reward_group() {
    let temp = Temp::new();
    let (_, mut session) = load(&temp, &temp.source());
    let m = prepare(&mut session);
    let item = session
        .state()
        .carried(HERO)
        .unwrap()
        .entries
        .iter()
        .find(|i| i.definition == inventory::fixtures::KEY)
        .unwrap()
        .id;
    session
        .apply(Command::Transfer {
            source: InventoryId(HERO.0),
            destination: InventoryId(COMPANION.0),
            item,
            quantity: 1,
        })
        .unwrap();
    arrive(&mut session, &m);
    let state = session.state();
    let t = state.trigger(TRIGGER);
    assert!(matches!(t.status, SequenceStatus::Failed { .. }));
    assert!(t.diagnostic.is_some());
    assert!(t.movement.is_none());
    assert_eq!(state.actor(MERCHANT).unwrap().position, m.destination);
    assert!(state.actor(HERO).unwrap().skills.is_empty());
    assert!(!state.claimed(dialogue::ClaimKey {
        claim: ClaimId([0x40; 16]),
        scope: dialogue::Scope::Playthrough
    }));
    locked(&mut session);
    assert!(arrive(&mut session, &m).events.is_empty());
    session
        .apply(Command::Transfer {
            source: InventoryId(COMPANION.0),
            destination: InventoryId(HERO.0),
            item,
            quantity: 1,
        })
        .unwrap();
    pump(&mut session);
    let second = pending(&mut session);
    assert_eq!(second.id.run, 2);
    arrive(&mut session, &second);
}

#[test]
fn immediate_plan_failure_has_bounded_retries_and_does_not_claim_partial_rewards() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/world/escort.ron");
    let mut trigger: TriggerDefinition = read(&path);
    trigger.steps = vec![SequenceStep::Apply(vec![Action::Claim {
        claim: ClaimId([0x40; 16]),
        actions: vec![
            Action::AwardExperience {
                skill: key("persuasion"),
                amount: 10,
            },
            Action::ConsumeItem {
                definition: inventory::fixtures::KEY,
                quantity: 2,
            },
        ],
    }])];
    write(path, &trigger);
    let path = root.join("scenarios/guard-gate.ron");
    let mut scenario: Scenario = read(&path);
    scenario.steps.clear();
    write(path, &scenario);
    let (_, mut session) = load(&temp, &root);
    activate(&mut session);
    enter(&mut session, 1, 1500);
    pump(&mut session);
    for n in 2..8 {
        enter(&mut session, n, 1500 + n as i64);
        pump(&mut session);
    }
    let state = session.state();
    let t = state.trigger(TRIGGER);
    assert_eq!(t.run, 3);
    assert_eq!(t.consecutive_failures, 3);
    assert_eq!(t.successes, 0);
    assert!(t.diagnostic.is_some());
    no_reward(&mut session);
    locked(&mut session);
    world(
        &mut session,
        WorldCommand::RetryTrigger { trigger: TRIGGER },
    );
    let state = session.state();
    assert_eq!(state.trigger(TRIGGER).run, 4);
    assert_eq!(state.trigger(TRIGGER).consecutive_failures, 1);
}

#[test]
fn movement_has_exclusive_ownership_and_explicit_retry_after_cancellation() {
    let temp = Temp::new();
    let root = temp.source();
    let mut package: PackageFile = read(root.join("packages/guard/package.ron"));
    let mut trigger: TriggerDefinition = read(root.join("packages/guard/world/escort.ron"));
    trigger.id = TriggerId([0x55; 16]);
    write(root.join("packages/guard/world/second.ron"), &trigger);
    package
        .triggers
        .push("packages/guard/world/second.ron".into());
    write(root.join("packages/guard/package.ron"), &package);
    let (_, mut session) = load(&temp, &root);
    let first = prepare(&mut session);
    let state = session.state();
    assert!(matches!(
        state.trigger(trigger.id).status,
        SequenceStatus::Failed { .. }
    ));
    assert!(session.next_movement(Some(TRIGGER)).is_none());
    world(
        &mut session,
        WorldCommand::FinishMove {
            id: first.id,
            result: MoveResult::Cancelled,
        },
    );
    world(
        &mut session,
        WorldCommand::RetryTrigger {
            trigger: trigger.id,
        },
    );
    let second = pending(&mut session);
    assert_eq!(second.id.trigger, trigger.id);
    arrive(&mut session, &second);
    world(&mut session, WorldCommand::Open { object: GATE });
}

#[test]
fn event_triggers_keep_edge_semantics() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/world/escort.ron");
    let mut trigger: TriggerDefinition = read(&path);
    trigger.activation = TriggerActivation::Events(
        [WorldSignal::Entered {
            actor: HERO,
            area: AREA,
        }]
        .into(),
    );
    write(path, &trigger);
    let (_, mut session) = load(&temp, &root);
    enter(&mut session, 1, 1500);
    pump(&mut session);
    activate(&mut session);
    pump(&mut session);
    assert!(session.next_movement(None).is_none());
    enter(&mut session, 2, 0);
    pump(&mut session);
    enter(&mut session, 3, 1500);
    pump(&mut session);
    assert_eq!(pending(&mut session).id.run, 1);
}

#[test]
fn queue_backpressure_rolls_back_the_source_command_and_processing_makes_room() {
    let temp = Temp::new();
    let (_, session) = load(&temp, &temp.source());
    let mut seed = session.state().clone();
    seed.generation = 5000;
    seed.world.pending = (1..=MAX_PENDING_EVENTS as u64)
        .map(|generation| PendingEvent {
            id: EventId {
                generation,
                ordinal: 0,
            },
            signal: WorldSignal::Quest(QUEST),
            after: None,
        })
        .collect();
    let mut full = GameSession::new(
        ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        seed.clone(),
    )
    .unwrap();
    assert!(
        full.apply(Command::Quest {
            quest: QUEST,
            transition: quests::Transition::Start
        })
        .is_err()
    );
    assert_eq!(full.state(), &seed);
    world(&mut full, WorldCommand::ProcessNext);
    world(&mut full, WorldCommand::ProcessNext);
    activate(&mut full);
    assert_eq!(full.state().world.pending.len(), MAX_PENDING_EVENTS);
}

#[test]
fn cooldown_uses_saved_time_and_throttles_new_events_after_success() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/world/escort.ron");
    let mut t: TriggerDefinition = read(&path);
    t.condition = Condition::InsideArea { area: AREA };
    t.repeat = TriggerRepeat::Cooldown { millis: 5000 };
    t.steps.truncate(1);
    t.steps.push(SequenceStep::Apply(vec![Action::SetLocked {
        object: GATE,
        locked: false,
    }]));
    write(path, &t);
    let path = root.join("scenarios/guard-gate.ron");
    let mut scenario: Scenario = read(&path);
    scenario.steps.clear();
    write(path, &scenario);
    let (_, mut session) = load(&temp, &root);
    let m = prepare(&mut session);
    arrive(&mut session, &m);
    pump(&mut session);
    enter(&mut session, 2, 0);
    enter(&mut session, 3, 1500);
    pump(&mut session);
    assert!(session.next_movement(None).is_none());
    let saves = SaveDirectory::new(temp.0.join("saves"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let mut restored = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    restored
        .apply(Command::AdvanceTime { millis: 5000 })
        .unwrap();
    enter(&mut restored, 4, 0);
    enter(&mut restored, 5, 1500);
    pump(&mut restored);
    assert_eq!(pending(&mut restored).id.run, 2);
}

#[test]
fn acquisition_edges_require_new_items_even_when_possession_was_already_satisfied() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/world/escort.ron");
    let mut t: TriggerDefinition = read(&path);
    t.activation = TriggerActivation::Events(
        [WorldSignal::ItemAcquired {
            actor: HERO,
            definition: inventory::fixtures::KEY,
        }]
        .into(),
    );
    write(path, &t);
    let path = root.join("scenarios/guard-gate.ron");
    let mut scenario: Scenario = read(&path);
    scenario.steps.clear();
    write(path, &scenario);
    let (_, mut session) = load(&temp, &root);
    activate(&mut session);
    enter(&mut session, 1, 1500);
    pump(&mut session);
    assert!(session.next_movement(None).is_none());
    let item = session
        .state()
        .carried(HERO)
        .unwrap()
        .entries
        .iter()
        .find(|i| i.definition == inventory::fixtures::KEY)
        .unwrap()
        .id;
    session
        .apply(Command::Transfer {
            source: InventoryId(HERO.0),
            destination: InventoryId(MERCHANT.0),
            item,
            quantity: 1,
        })
        .unwrap();
    pump(&mut session);
    assert!(session.next_movement(None).is_none());
    session
        .apply(Command::Transfer {
            source: InventoryId(MERCHANT.0),
            destination: InventoryId(HERO.0),
            item,
            quantity: 1,
        })
        .unwrap();
    pump(&mut session);
    assert_eq!(pending(&mut session).id.run, 1);
}
