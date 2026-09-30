use game_types::*;
use yarra_gameplay::dialogue;
use yarra_gameplay::inventory::{fixtures::*, *};
use yarra_gameplay::rules::Use;
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
    assert_eq!(session.stat(HERO, &key("health")).unwrap(), 75);
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
    assert_eq!(session.stat(HERO, &key("strength")).unwrap(), 12);
    session
        .apply(Command::Transfer {
            source: HERO_BAG,
            destination: COMPANION_BAG,
            item: sword,
            quantity: 1,
        })
        .unwrap();
    assert!(snapshot(&session).actor(HERO).unwrap().equipment.is_empty());
    assert_eq!(session.stat(HERO, &key("strength")).unwrap(), 10);
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
            .any(|e| matches!(e, GameEvent::Checked { passed: true, .. }))
    );
    assert_eq!(
        session
            .state()
            .variable(session.content(), REWARDED)
            .unwrap(),
        Value::Bool(true)
    );
    assert_eq!(session.state().party.experience, 10);
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
    if let Action::Check { success, .. } = persuade(&mut content) {
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
    if let Action::Check { difficulty, .. } = persuade(&mut content) {
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
            .any(|e| matches!(e, GameEvent::Checked { passed: false, .. }))
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
    assert!(session.state().variables.is_empty());
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
        .push(Use::Apply {
            effect: key("fortified"),
            duration_ms: Some(1000),
        });
    let mut session = new(content, snapshot(&source)).unwrap();
    let potion = item(&session, HERO_BAG, POTION);
    session
        .apply(Command::UseItem {
            actor: HERO,
            item: potion,
        })
        .unwrap();
    assert_eq!(session.stat(HERO, &key("strength")).unwrap(), 13);
    session.apply(Command::AdvanceTime { millis: 999 }).unwrap();
    assert_eq!(session.stat(HERO, &key("strength")).unwrap(), 13);
    session.apply(Command::AdvanceTime { millis: 1 }).unwrap();
    assert_eq!(session.stat(HERO, &key("strength")).unwrap(), 10);
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
    *persuade(&mut content) = Action::Set {
        variable: VariableId::named("undeclared"),
        of: None,
        value: Value::Bool(true),
    };
    assert!(content.validate().is_err());
}
#[test]
fn the_fingerprint_does_not_depend_on_which_names_were_parsed() {
    // Named in code only, so nothing has told the process this name yet.
    const NAMED: &str = "fingerprint/named_in_code";
    let mut content = content();
    content.game.variables.push(VariableDefinition {
        id: VariableId::named(NAMED),
        initial: Value::Bool(false),
        scope: Default::default(),
    });
    content.sort();
    let before = content.fingerprint().unwrap();
    VariableId::try_from(NAMED.to_owned()).unwrap();
    assert_eq!(VariableId::named(NAMED).to_string(), NAMED);
    assert_eq!(content.fingerprint().unwrap(), before);
}

#[test]
fn conversations_keep_their_turns_through_a_save_and_hold_the_world_still() {
    let mut session = session();
    let talk = |participant| ConversationKey {
        dialogue: GATE_DIALOGUE,
        participant,
        speaker: MERCHANT,
    };
    // Started in the opposite order to the one their keys sort in.
    let (first, second) = if talk(HERO) > talk(COMPANION) {
        (HERO, COMPANION)
    } else {
        (COMPANION, HERO)
    };
    for participant in [first, second] {
        session
            .apply(Command::StartDialogue {
                bindings: Default::default(),
                dialogue: GATE_DIALOGUE,
                participant,
                speaker: MERCHANT,
            })
            .unwrap();
    }
    let floor = session.state().floor.blocking.clone();
    assert_eq!(floor, [talk(first), talk(second)]);
    // The world waits while a conversation is on screen.
    let time = session.state().time;
    session
        .apply(Command::AdvanceTime { millis: 1000 })
        .unwrap();
    assert_eq!(session.state().time, time);
    // A save keeps whose turn it is, and a save that loses a turn is refused.
    let json = serde_json::to_string(session.state()).unwrap();
    let saved: SessionState = serde_json::from_str(&json).unwrap();
    let mut lost = saved.clone();
    lost.floor.blocking.pop_back();
    assert!(new(content(), lost).is_err());
    let mut session = new(content(), saved).unwrap();
    assert_eq!(session.state().floor.blocking, floor);
    // Leaving one hands the floor to the next; with both over, time moves on.
    for (key, next) in [(talk(first), Some(talk(second))), (talk(second), None)] {
        let expected = session.conversation_view(key).unwrap().token;
        session
            .apply(Command::InterruptDialogue { key, expected })
            .unwrap();
        assert_eq!(
            session.state().floor.current(dialogue::Mode::Blocking),
            next
        );
    }
    session
        .apply(Command::AdvanceTime { millis: 1000 })
        .unwrap();
    assert_eq!(session.state().time, GameTime(time.0 + 1000));
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
    fn dialogue(&mut self, id: DialogueId) -> Result<dialogue::Dialogue> {
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
        let id = ActorId(id);
        state
            .spawn(&content, content.game.actors[0].id, id)
            .unwrap();
        let mut bag = Inventory::new(OwnerRef::actor(id), "carried").unwrap();
        bag.grant(&content.items, POTION, 5).unwrap();
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

mod party {
    use super::*;
    use yarra_gameplay::dialogue::{Dialogue, Node, NodeKind, Repeat, Role};

    const BANTER: DialogueId = DialogueId([9; 16]);
    const STRANGER: ActorId = ActorId([77; 16]);
    fn node(id: &str, kind: NodeKind, speaker: &str, children: &[&str]) -> Node {
        Node {
            id: key(id),
            kind,
            speaker: key(speaker),
            text: TextRef::from(id),
            arguments: Default::default(),
            repeat: Repeat::Always,
            condition: None,
            actions: vec![],
            children: children.iter().map(|c| key(c)).collect(),
        }
    }
    /// The merchant greets; whoever travels with the hero may cut in before the hero answers.
    /// Two named characters together get a line neither has alone, and the merchant answers
    /// a companion directly.
    fn banter() -> Dialogue {
        let line = NodeKind::Line;
        let mut pair = node("pair", line, "companion", &["retort"]);
        pair.condition = Some(Condition::Present(STRANGER));
        let mut counted = node("counted", NodeKind::Choice, "player", &[]);
        counted.repeat = Repeat::OncePerRun;
        Dialogue {
            id: BANTER,
            roles: [
                (key("player"), Role::Required),
                (key("speaker"), Role::Required),
                (key("companion"), Role::Actor(COMPANION)),
                (key("stranger"), Role::Actor(STRANGER)),
                (key("witness"), Role::Optional),
            ]
            .into(),
            history_scope: dialogue::ScopeSelector::Interaction,
            repeat: dialogue::RepeatPolicy::Always,
            mode: Default::default(),
            start: vec![key("greeting")],
            nodes: vec![
                node(
                    "greeting",
                    line,
                    "speaker",
                    &["pair", "aside", "overheard", "ask", "bye"],
                ),
                pair,
                node("retort", line, "stranger", &["reply"]),
                node("aside", line, "companion", &["reply"]),
                node("reply", line, "speaker", &["ask", "bye"]),
                node("overheard", line, "witness", &["ask", "bye"]),
                node("ask", NodeKind::Choice, "player", &["answer"]),
                node("answer", line, "speaker", &["counted", "ask", "bye"]),
                counted,
                node("bye", NodeKind::Choice, "player", &[]),
            ],
        }
    }
    fn session(party: &[ActorId], stranger: bool) -> TestSession {
        let mut content = content();
        let graph = banter();
        content.game.dialogue_contracts.push(graph.contract());
        content.game.dialogues.push(graph);
        let mut state = state();
        if stranger {
            let mut actor = state.actors[&COMPANION].clone();
            actor.id = STRANGER;
            let mut bag = Inventory::new(OwnerRef::actor(STRANGER), "carried").unwrap();
            bag.id = InventoryId([77; 16]);
            state.add_actor(actor);
            state.add_inventory(bag);
        }
        let mut session = new(content, state).unwrap();
        for actor in party {
            session
                .apply(Command::Party {
                    actor: *actor,
                    member: true,
                })
                .unwrap();
        }
        session
    }
    const KEY: ConversationKey = ConversationKey {
        dialogue: BANTER,
        participant: HERO,
        speaker: MERCHANT,
    };
    fn start(session: &mut TestSession, bindings: &[(&str, ActorId)]) {
        session
            .apply(Command::StartDialogue {
                bindings: bindings.iter().map(|(r, a)| (key(r), *a)).collect(),
                dialogue: BANTER,
                participant: HERO,
                speaker: MERCHANT,
            })
            .unwrap();
    }
    /// Acknowledges lines until the player has to choose; returns who said what.
    fn listen(session: &mut TestSession) -> Vec<(ActorId, String)> {
        let mut heard = Vec::new();
        loop {
            let view = session.conversation_view(KEY).unwrap();
            let Some(line) = view.line else {
                return heard;
            };
            heard.push((line.speaker, line.id.as_str().to_owned()));
            session
                .apply(Command::AdvanceLine {
                    key: KEY,
                    expected: view.token,
                })
                .unwrap();
        }
    }
    fn choices(session: &mut TestSession) -> Vec<String> {
        let view = session.conversation_view(KEY).unwrap();
        view.choices
            .iter()
            .map(|c| c.id.as_str().to_owned())
            .collect()
    }
    fn pick(session: &mut TestSession, choice: &str) {
        let expected = session.conversation_view(KEY).unwrap().token;
        session
            .apply(Command::Choose {
                expected,
                dialogue: BANTER,
                participant: HERO,
                speaker: MERCHANT,
                choice: key(choice),
            })
            .unwrap();
    }
    fn said(lines: &[(ActorId, &str)]) -> Vec<(ActorId, String)> {
        lines.iter().map(|(a, l)| (*a, l.to_string())).collect()
    }

    #[test]
    fn nobody_cuts_in_when_the_hero_is_alone() {
        let mut session = session(&[], true);
        start(&mut session, &[]);
        assert_eq!(listen(&mut session), said(&[(MERCHANT, "greeting")]));
        assert_eq!(choices(&mut session), ["ask", "bye"]);
    }
    #[test]
    fn a_companion_in_the_party_reacts_and_the_speaker_answers_them() {
        let mut session = session(&[HERO, COMPANION], false);
        start(&mut session, &[]);
        assert_eq!(
            listen(&mut session),
            said(&[
                (MERCHANT, "greeting"),
                (COMPANION, "aside"),
                (MERCHANT, "reply")
            ])
        );
        assert_eq!(choices(&mut session), ["ask", "bye"]);
    }
    #[test]
    fn two_companions_together_get_their_own_exchange() {
        let mut session = session(&[HERO, COMPANION, STRANGER], true);
        start(&mut session, &[]);
        assert_eq!(
            listen(&mut session),
            said(&[
                (MERCHANT, "greeting"),
                (COMPANION, "pair"),
                (STRANGER, "retort"),
                (MERCHANT, "reply")
            ])
        );
        // The stranger alone has no line of their own here.
        let mut session = self::session(&[HERO, STRANGER], true);
        start(&mut session, &[]);
        assert_eq!(listen(&mut session), said(&[(MERCHANT, "greeting")]));
    }
    #[test]
    fn a_bystander_bound_for_this_conversation_speaks_without_joining_the_party() {
        let mut session = session(&[], true);
        start(&mut session, &[("witness", STRANGER)]);
        assert_eq!(
            listen(&mut session),
            said(&[(MERCHANT, "greeting"), (STRANGER, "overheard")])
        );
        assert_eq!(session.state().party.members, [HERO].into());
        // A named character cannot be supplied under someone else's role.
        let mut other = self::session(&[], true);
        assert!(
            other
                .apply(Command::StartDialogue {
                    bindings: [(key("companion"), STRANGER)].into(),
                    dialogue: BANTER,
                    participant: HERO,
                    speaker: MERCHANT,
                })
                .is_err()
        );
    }
    #[test]
    fn hubs_loop_and_per_run_choices_disappear_once_taken() {
        let mut session = session(&[], false);
        start(&mut session, &[]);
        listen(&mut session);
        pick(&mut session, "ask");
        assert_eq!(listen(&mut session), said(&[(MERCHANT, "answer")]));
        assert_eq!(choices(&mut session), ["counted", "ask", "bye"]);
        pick(&mut session, "counted");
        assert_eq!(
            session.state().conversation(KEY).unwrap().status,
            dialogue::RunStatus::Completed
        );
        // Every acknowledged line and picked choice is remembered for later conditions.
        let history = session.state().histories.values().next().unwrap().clone();
        assert_eq!(history.completed, 1);
        assert_eq!(history.count(&dialogue::HistoryEvent::Node(key("ask"))), 1);
        assert_eq!(
            history.count(&dialogue::HistoryEvent::Node(key("answer"))),
            1
        );
        start(&mut session, &[]);
        listen(&mut session);
        pick(&mut session, "ask");
        listen(&mut session);
        assert_eq!(choices(&mut session), ["counted", "ask", "bye"]);
    }
    #[test]
    fn leaving_the_party_mid_game_removes_the_reaction_and_a_failed_command_keeps_the_party() {
        let mut session = session(&[HERO, COMPANION], false);
        let before = snapshot(&session);
        assert!(
            session
                .apply(Command::Party {
                    actor: ActorId([200; 16]),
                    member: true
                })
                .is_err()
        );
        assert_eq!(snapshot(&session), before);
        session
            .apply(Command::Party {
                actor: COMPANION,
                member: false,
            })
            .unwrap();
        start(&mut session, &[]);
        assert_eq!(listen(&mut session), said(&[(MERCHANT, "greeting")]));
    }
}

mod variables {
    use super::*;
    const VISITS: VariableId = VariableId::named("old_gate/visits");
    fn with_counter(actions: Vec<Action>) -> GameContent {
        let mut content = content();
        content.game.variables.push(VariableDefinition {
            id: VISITS,
            initial: Value::Int(0),
            scope: Default::default(),
        });
        let choice = &mut content.game.dialogues[0].nodes[1];
        choice.actions = actions;
        content.sort();
        content
    }
    fn add(amount: i64) -> Action {
        Action::Add {
            variable: VISITS,
            of: None,
            amount,
        }
    }
    fn holds(session: &TestSession, test: Test) -> bool {
        let condition = Condition::Variable {
            variable: VISITS,
            of: None,
            test,
        };
        session
            .content()
            .evaluate(&condition, session.state(), HERO, MERCHANT)
            .unwrap()
            .matched
    }
    #[test]
    fn an_unset_variable_reads_as_its_initial_value_and_counts_from_there() {
        let mut session = new(with_counter(vec![add(2), add(-5)]), state()).unwrap();
        assert!(session.state().variables.is_empty());
        assert!(holds(&session, Test::Is(Value::Int(0))));
        start(&mut session);
        session.apply(choose()).unwrap();
        assert_eq!(session.state().variables[&VISITS.into()], Value::Int(-3));
        assert!(holds(&session, Test::AtMost(-3)) && !holds(&session, Test::AtLeast(0)));
        // The untouched flag is still absent and still reads as false.
        assert_eq!(session.state().variables.len(), 1);
        assert_eq!(
            session
                .state()
                .variable(session.content(), REWARDED)
                .unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn a_later_failure_undoes_earlier_variable_changes() {
        let failing = Action::ConsumeItem {
            definition: KEY,
            quantity: 9,
        };
        let mut session = new(with_counter(vec![add(1), failing]), state()).unwrap();
        start(&mut session);
        let before = snapshot(&session);
        assert!(session.apply(choose()).is_err());
        assert_eq!(snapshot(&session), before);
    }
    #[test]
    fn tests_and_values_must_fit_the_variable_type() {
        let wrong_value = Action::Set {
            variable: VISITS,
            of: None,
            value: Value::Text("many".into()),
        };
        assert!(with_counter(vec![wrong_value]).validate().is_err());
        let not_a_number = Action::Add {
            variable: REWARDED,
            of: None,
            amount: 1,
        };
        assert!(with_counter(vec![not_a_number]).validate().is_err());
        let content = with_counter(vec![]);
        let at_least = Condition::Variable {
            variable: REWARDED,
            of: None,
            test: Test::AtLeast(1),
        };
        assert!(content.validate_condition(&at_least).is_err());
        // Saved state with a value of the wrong type is rejected when loaded.
        let mut state = state();
        state.variables.insert(VISITS.into(), Value::Bool(true));
        assert!(new(content, state).is_err());
    }
    const INSULTED: VariableId = VariableId::named("old_gate/insulted");
    /// The gate choice insults whoever is spoken to, and is only offered to those not yet
    /// insulted.
    fn insulting() -> GameContent {
        let mut content = with_counter(vec![Action::Set {
            variable: INSULTED,
            of: Some(Participant::Speaker),
            value: Value::Bool(true),
        }]);
        content.game.variables.push(VariableDefinition {
            id: INSULTED,
            initial: Value::Bool(false),
            scope: VariableScope::Actor,
        });
        let graph = &mut content.game.dialogues[0];
        graph.repeat = dialogue::RepeatPolicy::Always;
        graph.nodes[1].repeat = dialogue::Repeat::Always;
        graph.nodes[1].condition = Some(Condition::Variable {
            variable: INSULTED,
            of: Some(Participant::Speaker),
            test: Test::Is(Value::Bool(false)),
        });
        content.game.dialogue_contracts = vec![graph.contract()];
        content.sort();
        content
    }
    fn offered(session: &mut TestSession, speaker: ActorId) -> bool {
        let key = ConversationKey {
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker,
        };
        session
            .apply(Command::StartDialogue {
                bindings: Default::default(),
                dialogue: GATE_DIALOGUE,
                participant: HERO,
                speaker,
            })
            .unwrap();
        let expected = session.conversation_view(key).unwrap().token;
        session
            .apply(Command::AdvanceLine { key, expected })
            .unwrap();
        let view = session.conversation_view(key).unwrap();
        let Some(choice) = view.choices.first() else {
            return false;
        };
        session
            .apply(Command::Choose {
                expected: view.token,
                dialogue: GATE_DIALOGUE,
                participant: HERO,
                speaker,
                choice: choice.id.clone(),
            })
            .unwrap();
        true
    }
    #[test]
    fn each_actor_has_its_own_value_of_a_per_actor_variable() {
        let mut session = new(insulting(), state()).unwrap();
        assert!(offered(&mut session, MERCHANT));
        // The merchant remembers; the companion was never insulted.
        assert!(!offered(&mut session, MERCHANT));
        assert!(offered(&mut session, COMPANION));
        let state = session.state();
        let of = |actor| VariableKey {
            variable: INSULTED,
            actor: Some(actor),
        };
        assert_eq!(state.variables.len(), 2);
        let value = |actor| state.variable(session.content(), of(actor)).unwrap();
        assert_eq!(value(MERCHANT), Value::Bool(true));
        assert_eq!(value(HERO), Value::Bool(false));
        // The values survive a save.
        let saved = serde_json::to_vec(state).unwrap();
        assert_eq!(
            &serde_json::from_slice::<SessionState>(&saved).unwrap(),
            state
        );
        // A per-actor variable cannot be read without saying whose.
        assert!(state.variable(session.content(), INSULTED).is_err());
        assert!(
            state
                .variable(session.content(), of(ActorId::named("nobody")))
                .is_err()
        );
    }
    #[test]
    fn content_must_name_an_actor_exactly_for_per_actor_variables() {
        let mut content = insulting();
        content.game.dialogues[0].nodes[1].condition = Some(Condition::Variable {
            variable: INSULTED,
            of: None,
            test: Test::Is(Value::Bool(false)),
        });
        assert!(
            content
                .validate()
                .unwrap_err()
                .to_string()
                .contains("per actor")
        );
        let mut content = insulting();
        content.game.dialogues[0].nodes[1].actions = vec![Action::Add {
            variable: VISITS,
            of: Some(Participant::Player),
            amount: 1,
        }];
        assert!(
            content
                .validate()
                .unwrap_err()
                .to_string()
                .contains("per actor")
        );
    }
}
