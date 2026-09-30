//! Character rules: stats, effects, levels, points, trainers, the party and its purse.
use game_types::*;
use yarra_gameplay::inventory::fixtures::*;
use yarra_gameplay::{fixtures::*, *};

type Result<T> = yarra_gameplay::Result<T>;
type TestSession = GameSession<ToolContent>;
const TALK: ConversationKey = ConversationKey {
    dialogue: GATE_DIALOGUE,
    participant: HERO,
    speaker: MERCHANT,
};
fn new(content: GameContent) -> TestSession {
    GameSession::new(ToolContent::new(content).unwrap(), state()).unwrap()
}
fn session() -> TestSession {
    new(content())
}
/// The fixture conversation with its one choice doing `actions`, always on offer.
fn offering(actions: Vec<Action>) -> GameContent {
    let mut content = content();
    let graph = &mut content.game.dialogues[0];
    graph.repeat = dialogue::RepeatPolicy::Always;
    let choice = &mut graph.nodes[1];
    choice.condition = None;
    choice.repeat = dialogue::Repeat::Always;
    choice.actions = actions;
    content.game.dialogue_contracts = vec![content.game.dialogues[0].contract()];
    content
}
/// Talks to the merchant and picks the choice, running its actions for the hero.
fn pick(session: &mut TestSession) -> Result<CommandOutcome> {
    // A choice that was refused is still on offer; otherwise the conversation starts over.
    let waiting = session
        .state()
        .conversations
        .get(&TALK)
        .is_some_and(|c| c.status == dialogue::RunStatus::Active);
    if !waiting {
        session.apply(Command::StartDialogue {
            bindings: Default::default(),
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        })?;
        let expected = session.conversation_view(TALK)?.token;
        session.apply(Command::AdvanceLine {
            key: TALK,
            expected,
        })?;
    }
    let expected = session.conversation_view(TALK)?.token;
    session.apply(Command::Choose {
        expected,
        dialogue: GATE_DIALOGUE,
        participant: HERO,
        speaker: MERCHANT,
        choice: key("return-key"),
    })
}
/// Runs actions for the hero through a conversation choice.
fn run(session: &mut TestSession, actions: Vec<Action>) -> Result<CommandOutcome> {
    let state = session.state().clone();
    *session = GameSession::new(ToolContent::new(offering(actions)).unwrap(), state).unwrap();
    pick(session)
}
fn stat(session: &TestSession, actor: ActorId, name: &str) -> i32 {
    session.stat(actor, &key(name)).unwrap()
}
fn rejected(result: Result<CommandOutcome>) -> Rejection {
    match result {
        Err(GameplayError::Rejected(rejection)) => rejection,
        other => panic!("expected a rejection, got {other:?}"),
    }
}
fn effect(name: &str, duration_ms: Option<u64>) -> Action {
    Action::ApplyEffect {
        of: Participant::Player,
        effect: key(name),
        duration_ms,
    }
}
fn award(amount: u64) -> Action {
    Action::AwardExperience { amount }
}
fn join(session: &mut TestSession, actor: ActorId) -> Result<CommandOutcome> {
    session.apply(Command::Party {
        actor,
        member: true,
    })
}

#[test]
fn stats_come_from_base_values_equipment_effects_and_the_rules_formula() {
    let mut session = session();
    // Level 1 with vitality 10: 40 + 10 * 5 + 1 * 10.
    assert_eq!(stat(&session, HERO, "max-health"), 100);
    assert_eq!(stat(&session, HERO, "health"), 50);
    assert_eq!(stat(&session, HERO, "attack"), 10);
    let sword = session
        .state()
        .carried(HERO)
        .unwrap()
        .entries
        .iter()
        .find(|e| e.definition == SWORD)
        .unwrap()
        .id;
    session
        .apply(Command::Equip {
            actor: HERO,
            item: sword,
        })
        .unwrap();
    // The sword adds to a primary stat, and the derived one follows it.
    assert_eq!(stat(&session, HERO, "strength"), 12);
    assert_eq!(stat(&session, HERO, "attack"), 12);
    run(&mut session, vec![effect("fortified", None)]).unwrap();
    assert_eq!(stat(&session, HERO, "attack"), 15);
    // Each change to a stat can be traced to where it comes from.
    let state = session.state();
    assert_eq!(
        state
            .modifiers_of(session.content(), HERO, &key("strength"))
            .unwrap(),
        [
            (ModifierSource::Item(SWORD), rules::Operation::Add(2)),
            (
                ModifierSource::Effect(key("fortified")),
                rules::Operation::Add(3)
            ),
        ]
    );
    session
        .apply(Command::Unequip {
            actor: HERO,
            slot: key("hand"),
        })
        .unwrap();
    assert_eq!(stat(&session, HERO, "attack"), 13);
    // What the character is built of did not change.
    assert_eq!(
        session.state().actor(HERO).unwrap().base[&key("strength")],
        10
    );
    assert!(session.stat(HERO, &key("charm")).is_err());
}

#[test]
fn effects_tick_and_end_in_order_of_time_and_only_their_bearers_are_looked_at() {
    let mut session = session();
    assert!(session.state().timed.is_empty());
    run(
        &mut session,
        vec![
            effect("poisoned", Some(3500)),
            effect("fortified", Some(2000)),
        ],
    )
    .unwrap();
    assert_eq!(session.state().timed, [HERO].into());
    assert_eq!(stat(&session, HERO, "strength"), 13);
    session
        .apply(Command::AdvanceTime { millis: 1000 })
        .unwrap();
    assert_eq!(stat(&session, HERO, "health"), 45);
    // One step across two ticks and both endings.
    let outcome = session
        .apply(Command::AdvanceTime { millis: 2500 })
        .unwrap();
    assert_eq!(stat(&session, HERO, "health"), 35);
    assert_eq!(stat(&session, HERO, "strength"), 10);
    let ended: Vec<&str> = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            GameEvent::EffectEnded { effect, .. } => Some(effect.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ended, ["fortified", "poisoned"]);
    assert!(session.state().actor(HERO).unwrap().effects.is_empty());
    assert!(session.state().timed.is_empty());
    // Applying an effect again starts it over instead of stacking it.
    run(&mut session, vec![effect("fortified", Some(1000))]).unwrap();
    session.apply(Command::AdvanceTime { millis: 900 }).unwrap();
    run(&mut session, vec![effect("fortified", Some(1000))]).unwrap();
    session.apply(Command::AdvanceTime { millis: 900 }).unwrap();
    assert_eq!(stat(&session, HERO, "strength"), 13);
    assert_eq!(session.state().actor(HERO).unwrap().effects.len(), 1);
}

#[test]
fn a_step_of_time_carries_out_everything_due_in_it_however_much() {
    // A salve that heals every millisecond: twenty seconds are 20,000 ticks in one step.
    let mut healing = offering(vec![effect("poisoned", None)]);
    let effects = &mut healing.game.rules.effects;
    let salve = effects
        .iter_mut()
        .find(|e| e.id == key("poisoned"))
        .unwrap();
    salve.periodic = Some(rules::Periodic {
        resource: key("health"),
        amount: 1,
        period_ms: 1,
    });
    let mut session = new(healing);
    pick(&mut session).unwrap();
    let time = session.state().time;
    session
        .apply(Command::AdvanceTime { millis: 20_000 })
        .unwrap();
    assert_eq!(session.state().time, GameTime(time.0 + 20_000));
    assert_eq!(stat(&session, HERO, "health"), 100);
    // Only something that takes no time at all could keep a step from ending, and the
    // rules refuse it.
    let mut content = content();
    content.game.rules.abilities[0].duration_ms = 0;
    let error = ToolContent::new(content).err().unwrap().to_string();
    assert!(error.contains("takes 1 ms"), "{error}");
}
#[test]
fn losing_the_last_of_the_life_resource_is_death() {
    let mut content = content();
    // A trigger hears of the death like any other event.
    content.game.world.triggers.push(TriggerDefinition {
        id: TriggerId::named("old_gate/mourn"),
        player: HERO,
        speaker: None,
        on: [WorldSignal::Died(HERO)].into(),
        condition: None,
        actions: vec![Action::Set {
            variable: REWARDED,
            of: None,
            value: Value::Bool(true),
        }],
        repeat: Default::default(),
    });
    let mut session = new(content);
    let drain = Action::ChangeResource {
        of: Participant::Player,
        resource: key("health"),
        amount: -48,
    };
    let state = session.state().clone();
    let mut tuned = offering(vec![drain, effect("poisoned", None)]);
    tuned.game.world = session.content().game.world.clone();
    session = GameSession::new(ToolContent::new(tuned).unwrap(), state).unwrap();
    pick(&mut session).unwrap();
    assert_eq!(stat(&session, HERO, "health"), 2);
    let outcome = session
        .apply(Command::AdvanceTime { millis: 1000 })
        .unwrap();
    assert!(outcome.events.contains(&GameEvent::Died { actor: HERO }));
    assert_eq!(stat(&session, HERO, "health"), 0);
    // Nothing lingers on the dead, and the dead do nothing.
    assert!(session.state().actor(HERO).unwrap().effects.is_empty());
    assert!(session.state().timed.is_empty());
    let potion = session.state().carried(HERO).unwrap().entries[0].id;
    let refused = session.apply(Command::UseItem {
        actor: HERO,
        item: potion,
    });
    assert_eq!(rejected(refused), Rejection::Dead(HERO));
    // The trigger listening for the death ran as part of the command that caused it.
    assert!(!session.world_work_pending());
    assert_eq!(
        session
            .state()
            .variable(session.content(), REWARDED)
            .unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn experience_is_shared_and_levels_grant_points_and_bonuses() {
    let mut session = session();
    join(&mut session, COMPANION).unwrap();
    let outcome = run(&mut session, vec![award(350)]).unwrap();
    assert_eq!(session.state().party.experience, 350);
    let levels: std::collections::BTreeSet<(ActorId, u32)> = outcome
        .events
        .iter()
        .filter_map(|e| match e {
            GameEvent::LeveledUp { actor, level } => Some((*actor, *level)),
            _ => None,
        })
        .collect();
    assert_eq!(
        levels,
        [(HERO, 2), (HERO, 3), (COMPANION, 2), (COMPANION, 3)].into()
    );
    let hero = session.state().actor(HERO).unwrap().clone();
    assert_eq!((hero.attribute_points, hero.learning_points), (4, 6));
    // Level 3 adds one vitality outright: 40 + 11 * 5 + 3 * 10. Health is not refilled.
    assert_eq!(hero.base[&key("vitality")], 11);
    assert_eq!(stat(&session, HERO, "max-health"), 125);
    assert_eq!(stat(&session, HERO, "health"), 50);
    // The merchant is not travelling with them; on joining he catches up.
    assert_eq!(session.state().actor(MERCHANT).unwrap().level, 1);
    join(&mut session, MERCHANT).unwrap();
    assert_eq!(session.state().actor(MERCHANT).unwrap().level, 3);
    assert_eq!(stat(&session, MERCHANT, "health"), 100);

    let spend = |session: &mut TestSession, actor, name: &str| {
        session.apply(Command::SpendAttributePoint {
            actor,
            stat: key(name),
        })
    };
    spend(&mut session, HERO, "strength").unwrap();
    assert_eq!(stat(&session, HERO, "attack"), 11);
    assert_eq!(session.state().actor(HERO).unwrap().attribute_points, 3);
    assert!(spend(&mut session, HERO, "max-health").is_err());
    session
        .apply(Command::Party {
            actor: MERCHANT,
            member: false,
        })
        .unwrap();
    assert_eq!(
        rejected(spend(&mut session, MERCHANT, "strength")),
        Rejection::NotInParty(MERCHANT)
    );
    for _ in 0..3 {
        spend(&mut session, HERO, "wisdom").unwrap();
    }
    let before = session.state().clone();
    assert_eq!(
        rejected(spend(&mut session, HERO, "wisdom")),
        Rejection::NoAttributePoints
    );
    assert_eq!(session.state(), &before);
    // There is a highest level.
    run(&mut session, vec![award(1_000_000)]).unwrap();
    assert_eq!(session.state().actor(HERO).unwrap().level, 4);
}

#[test]
fn a_trainer_teaches_what_the_class_allows_for_learning_points() {
    let teach = |skill: &str| Action::Teach { skill: key(skill) };
    let can_learn = |session: &TestSession, skill: &str| {
        session
            .content()
            .evaluate(
                &Condition::CanLearn { skill: key(skill) },
                session.state(),
                HERO,
                MERCHANT,
            )
            .unwrap()
            .matched
    };
    let mut session = session();
    assert!(!can_learn(&session, "persuasion"));
    let before = session.state().clone();
    assert_eq!(
        rejected(run(&mut session, vec![teach("persuasion")])),
        Rejection::NotEnoughLearningPoints {
            needed: 1,
            available: 0
        }
    );
    assert_eq!(session.state().actors, before.actors);

    run(&mut session, vec![award(100)]).unwrap();
    assert!(can_learn(&session, "persuasion"));
    let outcome = run(&mut session, vec![teach("persuasion"), teach("persuasion")]).unwrap();
    assert!(outcome.events.contains(&GameEvent::SkillLearned {
        actor: HERO,
        skill: key("persuasion"),
        rank: 2
    }));
    let hero = session.state().actor(HERO).unwrap();
    assert_eq!(
        (hero.skill(&key("persuasion")), hero.learning_points),
        (2, 0)
    );
    // A skill rank can feed a derived stat: the formula adds two attack per rank.
    run(&mut session, vec![award(200), teach("swordsmanship")]).unwrap();
    assert_eq!(stat(&session, HERO, "attack"), 12);
    run(&mut session, vec![teach("swordsmanship")]).unwrap();
    // The adventurer's trainers stop at rank two of three.
    run(&mut session, vec![award(300)]).unwrap();
    assert_eq!(
        rejected(run(&mut session, vec![teach("swordsmanship")])),
        Rejection::ClassCannotLearn {
            class: key("adventurer"),
            skill: key("swordsmanship")
        }
    );
    assert!(!can_learn(&session, "swordsmanship"));

    // A scholar cannot learn it at all, whatever points there are.
    let mut content = offering(vec![award(100), teach("swordsmanship")]);
    content.game.actors[0].class = key("scholar");
    let mut state = SessionState::empty(1);
    for id in [HERO, MERCHANT] {
        state
            .spawn(&content, content.game.actors[0].id, id)
            .unwrap();
    }
    state.party.members.insert(HERO);
    let mut scholar = GameSession::new(ToolContent::new(content).unwrap(), state).unwrap();
    assert!(matches!(
        rejected(pick(&mut scholar)),
        Rejection::ClassCannotLearn { .. }
    ));
    assert_eq!(scholar.state().party.experience, 0);
}

#[test]
fn gold_is_paid_from_the_party_purse() {
    let mut session = session();
    let gold = |session: &TestSession, minimum| {
        session
            .content()
            .evaluate(
                &Condition::Gold { minimum },
                session.state(),
                HERO,
                MERCHANT,
            )
            .unwrap()
            .matched
    };
    assert!(gold(&session, 100) && !gold(&session, 101));
    run(&mut session, vec![Action::Pay { amount: 30 }]).unwrap();
    assert_eq!(session.state().gold().unwrap(), 70);
    let before = session.state().wallets.clone();
    assert_eq!(
        rejected(run(
            &mut session,
            vec![award(5), Action::Pay { amount: 100 }]
        )),
        Rejection::NotEnoughGold {
            needed: 100,
            available: 70
        }
    );
    // The experience given before the payment failed is taken back with it.
    assert_eq!(session.state().wallets, before);
    assert_eq!(session.state().party.experience, 0);
}

#[test]
fn the_party_holds_four_and_one_of_them_is_steered() {
    let content = content();
    let mut state = state();
    let extras: Vec<ActorId> = (1..=3u8).map(|n| ActorId([n; 16])).collect();
    for id in &extras {
        state
            .spawn(&content, content.game.actors[0].id, *id)
            .unwrap();
    }
    let mut session = GameSession::new(ToolContent::new(content).unwrap(), state).unwrap();
    for actor in [COMPANION, extras[0], extras[1]] {
        join(&mut session, actor).unwrap();
    }
    // Joining twice is not joining again.
    join(&mut session, COMPANION).unwrap();
    assert_eq!(session.state().party.members.len(), 4);
    assert_eq!(
        rejected(join(&mut session, extras[2])),
        Rejection::PartyFull
    );

    assert_eq!(session.state().party.controlled, Some(HERO));
    session
        .apply(Command::Control { actor: COMPANION })
        .unwrap();
    assert_eq!(session.state().party.controlled, Some(COMPANION));
    assert_eq!(
        rejected(session.apply(Command::Control { actor: MERCHANT })),
        Rejection::NotInParty(MERCHANT)
    );
    // The steered character cannot be sent away; anyone else can.
    let leave = |session: &mut TestSession, actor| {
        session.apply(Command::Party {
            actor,
            member: false,
        })
    };
    assert!(leave(&mut session, COMPANION).is_err());
    leave(&mut session, HERO).unwrap();
    join(&mut session, extras[2]).unwrap();
}

#[test]
fn a_check_is_decided_by_the_rules_formula_and_reports_what_it_rolled() {
    let check = |difficulty| Action::Check {
        skill: key("persuasion"),
        difficulty,
        success: vec![award(1)],
        failure: vec![],
    };
    let mut session = session();
    let random = session.state().random;
    let outcome = run(&mut session, vec![check(1)]).unwrap();
    let Some(GameEvent::Checked {
        actor,
        skill,
        passed,
        rolls,
    }) = outcome
        .events
        .iter()
        .find(|e| matches!(e, GameEvent::Checked { .. }))
    else {
        panic!("no check was reported")
    };
    assert_eq!(
        (*actor, skill.as_str(), *passed),
        (HERO, "persuasion", true)
    );
    assert!(rolls.len() == 1 && (1..=20).contains(&rolls[0]));
    assert_ne!(session.state().random, random);
    assert_eq!(session.state().party.experience, 1);
    // Out of reach for an untrained character with average wisdom: 20 at most.
    run(&mut session, vec![check(21)]).unwrap();
    assert_eq!(session.state().party.experience, 1);
    // Two ranks add four.
    run(&mut session, vec![award(99)]).unwrap();
    let persuasion = Action::Teach {
        skill: key("persuasion"),
    };
    run(&mut session, vec![persuasion.clone(), persuasion]).unwrap();
    let passes = (0..40)
        .filter(|_| {
            let before = session.state().party.experience;
            run(&mut session, vec![check(21)]).unwrap();
            session.state().party.experience > before
        })
        .count();
    assert!((1..40).contains(&passes), "{passes} of 40 passed");
}

#[test]
fn a_saved_character_is_brought_in_line_with_the_rules_or_refused_on_load() {
    let content = content();
    let good = state();
    let open = |change: fn(&mut actors::Actor)| {
        let mut state = good.clone();
        change(state.actors.get_mut(&HERO).unwrap());
        GameSession::new(ToolContent::new(content.clone()).unwrap(), state)
    };
    // What follows from the rules is worked out again...
    let hero = &good.actors[&HERO];
    let restored = open(|a| {
        a.stats.insert(key("attack"), 99);
    })
    .unwrap();
    assert_eq!(restored.state().actors[&HERO].stats, hero.stats);
    let restored = open(|a| {
        a.resources.insert(key("health"), 101);
    })
    .unwrap();
    assert_eq!(
        restored.state().actors[&HERO].resources[&key("health")],
        100
    );
    let mut state = good.clone();
    state.party.experience = 100;
    let restored = GameSession::new(ToolContent::new(content.clone()).unwrap(), state).unwrap();
    assert_eq!(restored.state().actors[&HERO].level, 2);
    // ...and what the rules cannot account for is refused.
    assert!(open(|a| a.level = 9).is_err());
    assert!(open(|a| a.class = key("paladin")).is_err());
    assert!(
        open(|a| {
            a.skills.insert(key("persuasion"), 4);
        })
        .is_err()
    );
    assert!(
        open(|a| {
            a.base.insert(key("strength"), 31);
        })
        .is_err()
    );
    assert!(
        open(|a| a.effects.push(rules::ActiveEffect {
            effect: key("fortified"),
            expires_at: Some(GameTime(5)),
            next_tick: None,
        }))
        .is_err()
    );
    let saved = serde_json::to_string(&good).unwrap();
    assert_eq!(serde_json::from_str::<SessionState>(&saved).unwrap(), good);
}

#[test]
fn what_a_player_runs_into_is_refused_with_a_reason_a_ui_can_match() {
    let mut session = session();
    let item = |session: &TestSession, definition| {
        let bag = session.state().carried(HERO).unwrap();
        bag.entries
            .iter()
            .find(|e| e.definition == definition)
            .unwrap()
            .id
    };
    let (potion, sword) = (item(&session, POTION), item(&session, SWORD));
    let before = session.state().clone();
    let mut refused = |command| rejected(session.apply(command));
    let hand = key("hand");
    assert_eq!(
        refused(Command::Unequip {
            actor: HERO,
            slot: hand.clone()
        }),
        Rejection::SlotEmpty(hand)
    );
    assert_eq!(
        refused(Command::Equip {
            actor: HERO,
            item: potion
        }),
        Rejection::NotEquippable
    );
    assert_eq!(
        refused(Command::UseItem {
            actor: HERO,
            item: sword
        }),
        Rejection::NotUsable
    );
    assert_eq!(
        refused(Command::Talk {
            participant: HERO,
            speaker: MERCHANT,
            topic: None,
            bindings: Default::default()
        }),
        Rejection::NothingToSay(MERCHANT)
    );
    assert_eq!(
        refused(Command::Party {
            actor: HERO,
            member: false
        }),
        Rejection::Controlled(HERO)
    );
    assert_eq!(session.state(), &before);

    // A conversation refuses what no longer fits where it is.
    let start = Command::StartDialogue {
        bindings: Default::default(),
        dialogue: GATE_DIALOGUE,
        participant: HERO,
        speaker: MERCHANT,
    };
    session.apply(start.clone()).unwrap();
    assert_eq!(rejected(session.apply(start)), Rejection::AlreadyTalking);
    let token = session.conversation_view(TALK).unwrap().token;
    let advance = Command::AdvanceLine {
        key: TALK,
        expected: token,
    };
    session.apply(advance.clone()).unwrap();
    assert_eq!(
        rejected(session.apply(advance)),
        Rejection::ConversationMoved
    );
    let expected = session.conversation_view(TALK).unwrap().token;
    let choose = Command::Choose {
        expected,
        dialogue: GATE_DIALOGUE,
        participant: HERO,
        speaker: MERCHANT,
        choice: key("bribe"),
    };
    assert_eq!(
        rejected(session.apply(choose)),
        Rejection::ChoiceUnavailable(key("bribe"))
    );
}

mod unopened {
    use super::*;
    use yarra_gameplay::inventory::{LootEntry, LootTable};

    const STOCK: LootId = LootId::named("merchant/stock");
    const GUARD: ActorTemplateId = ActorTemplateId::named("armed_guard");
    const TRUNK: ObjectId = ObjectId::named("old_gate/trunk");
    const TRUNK_BAG: InventoryId = InventoryId::named("old_gate/trunk");
    fn armed() -> GameContent {
        let mut content = content();
        content.game.loot.push(LootTable {
            id: STOCK,
            entries: vec![
                LootEntry {
                    item: POTION,
                    quantity: (2, 12),
                    chance: 100,
                },
                LootEntry {
                    item: KEY,
                    quantity: (1, 1),
                    chance: 50,
                },
            ],
        });
        let mut guard = content.game.actors[0].clone();
        guard.id = GUARD;
        guard.equipment = vec![SWORD];
        guard.loot = Some(STOCK);
        content.game.actors.push(guard);
        content.game.world.objects.push(ObjectDefinition {
            id: TRUNK,
            name: "Trunk".into(),
            kind: ObjectKind::Container {
                inventory: TRUNK_BAG,
                loot: Some(STOCK),
            },
            locked: false,
        });
        content.sort();
        content
    }
    fn start(count: u8) -> (SessionState, Vec<ActorId>) {
        let content = armed();
        let mut state = state();
        let ids: Vec<ActorId> = (1..=count).map(|n| ActorId([n; 16])).collect();
        for id in &ids {
            state.spawn(&content, GUARD, *id).unwrap();
        }
        (state, ids)
    }
    fn open(state: SessionState) -> TestSession {
        GameSession::new(ToolContent::new(armed()).unwrap(), state).unwrap()
    }
    fn guards(count: u8) -> (TestSession, Vec<ActorId>) {
        let (state, ids) = start(count);
        (open(state), ids)
    }
    fn potions(session: &TestSession, actor: ActorId) -> u64 {
        let state = session.state();
        state.item_count(session.content(), actor, POTION).unwrap()
    }

    #[test]
    fn an_inventory_is_made_when_it_is_first_needed_and_is_what_was_promised() {
        let (mut session, ids) = guards(3);
        let guard = ids[0];
        let before = session.state().inventories.len();
        // No inventory yet, but the sword the template wields already counts, and what the
        // inventory will hold can be asked.
        assert!(session.state().carried(guard).is_err());
        assert_eq!(stat(&session, guard, "strength"), 12);
        let promised = potions(&session, guard);
        assert!((2..=12).contains(&promised));

        session
            .apply(Command::OpenInventory { actor: guard })
            .unwrap();
        assert_eq!(session.state().inventories.len(), before + 1);
        assert_eq!(potions(&session, guard), promised);
        let state = session.state();
        let bag = state.carried(guard).unwrap();
        let worn = state.actor(guard).unwrap().equipment[&key("hand")];
        assert_eq!(bag.entry(worn).unwrap().definition, SWORD);
        assert_eq!(stat(&session, guard, "strength"), 12);
        // Opening again changes nothing.
        let opened = session.state().clone();
        session
            .apply(Command::OpenInventory { actor: guard })
            .unwrap();
        assert_eq!(session.state().inventories, opened.inventories);

        // Taking the sword off works like any equipment.
        session
            .apply(Command::Unequip {
                actor: guard,
                slot: key("hand"),
            })
            .unwrap();
        assert_eq!(stat(&session, guard, "strength"), 10);
    }

    #[test]
    fn what_is_found_does_not_depend_on_the_order_of_opening() {
        let (state, ids) = start(3);
        let found = |order: [usize; 3], trunk_first: bool| {
            let mut session = open(state.clone());
            let trunk = Command::World(WorldCommand::Open { object: TRUNK });
            if trunk_first {
                session.apply(trunk.clone()).unwrap();
            }
            for index in order {
                let actor = ids[index];
                session.apply(Command::OpenInventory { actor }).unwrap();
            }
            if !trunk_first {
                session.apply(trunk).unwrap();
            }
            let state = session.state();
            let list = |bag: &inventory::Inventory| -> Vec<(ItemDefinitionId, u32)> {
                let entries = bag.entries.iter();
                entries.map(|e| (e.definition, e.quantity)).collect()
            };
            let mut found: Vec<_> = ids
                .iter()
                .map(|id| list(state.carried(*id).unwrap()))
                .collect();
            found.push(list(state.inventory(TRUNK_BAG).unwrap()));
            // Loot has its own stream: the one checks and abilities roll from is untouched.
            assert_eq!(state.random, self::state().random);
            found
        };
        let forwards = found([0, 1, 2], false);
        assert_eq!(found([2, 0, 1], true), forwards);
        // Four owners with the same table do not all find the same.
        let potions: std::collections::BTreeSet<u32> = forwards
            .iter()
            .flat_map(|list| list.iter().filter(|(item, _)| *item == POTION))
            .map(|(_, quantity)| *quantity)
            .collect();
        assert!(potions.len() > 1, "{forwards:?}");
    }

    #[test]
    fn trading_needs_the_stock_opened_and_a_failed_command_leaves_it_unopened() {
        let (mut session, ids) = guards(1);
        let guard = ids[0];
        let promised = potions(&session, guard);
        session
            .apply(Command::OpenInventory { actor: guard })
            .unwrap();
        let stock = session.state().carried(guard).unwrap();
        let participants = inventory::TradeParticipants {
            merchant_inventory: stock.id,
            customer_inventory: HERO_BAG,
            merchant_wallet: MERCHANT_WALLET,
            customer_wallet: PARTY_WALLET,
        };
        let potion = stock.entries.iter().find(|e| e.definition == POTION);
        let offer = inventory::TradeOffer {
            purchases: vec![inventory::TradeLine {
                entry: potion.unwrap().id,
                quantity: 1,
            }],
            sales: vec![],
        };
        let quote = session.quote_trade(participants, offer).unwrap();
        session.apply(Command::Trade(quote)).unwrap();
        assert_eq!(potions(&session, guard), promised - 1);

        // A command that opens an inventory and then fails leaves it unopened.
        let (mut session, ids) = guards(1);
        let before = session.state().clone();
        let bad = Command::UseItem {
            actor: ids[0],
            item: ItemId([0; 16]),
        };
        assert!(session.apply(bad).is_err());
        assert_eq!(session.state(), &before);
        assert!(session.state().carried(ids[0]).is_err());
    }

    #[test]
    fn a_container_is_filled_the_first_time_it_is_opened() {
        let (mut session, _) = guards(0);
        assert!(session.state().inventory(TRUNK_BAG).is_err());
        let open = Command::World(WorldCommand::Open { object: TRUNK });
        // Shut and locked containers say which of the two they are.
        let contents = |session: &TestSession| {
            let refused = session.container_contents(TRUNK).unwrap_err();
            refused.rejection().cloned()
        };
        assert_eq!(contents(&session), Some(Rejection::Closed(TRUNK)));
        let lock = |locked| {
            Command::World(WorldCommand::SetLocked {
                object: TRUNK,
                locked,
            })
        };
        session.apply(lock(true)).unwrap();
        assert_eq!(contents(&session), Some(Rejection::Locked(TRUNK)));
        assert_eq!(
            rejected(session.apply(open.clone())),
            Rejection::Locked(TRUNK)
        );
        session.apply(lock(false)).unwrap();
        session.apply(open.clone()).unwrap();
        let first = session.container_contents(TRUNK).unwrap().clone();
        assert!(first.entries.iter().any(|e| e.definition == POTION));
        // Emptied and opened again, it stays empty: it is filled once.
        let entries: Vec<(ItemId, u32)> =
            first.entries.iter().map(|e| (e.id, e.quantity)).collect();
        for (item, quantity) in entries {
            session
                .apply(Command::Transfer {
                    source: TRUNK_BAG,
                    destination: HERO_BAG,
                    item,
                    quantity,
                })
                .unwrap();
        }
        session
            .apply(Command::World(WorldCommand::Close { object: TRUNK }))
            .unwrap();
        session.apply(open).unwrap();
        assert!(
            session
                .container_contents(TRUNK)
                .unwrap()
                .entries
                .is_empty()
        );
        // The save holds the opened inventory like any other.
        let saved = serde_json::to_string(session.state()).unwrap();
        let restored: SessionState = serde_json::from_str(&saved).unwrap();
        GameSession::new(ToolContent::new(armed()).unwrap(), restored).unwrap();
    }
}

#[test]
fn item_commands_cost_the_same_in_a_large_catalog() {
    let mut content = content();
    let template = content.items.items[0].clone();
    // As many as a catalog may hold.
    for n in 0..9_997u32 {
        let mut item = template.clone();
        let mut id = [3u8; 16];
        id[..4].copy_from_slice(&n.to_le_bytes());
        item.id = ItemDefinitionId(id);
        content.items.items.push(item);
    }
    content.sort();
    let mut session = GameSession::new(ToolContent::new(content).unwrap(), state()).unwrap();
    let potion = session.state().carried(HERO).unwrap().entries[0].id;
    let started = std::time::Instant::now();
    for _ in 0..3 {
        let use_item = Command::UseItem {
            actor: HERO,
            item: potion,
        };
        session.apply(use_item).unwrap();
    }
    let elapsed = started.elapsed();
    println!("three item commands against 10,000 item definitions: {elapsed:?}");
    // Checking the whole catalog on every item operation took about 3 ms per command in
    // this unoptimised build; with definitions found by bisection it is tens of microseconds.
    assert!(elapsed.as_millis() < 5, "{elapsed:?}");
}
