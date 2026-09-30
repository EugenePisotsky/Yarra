//! Actions that take time: intents, durations, costs, cooldowns and death.
use game_types::*;
use yarra_gameplay::actors::Intent;
use yarra_gameplay::{fixtures::*, *};

type Result<T> = yarra_gameplay::Result<T>;
type TestSession = GameSession<ToolContent>;

fn open(state: SessionState) -> TestSession {
    GameSession::new(ToolContent::new(content()).unwrap(), state).unwrap()
}
fn session() -> TestSession {
    open(state())
}
fn intent(ability: &str, target: Option<ActorId>, repeat: bool) -> Intent {
    Intent {
        ability: key(ability),
        target,
        repeat,
    }
}
fn intend(session: &mut TestSession, actor: ActorId, intent: Intent) -> Result<CommandOutcome> {
    session.apply(Command::Intend {
        actor,
        intent,
        clear: false,
    })
}
fn strike(target: ActorId) -> Intent {
    intent("strike", Some(target), false)
}
fn pass(session: &mut TestSession, millis: u64) -> Vec<GameEvent> {
    session
        .apply(Command::AdvanceTime { millis })
        .unwrap()
        .events
}
fn stat(session: &TestSession, actor: ActorId, name: &str) -> i32 {
    session.stat(actor, &key(name)).unwrap()
}
fn resolved(events: &[GameEvent]) -> Vec<(ActorId, &str)> {
    events
        .iter()
        .filter_map(|e| match e {
            GameEvent::AbilityResolved { actor, ability, .. } => Some((*actor, ability.as_str())),
            _ => None,
        })
        .collect()
}
fn rejected(result: Result<CommandOutcome>) -> Rejection {
    match result {
        Err(GameplayError::Rejected(rejection)) => rejection,
        other => panic!("expected a rejection, got {other:?}"),
    }
}
fn with_health(actor: ActorId, health: i32) -> SessionState {
    let mut state = state();
    let resources = &mut state.actors.get_mut(&actor).unwrap().resources;
    resources.insert(key("health"), health);
    state
}

#[test]
fn an_action_takes_its_time_and_then_takes_effect() {
    let mut session = session();
    let outcome = intend(&mut session, HERO, strike(MERCHANT)).unwrap();
    assert!(outcome.events.contains(&GameEvent::AbilityBegun {
        actor: HERO,
        ability: key("strike"),
        target: Some(MERCHANT),
        completes_at: GameTime(1500),
    }));
    assert!(session.state().actor(HERO).unwrap().acting.is_some());
    assert_eq!(session.state().timed, [HERO].into());
    assert!(resolved(&pass(&mut session, 1499)).is_empty());
    assert_eq!(stat(&session, MERCHANT, "health"), 100);
    assert_eq!(resolved(&pass(&mut session, 1)), [(HERO, "strike")]);
    // Attack 10 plus a d6.
    assert!((84..=89).contains(&stat(&session, MERCHANT, "health")));
    assert!(session.state().actor(HERO).unwrap().acting.is_none());
    assert!(session.state().timed.is_empty());

    // An ability without a target works on its user, costs stamina and then has to rest.
    intend(&mut session, HERO, intent("second-wind", None, false)).unwrap();
    assert_eq!(stat(&session, HERO, "stamina"), 15);
    pass(&mut session, 500);
    assert_eq!(stat(&session, HERO, "health"), 75);
    let hero = session.state().actor(HERO).unwrap();
    assert_eq!(hero.cooldowns[&key("second-wind")], GameTime(32_000));
}

#[test]
fn what_is_lined_up_is_done_in_order_and_a_repeating_strike_resumes() {
    let mut session = session();
    intend(&mut session, HERO, intent("strike", Some(MERCHANT), true)).unwrap();
    assert_eq!(resolved(&pass(&mut session, 3000)).len(), 2);
    // Asked for in the middle of the third strike: it is done next, then the strikes go on.
    intend(
        &mut session,
        HERO,
        intent("power-strike", Some(MERCHANT), false),
    )
    .unwrap();
    let mut done = Vec::new();
    for _ in 0..45 {
        let events = pass(&mut session, 100);
        done.extend(
            resolved(&events)
                .into_iter()
                .map(|(_, name)| name.to_owned()),
        );
    }
    assert_eq!(done, ["strike", "power-strike", "strike"]);
    // Two strikes, one more, the power strike for twice the attack, and another strike.
    let lost = 100 - stat(&session, MERCHANT, "health");
    assert!((4 * 11 + 20..=4 * 16 + 20).contains(&lost), "{lost}");

    session.apply(Command::Interrupt { actor: HERO }).unwrap();
    let hero = session.state().actor(HERO).unwrap();
    assert!(hero.acting.is_none() && hero.intents.is_empty());
    assert!(session.state().timed.is_empty());
    let health = stat(&session, MERCHANT, "health");
    pass(&mut session, 10_000);
    assert_eq!(stat(&session, MERCHANT, "health"), health);
}

#[test]
fn a_cooldown_makes_a_character_wait_and_costs_are_paid_on_beginning() {
    let mut session = session();
    for _ in 0..3 {
        intend(
            &mut session,
            HERO,
            intent("power-strike", Some(MERCHANT), false),
        )
        .unwrap();
    }
    assert_eq!(stat(&session, HERO, "stamina"), 10);
    pass(&mut session, 1500);
    assert_eq!(stat(&session, MERCHANT, "health"), 80);
    // Ready again six seconds after it took effect; until then the hero only waits.
    let hero = session.state().actor(HERO).unwrap();
    assert!(hero.acting.is_none() && hero.intents.len() == 2);
    assert_eq!(hero.next_event(), Some(GameTime(7500)));
    assert_eq!(session.state().timed, [HERO].into());
    assert!(
        pass(&mut session, 5999)
            .iter()
            .all(|e| *e == GameEvent::TimeAdvanced)
    );
    let events = pass(&mut session, 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, GameEvent::AbilityBegun { .. }))
    );
    assert_eq!(stat(&session, HERO, "stamina"), 0);
    // The third cannot be paid for, so it is dropped as soon as its turn comes.
    let events = pass(&mut session, 1500);
    assert!(events.contains(&GameEvent::IntentDropped {
        actor: HERO,
        ability: key("power-strike"),
        reason: Rejection::NotEnough(key("stamina")),
    }));
    assert_eq!(stat(&session, MERCHANT, "health"), 60);
    assert!(session.state().timed.is_empty());
}

#[test]
fn the_dead_stop_and_stop_being_targets() {
    let mut session = open(with_health(MERCHANT, 5));
    intend(&mut session, MERCHANT, intent("strike", Some(HERO), true)).unwrap();
    intend(&mut session, HERO, intent("strike", Some(MERCHANT), true)).unwrap();
    intend(&mut session, HERO, strike(MERCHANT)).unwrap();
    let events = pass(&mut session, 1500);
    assert!(events.contains(&GameEvent::Died { actor: MERCHANT }));
    // The merchant's own strike landed only if his turn came first at that moment.
    assert!(stat(&session, HERO, "health") <= 50);
    let merchant = session.state().actor(MERCHANT).unwrap();
    assert!(merchant.acting.is_none() && merchant.intents.is_empty());
    // The hero's repeating strike is not lined up again and the queued one is dropped.
    assert!(events.contains(&GameEvent::IntentDropped {
        actor: HERO,
        ability: key("strike"),
        reason: Rejection::Dead(MERCHANT),
    }));
    let hero = session.state().actor(HERO).unwrap();
    assert!(hero.acting.is_none() && hero.intents.is_empty());
    assert!(session.state().timed.is_empty());
    assert_eq!(
        rejected(intend(&mut session, HERO, strike(MERCHANT))),
        Rejection::Dead(MERCHANT)
    );
    assert_eq!(
        rejected(intend(&mut session, MERCHANT, strike(HERO))),
        Rejection::Dead(MERCHANT)
    );

    // A target that dies while the blow is on its way: the blow comes to nothing.
    let mut session = open(with_health(MERCHANT, 5));
    intend(&mut session, COMPANION, strike(MERCHANT)).unwrap();
    pass(&mut session, 700);
    intend(&mut session, HERO, strike(MERCHANT)).unwrap();
    pass(&mut session, 800);
    let events = pass(&mut session, 700);
    assert_eq!(
        events,
        [
            GameEvent::IntentDropped {
                actor: HERO,
                ability: key("strike"),
                reason: Rejection::Dead(MERCHANT),
            },
            GameEvent::TimeAdvanced
        ]
    );
}

#[test]
fn what_cannot_be_asked_for_is_refused_with_a_reason() {
    let mut state = state();
    let hero = state.actors.get_mut(&HERO).unwrap();
    hero.abilities.remove(&key("power-strike"));
    let mut session = open(state);
    let before = session.state().clone();
    let ask = |session: &mut TestSession, intent| rejected(intend(session, HERO, intent));
    assert_eq!(
        ask(&mut session, intent("power-strike", Some(MERCHANT), false)),
        Rejection::AbilityNotKnown(key("power-strike"))
    );
    assert_eq!(
        ask(&mut session, intent("strike", None, false)),
        Rejection::TargetRequired(key("strike"))
    );
    assert_eq!(
        ask(&mut session, intent("second-wind", Some(MERCHANT), false)),
        Rejection::NoTarget(key("second-wind"))
    );
    assert!(intend(&mut session, HERO, intent("fireball", None, false)).is_err());
    assert!(intend(&mut session, HERO, strike(ActorId([9; 16]))).is_err());
    assert_eq!(session.state(), &before);
    // One in hand and eight lined up is as much as a character holds.
    for _ in 0..9 {
        intend(&mut session, HERO, strike(MERCHANT)).unwrap();
    }
    assert_eq!(ask(&mut session, strike(MERCHANT)), Rejection::QueueFull);
    // Asking for something instead of the queue replaces it; the strike in hand goes on.
    session
        .apply(Command::Intend {
            actor: HERO,
            intent: intent("second-wind", None, false),
            clear: true,
        })
        .unwrap();
    let hero = session.state().actor(HERO).unwrap();
    assert_eq!(hero.acting.as_ref().unwrap().intent, strike(MERCHANT));
    assert_eq!(hero.intents, [intent("second-wind", None, false)]);
}

#[test]
fn a_fight_carries_on_from_a_save_exactly_as_it_would_have() {
    let start = state();
    let fight = |save_at: Option<u32>| {
        let mut session = open(start.clone());
        intend(&mut session, HERO, intent("strike", Some(MERCHANT), true)).unwrap();
        intend(&mut session, MERCHANT, intent("strike", Some(HERO), true)).unwrap();
        intend(
            &mut session,
            COMPANION,
            intent("power-strike", Some(MERCHANT), false),
        )
        .unwrap();
        for tick in 0..120 {
            if save_at == Some(tick) {
                let saved = serde_json::to_string(session.state()).unwrap();
                session = open(serde_json::from_str(&saved).unwrap());
            }
            pass(&mut session, 100);
        }
        session.into_state()
    };
    let straight = fight(None);
    assert!(fight(Some(43)) == straight && fight(Some(15)) == straight);
    // Somebody lost: twelve seconds of trading blows is more than fifty health.
    let rules = &content().game.rules;
    assert!(!straight.actors[&HERO].alive(rules) || !straight.actors[&MERCHANT].alive(rules));
}

#[test]
fn a_skirmish_costs_what_happens_in_it_not_the_size_of_the_world() {
    let content = content();
    let mut state = state();
    const FIGHTERS: u32 = 200;
    const BYSTANDERS: u32 = 20_000;
    let id = |n: u32| {
        let mut id = [5u8; 16];
        id[..4].copy_from_slice(&n.to_le_bytes());
        ActorId(id)
    };
    for n in 0..FIGHTERS + BYSTANDERS {
        state
            .spawn(&content, content.game.actors[0].id, id(n))
            .unwrap();
    }
    let mut session = GameSession::new(ToolContent::new(content).unwrap(), state).unwrap();
    for n in 0..FIGHTERS {
        // Pairs trading blows.
        let intent = intent("strike", Some(id(n ^ 1)), true);
        intend(&mut session, id(n), intent).unwrap();
    }
    assert_eq!(session.state().timed.len(), FIGHTERS as usize);
    let started = std::time::Instant::now();
    let mut blows = 0;
    for _ in 0..100 {
        blows += resolved(&pass(&mut session, 100)).len();
    }
    let elapsed = started.elapsed();
    println!(
        "{FIGHTERS} fighting among {BYSTANDERS}: ten seconds in 100 steps took {elapsed:?}, \
         {blows} blows, {:?} per step",
        elapsed / 100
    );
    assert!(blows >= 600, "{blows}");
    // An unoptimised build; the bound only guards against cost growing with the world.
    assert!(elapsed.as_millis() < 3000, "{elapsed:?}");
}
