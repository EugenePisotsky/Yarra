use game_types::*;
use gameplay::inventory::fixtures::*;
use gameplay::{dialogue, rules};
use gameplay::{fixtures::*, *};
use rusqlite::Connection;
use yarra_save::*;
type Session = StoredSession<ToolContent>;
struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("yarra-checkpoint-{}", OwnerId::new()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.0.join(name)
    }
    fn restore(&self, saves: &SaveDirectory, slot: SaveSlot) -> Session {
        saves
            .load(
                slot,
                ToolContent::new(content()).unwrap(),
                self.path(&format!("working-{}.sqlite", OwnerId::new())),
            )
            .unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn session(temp: &Temp) -> Session {
    let content = content();
    GameSession::new(
        WorkingStore::create(temp.path("live.sqlite"), &content, &state()).unwrap(),
        ToolContent::new(content).unwrap(),
    )
    .unwrap()
}
fn snapshot(session: &Session) -> SessionState {
    session.store().export_for_tools(100_000).unwrap()
}
fn use_potion(session: &mut Session) {
    let item = session
        .query(StateRequest {
            actors: [HERO].into(),
            ..Default::default()
        })
        .unwrap()
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
fn start() -> Command {
    Command::StartDialogue {
        bindings: Default::default(),
        dialogue: GATE_DIALOGUE,
        participant: HERO,
        speaker: MERCHANT,
    }
}
fn choose() -> Command {
    Command::Choose {
        expected: dialogue::Token { run: 1, step: 1 },
        dialogue: GATE_DIALOGUE,
        participant: HERO,
        speaker: MERCHANT,
        choice: key("return-key"),
    }
}
#[test]
fn coherent_roundtrip_includes_unloaded_records_and_no_definitions() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(temp.path("slots"), 2).unwrap();
    let mut live = session(&temp);
    let sword = live
        .query(StateRequest {
            actors: [HERO].into(),
            ..Default::default()
        })
        .unwrap()
        .carried(HERO)
        .unwrap()
        .entries
        .iter()
        .find(|e| e.definition == SWORD)
        .unwrap()
        .id;
    live.apply(Command::Equip {
        actor: HERO,
        item: sword,
    })
    .unwrap();
    use_potion(&mut live);
    live.apply(start()).unwrap();
    live.apply(Command::AdvanceLine {
        key: gameplay::ConversationKey {
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        },
        expected: dialogue::Token { run: 1, step: 0 },
    })
    .unwrap();
    live.apply(choose()).unwrap();
    live.apply(Command::AdvanceTime { millis: 12345 }).unwrap();
    let expected = snapshot(&live);
    let reads = live.store().stats();
    saves
        .save(SaveSlot::Manual(1), &live, "At the gate")
        .unwrap();
    assert_eq!(
        live.store().stats(),
        reads,
        "checkpoint must not materialize state"
    );
    let mut loaded = temp.restore(&saves, SaveSlot::Manual(1));
    assert_eq!(
        loaded.store().stats().decoded_records,
        0,
        "load must not materialize state"
    );
    assert_eq!(snapshot(&loaded), expected);
    assert_eq!(loaded.identity(), live.identity());
    assert_eq!(
        loaded
            .query(StateRequest {
                actors: [MERCHANT].into(),
                ..Default::default()
            })
            .unwrap()
            .actor(MERCHANT)
            .unwrap()
            .position
            .millimetres,
        [100000000, 0, 100000000]
    );
    assert_eq!(loaded.derived(HERO).unwrap()[&key("strength")], 12);
    assert!(loaded.apply(choose()).is_err());
    let c = Connection::open(saves.path(SaveSlot::Manual(1))).unwrap();
    assert_eq!(c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('assets','item_definitions','categories','save_manifest')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
#[test]
fn checkpoint_failure_preserves_previous_slot_and_cleans_failed_creation() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(temp.path("slots"), 2).unwrap();
    let mut live = session(&temp);
    saves.quicksave(&live).unwrap();
    let previous = snapshot(&live);
    use_potion(&mut live);
    let accepted = snapshot(&live);
    let c = Connection::open(temp.path("live.sqlite")).unwrap();
    c.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE UPDATE OF info ON session_meta WHEN NEW.info IS NOT NULL BEGIN SELECT RAISE(ABORT,'injected after snapshot copy'); END;").unwrap();
    assert!(saves.quicksave(&live).is_err());
    assert!(
        saves
            .save(SaveSlot::Manual(8), &live, "Failed new checkpoint")
            .is_err()
    );
    assert!(!saves.path(SaveSlot::Manual(8)).exists());
    assert_eq!(snapshot(&temp.restore(&saves, SaveSlot::Quick)), previous);
    assert_eq!(snapshot(&live), accepted);
    assert!(
        std::fs::read_dir(temp.path("slots")).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with('.'))
    );
    c.execute_batch("DROP TRIGGER reject_checkpoint").unwrap();
    saves.quicksave(&live).unwrap();
    assert_eq!(snapshot(&temp.restore(&saves, SaveSlot::Quick)), accepted);
}
#[test]
fn command_write_failure_rolls_back_records_header_facts_and_rng() {
    let temp = Temp::new();
    let mut live = session(&temp);
    live.apply(start()).unwrap();
    live.apply(Command::AdvanceLine {
        key: gameplay::ConversationKey {
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        },
        expected: dialogue::Token { run: 1, step: 0 },
    })
    .unwrap();
    let before = snapshot(&live);
    let c = Connection::open(temp.path("live.sqlite")).unwrap();
    c.execute_batch("CREATE TRIGGER reject_commit BEFORE UPDATE OF header ON session_meta BEGIN SELECT RAISE(ABORT,'injected after all record writes'); END;").unwrap();
    assert!(live.apply(choose()).is_err());
    assert_eq!(snapshot(&live), before);
    c.execute_batch("DROP TRIGGER reject_commit").unwrap();
    assert!(live.apply(choose()).is_ok());
}
#[test]
fn explicit_slots_and_restore_discard_unsaved_work() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(temp.path("slots"), 2).unwrap();
    let mut live = session(&temp);
    saves.save(SaveSlot::Manual(0), &live, "Manual").unwrap();
    saves.quicksave(&live).unwrap();
    let manual = snapshot(&live);
    saves.autosave(&live).unwrap();
    use_potion(&mut live);
    saves.autosave(&live).unwrap();
    use_potion(&mut live);
    saves.autosave(&live).unwrap();
    assert_eq!(saves.list().unwrap().len(), 4);
    assert_eq!(snapshot(&temp.restore(&saves, SaveSlot::Manual(0))), manual);
    assert_eq!(snapshot(&temp.restore(&saves, SaveSlot::Quick)), manual);
    assert!(
        saves
            .save(SaveSlot::Auto(2), &live, "Outside retention")
            .is_err()
    );
    let reopened = WorkingStore::open(temp.path("live.sqlite")).unwrap();
    assert_eq!(reopened.header().unwrap(), live.header().unwrap());
    assert!(
        saves
            .load(
                SaveSlot::Quick,
                ToolContent::new(content()).unwrap(),
                temp.path("live.sqlite")
            )
            .is_err()
    );
    assert_ne!(live.header().unwrap().generation, manual.generation);
}
#[test]
fn content_schema_and_requested_corruption_are_explicit_errors() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(temp.path("slots"), 1).unwrap();
    let live = session(&temp);
    saves.quicksave(&live).unwrap();
    let mut other = content();
    other.items.items[0].weight_grams += 1;
    assert!(matches!(
        saves.load(
            SaveSlot::Quick,
            ToolContent::new(other).unwrap(),
            temp.path("other.sqlite")
        ),
        Err(SaveError::ContentMismatch)
    ));
    let c = Connection::open(saves.path(SaveSlot::Quick)).unwrap();
    c.pragma_update(None, "user_version", 2).unwrap();
    assert!(matches!(
        saves.load(
            SaveSlot::Quick,
            ToolContent::new(content()).unwrap(),
            temp.path("old.sqlite")
        ),
        Err(SaveError::Schema(2))
    ));
    c.pragma_update(None, "user_version", SCHEMA_VERSION)
        .unwrap();
    c.execute("UPDATE records SET payload=CAST(json_set(CAST(payload AS TEXT),'$.health',999999) AS BLOB) WHERE kind=1 AND id=?1",[HERO.to_string()]).unwrap();
    let mut loaded = temp.restore(&saves, SaveSlot::Quick);
    assert!(
        loaded
            .query(StateRequest {
                actors: [HERO].into(),
                ..Default::default()
            })
            .is_err()
    );
    assert_eq!(snapshot(&live).actor(HERO).unwrap().health, 50);
}
#[test]
fn wrong_database_is_never_replaced() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(temp.path("slots"), 1).unwrap();
    let live = session(&temp);
    let c = Connection::open(saves.path(SaveSlot::Quick)).unwrap();
    c.execute_batch("CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('keep');")
        .unwrap();
    assert!(matches!(
        saves.quicksave(&live),
        Err(SaveError::WrongDatabase)
    ));
    assert_eq!(
        c.query_row("SELECT value FROM sentinel", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep"
    );
}
#[test]
fn rng_effects_and_generated_item_identities_resume() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(temp.path("slots"), 1).unwrap();
    let mut seed = state();
    seed.actors[0].effects.push(rules::ActiveEffect {
        modifier: rules::Modifier {
            attribute: key("strength"),
            amount: 4,
        },
        expires_at: GameTime(500),
    });
    let mut live = GameSession::new(
        WorkingStore::in_memory(&content(), &seed).unwrap(),
        ToolContent::new(content()).unwrap(),
    )
    .unwrap();
    live.apply(start()).unwrap();
    live.apply(Command::AdvanceLine {
        key: gameplay::ConversationKey {
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        },
        expected: dialogue::Token { run: 1, step: 0 },
    })
    .unwrap();
    saves.quicksave(&live).unwrap();
    let mut loaded = temp.restore(&saves, SaveSlot::Quick);
    assert_eq!(
        live.apply(choose()).unwrap(),
        loaded.apply(choose()).unwrap()
    );
    assert_eq!(snapshot(&live), snapshot(&loaded));
    loaded.apply(Command::AdvanceTime { millis: 500 }).unwrap();
    assert_eq!(loaded.derived(HERO).unwrap()[&key("strength")], 10);
}
#[test]
fn autosave_rotation_uses_sequence_when_wall_clock_moves_backwards() {
    let temp = Temp::new();
    let saves = SaveDirectory::new(temp.path("slots"), 2).unwrap();
    let live = session(&temp);
    assert_eq!(saves.autosave(&live).unwrap().slot, SaveSlot::Auto(0));
    assert_eq!(saves.autosave(&live).unwrap().slot, SaveSlot::Auto(1));
    Connection::open(saves.path(SaveSlot::Auto(1)))
        .unwrap()
        .execute_batch("UPDATE session_meta SET info=json_set(info,'$.saved_at_ms',0)")
        .unwrap();
    let saves = SaveDirectory::new(temp.path("slots"), 2).unwrap();
    let next = saves.autosave(&live).unwrap();
    assert_eq!(next.slot, SaveSlot::Auto(0));
    assert_eq!(next.sequence, 3);
    assert_eq!(saves.autosave(&live).unwrap().slot, SaveSlot::Auto(1));
}

#[test]
fn schema_indexes_expiration_and_stale_records_are_checked() {
    let temp = Temp::new();
    let mut store = WorkingStore::create(temp.path("work.sqlite"), &content(), &state()).unwrap();
    let before = store.export_for_tools(100_000).unwrap();
    {
        let mut tx = store.begin().unwrap();
        let batch = tx
            .load(&StateRequest {
                actors: [HERO].into(),
                ..Default::default()
            })
            .unwrap();
        let mut next = batch.state.clone();
        next.actors[0].health = 49;
        tx.stage(&batch, &next).unwrap();
        assert!(tx.stage(&batch, &next).is_err());
    }
    assert_eq!(store.export_for_tools(100_000).unwrap(), before);
    let c = Connection::open(temp.path("work.sqlite")).unwrap();
    let plan:String=c.query_row("EXPLAIN QUERY PLAN SELECT id FROM records INDEXED BY expiration_queue WHERE kind=1 AND next_expiry IS NOT NULL AND next_expiry<=?1 ORDER BY next_expiry,id LIMIT 1",[1000],|r|r.get(3)).unwrap();
    assert!(plan.contains("expiration_queue"), "{plan}");
    assert!(!plan.contains("TEMP"), "{plan}");
    c.execute(
        "UPDATE records SET next_expiry=1 WHERE kind=1 AND id=?1",
        [HERO.to_string()],
    )
    .unwrap();
    let mut live = GameSession::new(store, ToolContent::new(content()).unwrap()).unwrap();
    assert!(live.apply(Command::AdvanceTime { millis: 10 }).is_err());
    assert_eq!(live.header().unwrap().time, before.time);
}
#[test]
fn large_clock_steps_preserve_intermediate_health_caps_for_nonresident_actors() {
    let mut seed = state();
    let hero = seed.actors.iter_mut().find(|a| a.id == HERO).unwrap();
    hero.health = 100;
    hero.effects = vec![
        rules::ActiveEffect {
            modifier: rules::Modifier {
                attribute: key("max-health"),
                amount: 50,
            },
            expires_at: GameTime(100),
        },
        rules::ActiveEffect {
            modifier: rules::Modifier {
                attribute: key("max-health"),
                amount: -50,
            },
            expires_at: GameTime(200),
        },
    ];
    let mut live = GameSession::new(
        WorkingStore::in_memory(&content(), &seed).unwrap(),
        ToolContent::new(content()).unwrap(),
    )
    .unwrap();
    live.apply(Command::AdvanceTime { millis: 200 }).unwrap();
    let model = live
        .query(StateRequest {
            actors: [HERO].into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(model.actor(HERO).unwrap().health, 50);
    assert!(model.actor(HERO).unwrap().effects.is_empty());
    let reads = live.store().stats().decoded_records;
    live.apply(Command::AdvanceTime { millis: 100 }).unwrap();
    assert_eq!(
        live.store().stats().decoded_records,
        reads,
        "time with no deadlines reads no actors"
    );
}
#[test]
fn oversized_queries_fail_and_named_slots_cannot_be_opened_as_working_state() {
    let temp = Temp::new();
    let mut live = session(&temp);
    let before = live.header().unwrap();
    let request = StateRequest {
        actors: (0u128..65).map(|n| ActorId(n.to_le_bytes())).collect(),
        ..Default::default()
    };
    assert!(live.query(request).is_err());
    assert_eq!(live.header().unwrap(), before);
    let saves = SaveDirectory::new(temp.path("slots"), 1).unwrap();
    saves.quicksave(&live).unwrap();
    assert!(WorkingStore::open(saves.path(SaveSlot::Quick)).is_err());
}
#[test]
fn wal_working_database_is_checkpointed_via_sqlite_snapshot() {
    let temp = Temp::new();
    let mut live = session(&temp);
    let c = Connection::open(temp.path("live.sqlite")).unwrap();
    c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    use_potion(&mut live);
    let expected = snapshot(&live);
    let saves = SaveDirectory::new(temp.path("slots"), 1).unwrap();
    saves.quicksave(&live).unwrap();
    assert_eq!(snapshot(&temp.restore(&saves, SaveSlot::Quick)), expected);
}

#[test]
fn oversized_payload_is_rejected_before_decode() {
    let temp = Temp::new();
    let mut live = session(&temp);
    let before = live.header().unwrap();
    let c = Connection::open(temp.path("live.sqlite")).unwrap();
    c.execute_batch("PRAGMA ignore_check_constraints=ON;")
        .unwrap();
    c.execute(
        "UPDATE records SET payload=zeroblob(1048577) WHERE kind=1 AND id=?1",
        [HERO.to_string()],
    )
    .unwrap();
    assert!(
        live.query(StateRequest {
            actors: [HERO].into(),
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(live.store().stats().decoded_records, 0);
    assert_eq!(live.header().unwrap(), before);
}
#[test]
fn duplicate_global_item_identity_rejects_seed_and_cleans_file() {
    let temp = Temp::new();
    let mut seed = state();
    let id = seed.inventory(HERO_BAG).unwrap().entries[0].id;
    seed.inventories
        .iter_mut()
        .find(|i| i.id == MERCHANT_BAG)
        .unwrap()
        .entries[0]
        .id = id;
    let path = temp.path("invalid.sqlite");
    assert!(WorkingStore::create(&path, &content(), &seed).is_err());
    assert!(!path.exists());
}
#[test]
fn failure_in_later_expiration_rolls_back_earlier_actors_and_clock() {
    let temp = Temp::new();
    let mut seed = state();
    for a in &mut seed.actors {
        a.effects.push(rules::ActiveEffect {
            modifier: rules::Modifier {
                attribute: key("strength"),
                amount: 1,
            },
            expires_at: GameTime(100),
        });
    }
    let store = WorkingStore::create(temp.path("live.sqlite"), &content(), &seed).unwrap();
    let mut live = GameSession::new(store, ToolContent::new(content()).unwrap()).unwrap();
    let before = snapshot(&live);
    let c = Connection::open(temp.path("live.sqlite")).unwrap();
    c.execute_batch("CREATE TRIGGER reject_later BEFORE UPDATE ON records WHEN NEW.kind=1 AND NEW.id='03030303-0303-0303-0303-030303030303' BEGIN SELECT RAISE(ABORT,'later actor'); END;").unwrap();
    assert!(live.apply(Command::AdvanceTime { millis: 100 }).is_err());
    assert_eq!(snapshot(&live), before);
    c.execute_batch("DROP TRIGGER reject_later").unwrap();
    live.apply(Command::AdvanceTime { millis: 100 }).unwrap();
    assert!(snapshot(&live).actors.iter().all(|a| a.effects.is_empty()));
}
