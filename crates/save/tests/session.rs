use game_types::*;
use gameplay::dialogue;
use gameplay::inventory::{fixtures::*, *};
use gameplay::rules::{Effect, Modifier};
use gameplay::{fixtures::*, *};
use yarra_save::WorkingStore;
type TestSession = gameplay::GameSession<WorkingStore, ToolContent>;
fn new(content: GameContent, state: SessionState) -> yarra_save::Result<TestSession> {
    Ok(GameSession::new(
        WorkingStore::in_memory(&content, &state)?,
        ToolContent::new(content)?,
    )?)
}
fn session() -> TestSession {
    new(content(), state()).unwrap()
}
fn snapshot(session: &TestSession) -> SessionState {
    session.store().export_for_tools(100_000).unwrap()
}
fn item(session: &TestSession, bag: InventoryId, definition: ItemDefinitionId) -> ItemId {
    snapshot(session)
        .inventory(bag)
        .unwrap()
        .entries
        .iter()
        .find(|e| e.definition == definition)
        .unwrap()
        .id
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
fn start(session: &mut TestSession) {
    session
        .apply(Command::StartDialogue {
            bindings: Default::default(),
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        })
        .unwrap();
    session
        .apply(Command::AdvanceLine {
            key: ConversationKey {
                dialogue: GATE_DIALOGUE,
                participant: HERO,
                speaker: MERCHANT,
            },
            expected: dialogue::Token { run: 1, step: 0 },
        })
        .unwrap();
}

#[test]
fn use_equipment_transfer_and_trade_are_coordinated() {
    let mut session = session();
    let potion = item(&session, HERO_BAG, POTION);
    let sword = item(&session, HERO_BAG, SWORD);
    session
        .apply(Command::UseItem {
            actor: HERO,
            item: potion,
        })
        .unwrap();
    assert_eq!(snapshot(&session).actor(HERO).unwrap().health, 75);
    assert_eq!(
        snapshot(&session)
            .inventory(HERO_BAG)
            .unwrap()
            .entry(potion)
            .unwrap()
            .quantity,
        2
    );
    session
        .apply(Command::Equip {
            actor: HERO,
            item: sword,
        })
        .unwrap();
    assert_eq!(session.derived(HERO).unwrap()[&key("strength")], 12);
    session
        .apply(Command::Transfer {
            source: HERO_BAG,
            destination: COMPANION_BAG,
            item: sword,
            quantity: 1,
        })
        .unwrap();
    assert!(snapshot(&session).actor(HERO).unwrap().equipment.is_empty());
    assert_eq!(session.derived(HERO).unwrap()[&key("strength")], 10);
    assert_eq!(
        snapshot(&session)
            .inventory(COMPANION_BAG)
            .unwrap()
            .entry(sword)
            .unwrap()
            .definition,
        SWORD
    );
    let participants = TradeParticipants {
        merchant_inventory: MERCHANT_BAG,
        customer_inventory: HERO_BAG,
        merchant_wallet: MERCHANT_WALLET,
        customer_wallet: PARTY_WALLET,
    };
    let quote = session
        .quote_trade(
            participants,
            TradeOffer {
                purchases: vec![TradeLine {
                    entry: item(&session, MERCHANT_BAG, POTION),
                    quantity: 1,
                }],
                sales: vec![],
            },
        )
        .unwrap();
    session.apply(Command::Trade(quote.clone())).unwrap();
    assert_eq!(
        snapshot(&session)
            .wallet(PARTY_WALLET)
            .unwrap()
            .balance
            .units(),
        80
    );
    let before = snapshot(&session);
    assert!(session.apply(Command::Trade(quote)).is_err());
    assert_eq!(snapshot(&session), before);
}
#[test]
fn rejected_use_restores_health_and_consumption() {
    let source = session();
    let mut state = snapshot(&source);
    state
        .inventories
        .iter_mut()
        .find(|i| i.id == HERO_BAG)
        .unwrap()
        .revision = i64::MAX as u64;
    let mut session = new(content(), state).unwrap();
    let before = snapshot(&session);
    let potion = item(&session, HERO_BAG, POTION);
    assert!(
        session
            .apply(Command::UseItem {
                actor: HERO,
                item: potion
            })
            .is_err()
    );
    assert_eq!(snapshot(&session), before);
}
#[test]
fn dialogue_rewards_once_and_preview_does_not_roll() {
    let mut session = session();
    start(&mut session);
    let before = snapshot(&session);
    assert_eq!(
        session
            .available_choices(GATE_DIALOGUE, HERO, MERCHANT)
            .unwrap(),
        vec![key("return-key")]
    );
    assert_eq!(snapshot(&session), before);
    let events = session.apply(choose()).unwrap();
    assert!(
        events
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::SkillChecked { passed: true, .. }))
    );
    assert!(snapshot(&session).facts.contains(&key("gate-rewarded")));
    assert_eq!(
        snapshot(&session).actor(HERO).unwrap().skills[&key("persuasion")],
        10
    );
    assert!(
        !snapshot(&session)
            .carried(HERO)
            .unwrap()
            .entries
            .iter()
            .any(|e| e.definition == KEY)
    );
    let after = snapshot(&session);
    assert!(session.apply(choose()).is_err());
    assert_eq!(snapshot(&session), after);
    assert!(
        session
            .apply(Command::StartDialogue {
                bindings: Default::default(),
                dialogue: GATE_DIALOGUE,
                participant: HERO,
                speaker: MERCHANT
            })
            .is_err()
    );
}
#[test]
fn failure_after_roll_restores_all_domains_and_rng() {
    let source = session();
    let mut content = content();
    if let Action::SkillCheck { success, .. } =
        content.game.actions.get_mut(&binding("persuade")).unwrap()
    {
        success.insert(
            0,
            Action::GrantItem {
                definition: SWORD,
                quantity: 4097,
            },
        );
    }
    let mut session = new(content, snapshot(&source)).unwrap();
    start(&mut session);
    let before = snapshot(&session);
    assert!(session.apply(choose()).is_err());
    assert_eq!(snapshot(&session), before);
}
#[test]
fn failed_skill_roll_is_a_committed_outcome() {
    let source = session();
    let mut content = content();
    if let Action::SkillCheck { difficulty, .. } =
        content.game.actions.get_mut(&binding("persuade")).unwrap()
    {
        *difficulty = 100;
    }
    let mut session = new(content, snapshot(&source)).unwrap();
    start(&mut session);
    let random = snapshot(&session).random;
    let events = session.apply(choose()).unwrap();
    assert!(
        events
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::SkillChecked { passed: false, .. }))
    );
    assert_ne!(snapshot(&session).random, random);
    assert_eq!(
        snapshot(&session).conversations[0].status,
        dialogue::RunStatus::Completed
    );
    assert!(!snapshot(&session).facts.contains(&key("gate-rewarded")));
}
#[test]
fn timed_effects_derive_from_explicit_time() {
    let source = session();
    let mut content = content();
    content
        .items
        .items
        .iter_mut()
        .find(|i| i.id == POTION)
        .unwrap()
        .mechanics
        .on_use
        .push(Effect::Buff {
            modifier: Modifier {
                attribute: key("strength"),
                amount: 3,
            },
            duration_ms: 1000,
        });
    let mut session = new(content, snapshot(&source)).unwrap();
    let potion = item(&session, HERO_BAG, POTION);
    session
        .apply(Command::UseItem {
            actor: HERO,
            item: potion,
        })
        .unwrap();
    assert_eq!(session.derived(HERO).unwrap()[&key("strength")], 13);
    session.apply(Command::AdvanceTime { millis: 999 }).unwrap();
    assert_eq!(session.derived(HERO).unwrap()[&key("strength")], 13);
    session.apply(Command::AdvanceTime { millis: 1 }).unwrap();
    assert_eq!(session.derived(HERO).unwrap()[&key("strength")], 10);
    assert!(snapshot(&session).actor(HERO).unwrap().effects.is_empty());
}
#[test]
fn cross_domain_validation_rejects_bad_ownership_equipment_and_content() {
    let source = session();
    let mut state = snapshot(&source);
    state.inventories[0].owner = OwnerRef::actor(ActorId([99; 16]));
    assert!(new(content(), state).is_err());
    let mut state = snapshot(&source);
    state.actors[0]
        .equipment
        .insert(key("hand"), item(&source, MERCHANT_BAG, POTION));
    assert!(new(content(), state).is_err());
    let mut content = content();
    content.game.actions.remove(&binding("consume-key"));
    assert!(content.validate().is_err());
}
