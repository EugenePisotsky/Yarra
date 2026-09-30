use game_types::*;
use yarra_gameplay::dialogue;
use yarra_gameplay::inventory::{fixtures::*, *};
use yarra_gameplay::rules::{Effect, Modifier};
use yarra_gameplay::{fixtures::*, *};
type Result<T> = yarra_gameplay::Result<T>;
type TestSession = GameSession<ToolContent>;
fn new(content: GameContent, state: SessionState) -> Result<TestSession> {
    GameSession::new(ToolContent::new(content)?, state)
}
fn session() -> TestSession {
    new(content(), state()).unwrap()
}
fn snapshot(session: &TestSession) -> SessionState {
    session.state().clone()
}
fn item(session: &TestSession, bag: InventoryId, definition: ItemDefinitionId) -> ItemId {
    session
        .state()
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
    state.inventories.get_mut(&HERO_BAG).unwrap().revision = i64::MAX as u64;
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
        session
            .state()
            .conversations
            .values()
            .next()
            .unwrap()
            .status,
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
    state.inventories.get_mut(&HERO_BAG).unwrap().owner = OwnerRef::actor(ActorId([99; 16]));
    assert!(new(content(), state).is_err());
    let mut state = snapshot(&source);
    state
        .actors
        .get_mut(&HERO)
        .unwrap()
        .equipment
        .insert(key("hand"), item(&source, MERCHANT_BAG, POTION));
    assert!(new(content(), state).is_err());
    let mut content = content();
    content.game.actions.remove(&binding("consume-key"));
    assert!(content.validate().is_err());
}

/// Counts graph reads so tests can show which commands touch dialogue content.
struct Counting {
    inner: ToolContent,
    graphs: std::rc::Rc<std::cell::Cell<usize>>,
}
impl ContentSource for Counting {
    fn identity(&self) -> ContentIdentity {
        self.inner.identity()
    }
    fn core(&mut self) -> Result<GameContent> {
        self.inner.core()
    }
    fn dialogue(&mut self, id: DialogueId) -> Result<DialoguePack> {
        self.graphs.set(self.graphs.get() + 1);
        self.inner.dialogue(id)
    }
}
#[test]
fn dialogue_graphs_load_on_first_use_and_stay_cached() {
    let graphs = std::rc::Rc::new(std::cell::Cell::new(0));
    let source = Counting {
        inner: ToolContent::new(content()).unwrap(),
        graphs: graphs.clone(),
    };
    let mut session = GameSession::new(source, state()).unwrap();
    assert!(session.content().game.dialogues.is_empty());
    let potion = session.state().carried(HERO).unwrap().entries[0].id;
    session
        .apply(Command::UseItem {
            actor: HERO,
            item: potion,
        })
        .unwrap();
    assert_eq!(graphs.get(), 0);
    session
        .apply(Command::StartDialogue {
            bindings: Default::default(),
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        })
        .unwrap();
    session
        .available_choices(GATE_DIALOGUE, HERO, MERCHANT)
        .unwrap();
    assert_eq!(graphs.get(), 1);
    // A restored playthrough reloads the graph of its active conversation only.
    let saved = serde_json::to_vec(session.state()).unwrap();
    let restored: SessionState = serde_json::from_slice(&saved).unwrap();
    assert_eq!(&restored, session.state());
    let again = std::rc::Rc::new(std::cell::Cell::new(0));
    let source = Counting {
        inner: ToolContent::new(content()).unwrap(),
        graphs: again.clone(),
    };
    GameSession::new(source, restored).unwrap();
    assert_eq!(again.get(), 1);
}
#[test]
fn untouched_records_stay_absent_and_read_as_defaults() {
    let mut session = session();
    let key = actors::RelationshipKey {
        from: MERCHANT,
        to: HERO,
    };
    assert_eq!(session.state().relationship(key).attitude, 0);
    assert!(session.state().relationships.is_empty());
    session
        .apply(Command::AdjustRelationship { key, amount: 250 })
        .unwrap();
    assert_eq!(session.state().relationship(key).attitude, 100);
    assert_eq!(session.state().relationships.len(), 1);
    let before = snapshot(&session);
    assert!(
        session
            .apply(Command::AdjustRelationship {
                key: actors::RelationshipKey {
                    from: HERO,
                    to: HERO
                },
                amount: 1
            })
            .is_err()
    );
    assert_eq!(snapshot(&session), before);
}
#[test]
fn command_cost_does_not_grow_with_the_population() {
    let content = content();
    let mut state = state();
    for n in 0..20_000u32 {
        let mut id = [7u8; 16];
        id[..4].copy_from_slice(&n.to_le_bytes());
        let mut actor = actors::Actor::from_template(
            &content.game.actors[0],
            &content.game.rules,
            actors::ActorRole::Npc,
        )
        .unwrap();
        actor.id = ActorId(id);
        let mut bag = Inventory::new(OwnerRef::actor(actor.id), "carried").unwrap();
        bag.grant(&content.items, POTION, 5).unwrap();
        state.add_actor(actor);
        state.add_inventory(bag);
    }
    let mut session = new(content, state).unwrap();
    let potion = item(&session, HERO_BAG, POTION);
    let started = std::time::Instant::now();
    for _ in 0..2 {
        session
            .apply(Command::UseItem {
                actor: HERO,
                item: potion,
            })
            .unwrap();
    }
    let mut elapsed = started.elapsed();
    let before = snapshot(&session);
    let started = std::time::Instant::now();
    assert!(
        session
            .apply(Command::UseItem {
                actor: HERO,
                item: ItemId([0; 16])
            })
            .is_err()
    );
    elapsed += started.elapsed();
    assert_eq!(snapshot(&session), before);
    // Generous bound: a whole-state copy or check per command would take far longer in
    // this unoptimised test build. Three commands normally finish in well under 1 ms.
    assert!(elapsed.as_millis() < 20, "commands took {elapsed:?}");
}
