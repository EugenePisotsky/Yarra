use game_types::*;
use gameplay::dialogue::{ArgumentSource, Dialogue, HistoryEvent, Node, Repeat, Role, Token};
use gameplay::quests::Transition;
use gameplay::{
    Action, Command, Condition, ConversationKey, GameEvent, Participant, Test, Value,
    VariableDefinition, VariableKey, VariableScope,
    fixtures::{COMPANION, HERO, MERCHANT, key},
};
use gameplay::{dialogue, inventory};
use save::{SaveDirectory, SaveSlot};
use yarra_game_content::*;
mod support;
use support::{Temp, read, write};
const REWARD: DialogueId = DialogueId::named("guard/reward");
const DUTY: DialogueId = DialogueId::named("guard/duty");
const A: DialogueId = DialogueId::named("guard/welcome_a");
const B: DialogueId = DialogueId::named("guard/welcome_b");
/// The reward the guard's own script gives once.
const REWARDED: VariableId = VariableId::named("guard/rewarded");
/// The reward `reward_source` gives once, for the playthrough or for each player.
const THANKED: VariableId = VariableId::named("guard/thanked");
const GATE: QuestId = QuestId::named("guard/gate");
fn pair(dialogue: DialogueId) -> ConversationKey {
    ConversationKey {
        dialogue,
        participant: HERO,
        speaker: MERCHANT,
    }
}
/// The merchant built from the guard's template, so it has the guard's conversations.
fn merchant_as_guard(root: &std::path::Path) {
    let path = root.join("packages/core/characters.ron");
    let mut characters: Vec<gameplay::actors::CharacterDefinition> = read(&path);
    let merchant = characters.iter_mut().find(|c| c.id == MERCHANT).unwrap();
    merchant.template = ActorTemplateId::named("gate_guard");
    write(path, &characters);
}
fn load(temp: &Temp, root: &std::path::Path) -> (LoadedProject, gameplay::GameSession) {
    let project = LoadedProject::load_directory(root).unwrap();
    let session = support::runtime(temp, &project);
    (project, session)
}
fn start(session: &mut gameplay::GameSession, key: ConversationKey) {
    session
        .apply(Command::StartDialogue {
            dialogue: key.dialogue,
            participant: key.participant,
            speaker: key.speaker,
            bindings: Default::default(),
        })
        .unwrap();
}
fn read_lines(session: &mut gameplay::GameSession, key: ConversationKey) {
    for _ in 0..64 {
        let view = session.conversation_view(key).unwrap();
        if view.line.is_none() {
            return;
        }
        session
            .apply(Command::AdvanceLine {
                key,
                expected: view.token,
            })
            .unwrap();
    }
    panic!("line budget exhausted");
}
fn choose(
    session: &mut gameplay::GameSession,
    key: ConversationKey,
    id: &str,
) -> gameplay::CommandOutcome {
    let view = session.conversation_view(key).unwrap();
    assert!(view.choices.iter().any(|c| c.id == key_fn(id)));
    session
        .apply(Command::Choose {
            dialogue: key.dialogue,
            participant: key.participant,
            speaker: key.speaker,
            choice: key_fn(id),
            expected: view.token,
        })
        .unwrap()
}
fn key_fn(s: &str) -> Key {
    key(s)
}
fn history(
    session: &mut gameplay::GameSession,
    project: &LoadedProject,
    key: ConversationKey,
) -> dialogue::History {
    let key = project
        .content()
        .dialogue_contract(key.dialogue)
        .unwrap()
        .history_key(key.participant, key.speaker);
    session.state().history(key).clone()
}
fn given(session: &gameplay::GameSession, variable: VariableId, actor: Option<ActorId>) -> bool {
    let key = VariableKey { variable, actor };
    session.state().variable(session.content(), key).unwrap() == Value::Bool(true)
}
fn node<'a>(graph: &'a mut Dialogue, id: &str) -> &'a mut Node {
    graph.nodes.iter_mut().find(|n| n.id == key(id)).unwrap()
}
/// A reward given once, whether for the playthrough or for each player, however often the
/// choice that gives it is picked.
fn reward_source(root: &std::path::Path, scope: VariableScope) {
    let package_path = root.join("packages/guard/package.ron");
    let mut package: PackageFile = read(&package_path);
    package.variables.push(VariableDefinition {
        id: THANKED,
        initial: Value::Bool(false),
        scope,
    });
    write(&package_path, &package);
    let of = (scope == VariableScope::Actor).then_some(Participant::Player);
    let graph_path = root.join("packages/guard/conversations/reward.dialogue.ron");
    let mut graph: dialogue::Dialogue = read(&graph_path);
    let reward = node(&mut graph, "return-key");
    reward.condition = None;
    reward.actions = vec![Action::If {
        condition: Condition::Variable {
            variable: THANKED,
            of,
            test: Test::Is(Value::Bool(false)),
        },
        then: vec![
            Action::Set {
                variable: THANKED,
                of,
                value: Value::Bool(true),
            },
            Action::Check {
                skill: key("persuasion"),
                difficulty: 1,
                success: vec![
                    Action::AwardExperience { amount: 10 },
                    Action::GrantItem {
                        definition: inventory::fixtures::SWORD,
                        quantity: 1,
                    },
                ],
                failure: vec![],
            },
        ],
        otherwise: vec![],
    }];
    let reward = reward.clone();
    write(graph_path, &graph);
    // The same variable can guard a reward offered by another graph or NPC.
    let second = root.join("packages/guard/conversations/duty.dialogue.ron");
    let mut duty: dialogue::Dialogue = read(&second);
    node(&mut duty, "greeting")
        .children
        .insert(0, reward.id.clone());
    duty.nodes.push(reward);
    write(second, &duty);
}

#[test]
fn a_reward_given_once_stays_given_across_graphs_npcs_and_a_restore() {
    let temp = Temp::new();
    let root = temp.source();
    reward_source(&root, VariableScope::Playthrough);
    let (project, mut session) = load(&temp, &root);
    for k in [
        pair(REWARD),
        pair(REWARD),
        pair(DUTY),
        ConversationKey {
            speaker: COMPANION,
            ..pair(DUTY)
        },
    ] {
        start(&mut session, k);
        read_lines(&mut session, k);
        choose(&mut session, k, "return-key");
    }
    assert!(given(&session, THANKED, None));
    assert_eq!(session.state().party.experience, 10);
    assert_eq!(
        history(&mut session, &project, pair(REWARD)).count(&HistoryEvent::Node(key("return-key"))),
        2
    );
    assert_eq!(history(&mut session, &project, pair(REWARD)).completed, 2);
    let saves = SaveDirectory::new(temp.0.join("slots"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let mut restored = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    let before = restored.state().carried(HERO).unwrap().entries.clone();
    start(&mut restored, pair(REWARD));
    read_lines(&mut restored, pair(REWARD));
    let outcome = choose(&mut restored, pair(REWARD), "return-key");
    assert!(
        !outcome
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::Checked { .. }))
    );
    assert_eq!(restored.state().carried(HERO).unwrap().entries, before);
}

#[test]
fn a_reward_given_once_per_player_is_still_there_for_the_next() {
    let temp = Temp::new();
    let root = temp.source();
    reward_source(&root, VariableScope::Actor);
    let (_, mut session) = load(&temp, &root);
    for (rewards, player) in [(1, HERO), (2, COMPANION)] {
        let k = ConversationKey {
            participant: player,
            ..pair(REWARD)
        };
        start(&mut session, k);
        read_lines(&mut session, k);
        choose(&mut session, k, "return-key");
        assert!(given(&session, THANKED, Some(player)));
        // Each of them earns it; the experience goes to the party either way.
        assert_eq!(session.state().party.experience, 10 * rewards);
    }
}

#[test]
fn interruption_and_previous_choices_drive_selection_without_giving_rewards() {
    let temp = Temp::new();
    let root = temp.source();
    merchant_as_guard(&root);
    let mut source: Scenario = read(root.join("scenario.ron"));
    source.steps.clear();
    write(root.join("scenario.ron"), &source);
    let (project, mut session) = load(&temp, &root);
    session
        .apply(Command::Quest {
            quest: GATE,
            transition: Transition::Start,
        })
        .unwrap();
    let talk = || Command::Talk {
        participant: HERO,
        speaker: MERCHANT,
        topic: None,
        bindings: Default::default(),
    };
    session.apply(talk()).unwrap();
    let key = pair(REWARD);
    let first = session.conversation_view(key).unwrap();
    assert_eq!(first.line.unwrap().speaker, MERCHANT);
    assert!(first.choices.is_empty());
    let before = session.state().clone();
    assert!(
        session
            .apply(Command::Choose {
                dialogue: REWARD,
                participant: HERO,
                speaker: MERCHANT,
                choice: key_fn("return-key"),
                expected: first.token
            })
            .is_err()
    );
    assert_eq!(session.state().clone(), before);
    session
        .apply(Command::AdvanceLine {
            key,
            expected: first.token,
        })
        .unwrap();
    let second = session.conversation_view(key).unwrap();
    assert_eq!(second.line.as_ref().unwrap().speaker, HERO);
    let saves = SaveDirectory::new(temp.0.join("slots"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let mut restored = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    assert_eq!(restored.conversation_view(key).unwrap(), second);
    restored
        .apply(Command::InterruptDialogue {
            key,
            expected: second.token,
        })
        .unwrap();
    let h = history(&mut restored, &project, key);
    assert_eq!((h.started, h.completed, h.interrupted), (1, 0, 1));
    assert!(!given(&restored, REWARDED, None));
    restored.apply(talk()).unwrap();
    assert_eq!(
        restored.conversation_view(key).unwrap().token,
        Token { run: 2, step: 0 }
    );
    let before = restored.header();
    assert!(
        restored
            .apply(Command::AdvanceLine {
                key,
                expected: second.token
            })
            .is_err()
    );
    assert_eq!(restored.header(), before);
    read_lines(&mut restored, key);
    choose(&mut restored, key, "refuse");
    assert_eq!(
        restored
            .preview_interaction(HERO, MERCHANT)
            .unwrap()
            .opening()
            .unwrap()
            .rule,
        key_fn("refused")
    );
    let h = history(&mut restored, &project, key);
    assert_eq!((h.started, h.completed, h.interrupted), (2, 1, 1));
    assert_eq!(h.count(&HistoryEvent::Node(key_fn("greeting"))), 2);
    assert!(!given(&restored, REWARDED, None));
}

#[test]
fn three_roles_bind_localized_names_and_numeric_attributes_and_restore() {
    let temp = Temp::new();
    let root = temp.source();
    let graph_path = root.join("packages/guard/conversations/reward.dialogue.ron");
    let mut graph: dialogue::Dialogue = read(&graph_path);
    graph.roles.insert(key("companion"), Role::Required);
    node(&mut graph, "greeting").arguments.insert(
        "strength".into(),
        ArgumentSource::Stat {
            role: key("companion"),
            stat: key("strength"),
        },
    );
    write(graph_path, &graph);
    // Using it in the English text is what gives the message the argument.
    for locale in ["en", "uk"] {
        let p = root.join(format!("packages/guard/{locale}.ftl"));
        let s = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<_> = s
            .lines()
            .map(|l| {
                if l.starts_with("reward =") {
                    format!("{l} ({{ $strength }})")
                } else {
                    l.to_owned()
                }
            })
            .collect();
        std::fs::write(p, lines.join("\n") + "\n").unwrap();
    }
    let mut scenario: Scenario = read(root.join("scenario.ron"));
    scenario
        .actors
        .iter_mut()
        .find(|a| a.id == HERO)
        .unwrap()
        .name = Some(TextRef::from("Ada"));
    write(root.join("scenario.ron"), &scenario);
    let (project, mut session) = load(&temp, &root);
    let k = pair(REWARD);
    let before = session.state().clone();
    assert!(
        session
            .apply(Command::StartDialogue {
                dialogue: REWARD,
                participant: HERO,
                speaker: MERCHANT,
                bindings: Default::default()
            })
            .is_err()
    );
    assert_eq!(session.state().clone(), before);
    session
        .apply(Command::StartDialogue {
            dialogue: REWARD,
            participant: HERO,
            speaker: MERCHANT,
            bindings: [(key("companion"), COMPANION)].into(),
        })
        .unwrap();
    let before = session.header();
    let view = session.conversation_view(k).unwrap();
    let text = &view.line.as_ref().unwrap().text;
    assert_eq!(text.arguments["strength"], BoundArgument::Number(10));
    for locale in ["en", "uk"] {
        let formatted = project.localization().format_bound(locale, text).unwrap();
        assert!(formatted.value.contains("Ada") && formatted.value.contains("10"));
        assert_eq!(formatted.locale.as_deref(), Some(locale));
    }
    assert_eq!(session.header(), before);
    session
        .apply(Command::AdvanceLine {
            key: k,
            expected: view.token,
        })
        .unwrap();
    assert_eq!(
        session.conversation_view(k).unwrap().line.unwrap().speaker,
        COMPANION
    );
    let saves = SaveDirectory::new(temp.0.join("slots"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let restored = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    assert_eq!(
        restored.conversation_view(k).unwrap(),
        session.conversation_view(k).unwrap()
    );
}

#[test]
fn hub_choices_repeat_per_run_or_history_and_npc_instances_stay_independent() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/conversations/welcome-a.dialogue.ron");
    let mut graph: dialogue::Dialogue = read(&path);
    for (id, repeat) in [("run", Repeat::OncePerRun), ("ever", Repeat::OnceEver)] {
        let mut choice = node(&mut graph, "leave").clone();
        choice.id = key(id);
        choice.repeat = repeat;
        choice.children = vec![key("greeting")];
        node(&mut graph, "greeting")
            .children
            .push(choice.id.clone());
        graph.nodes.push(choice);
    }
    write(path, &graph);
    let (project, mut session) = load(&temp, &root);
    let k = pair(A);
    start(&mut session, k);
    read_lines(&mut session, k);
    choose(&mut session, k, "run");
    read_lines(&mut session, k);
    assert!(
        !session
            .available_choices(A, HERO, MERCHANT)
            .unwrap()
            .contains(&key("run"))
    );
    choose(&mut session, k, "ever");
    read_lines(&mut session, k);
    assert_eq!(
        session.available_choices(A, HERO, MERCHANT).unwrap(),
        vec![key("leave")]
    );
    choose(&mut session, k, "leave");
    start(&mut session, k);
    read_lines(&mut session, k);
    let choices = session.available_choices(A, HERO, MERCHANT).unwrap();
    assert!(choices.contains(&key("run")) && !choices.contains(&key("ever")));
    let other = ConversationKey {
        speaker: COMPANION,
        ..k
    };
    start(&mut session, other);
    read_lines(&mut session, other);
    assert!(
        session
            .available_choices(A, HERO, COMPANION)
            .unwrap()
            .contains(&key("ever"))
    );
    assert_eq!(
        history(&mut session, &project, other).count(&HistoryEvent::Node(key("ever"))),
        0
    );
}

#[test]
fn repeat_contracts_filter_variants_and_cooldowns_use_saved_logical_time() {
    let temp = Temp::new();
    let root = temp.source();
    for (name, repeat) in [
        ("welcome-a", dialogue::RepeatPolicy::OnceCompleted),
        (
            "welcome-b",
            dialogue::RepeatPolicy::Cooldown { millis: 500 },
        ),
    ] {
        let path = root.join(format!("packages/guard/conversations/{name}.dialogue.ron"));
        let mut graph: dialogue::Dialogue = read(&path);
        graph.repeat = repeat;
        write(path, &graph);
    }
    merchant_as_guard(&root);
    let mut source: Scenario = read(root.join("scenario.ron"));
    source.steps.clear();
    write(root.join("scenario.ron"), &source);
    let (_, mut session) = load(&temp, &root);
    for d in [A, B] {
        start(&mut session, pair(d));
        read_lines(&mut session, pair(d));
        choose(&mut session, pair(d), "leave");
    }
    assert!(
        session
            .preview_interaction(HERO, MERCHANT)
            .unwrap()
            .opening()
            .is_none()
    );
    let before = session.header();
    assert!(
        session
            .apply(Command::Talk {
                participant: HERO,
                speaker: MERCHANT,
                topic: None,
                bindings: Default::default()
            })
            .is_err()
    );
    assert_eq!(session.header(), before);
    let saves = SaveDirectory::new(temp.0.join("slots"), 1).unwrap();
    saves.quicksave(&session).unwrap();
    let mut session = saves
        .load(
            SaveSlot::Quick,
            ContentRepository::open(temp.0.join("content.sqlite")).unwrap(),
        )
        .unwrap();
    session.apply(Command::AdvanceTime { millis: 499 }).unwrap();
    assert!(
        session
            .preview_interaction(HERO, MERCHANT)
            .unwrap()
            .opening()
            .is_none()
    );
    session.apply(Command::AdvanceTime { millis: 1 }).unwrap();
    let preview = session.preview_interaction(HERO, MERCHANT).unwrap();
    let rule = preview.opening().unwrap();
    assert_eq!(rule.variants.len(), 1);
    assert_eq!(rule.variants[0].dialogue, B);
    assert_eq!(rule.unavailable_variants, [A]);
}

#[test]
fn invalid_speakers_message_bindings_and_history_references_fail_publication() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/conversations/reward.dialogue.ron");
    let original: dialogue::Dialogue = read(&path);
    for change in 0..4 {
        let mut graph = original.clone();
        match change {
            0 => node(&mut graph, "greeting").speaker = key("unknown"),
            1 => {
                node(&mut graph, "greeting").arguments.clear();
            }
            // An argument the message does not take.
            2 => {
                node(&mut graph, "greeting")
                    .arguments
                    .insert("mood".into(), ArgumentSource::Number(1));
            }
            _ => graph.nodes[1].id = graph.nodes[0].id.clone(),
        }
        write(&path, &graph);
        assert!(LoadedProject::load_directory(&root).is_err());
    }
    write(path, &original);
    let path = root.join("packages/guard/guard.profile.ron");
    let mut profile: gameplay::InteractionProfile = read(&path);
    profile.rules[0].condition = gameplay::Condition::History {
        dialogue: REWARD,
        event: HistoryEvent::Node(key("missing")),
        minimum: 1,
    };
    write(path, &profile);
    assert!(
        LoadedProject::load_directory(root)
            .err()
            .unwrap()
            .to_string()
            .contains("unknown history node")
    );
}

#[test]
fn authored_refusal_exercise_runs_through_the_indexed_session() {
    let temp = Temp::new();
    let root = temp.source();
    let mut manifest: ProjectFile = read(root.join("project.ron"));
    manifest.scenario = "scenarios/guard-refusal.ron".into();
    write(root.join("project.ron"), &manifest);
    let (project, mut session) = load(&temp, &root);
    for step in &project.scenario().steps {
        step.apply(&mut session).unwrap();
    }
    let with_guard = ConversationKey {
        speaker: ActorId::named("guard"),
        ..pair(REWARD)
    };
    let h = history(&mut session, &project, with_guard);
    assert_eq!((h.started, h.completed, h.interrupted), (3, 2, 1));
    assert!(given(&session, REWARDED, None));
    assert_eq!(session.state().party.experience, 120);
}

#[test]
fn final_line_completes_a_choice_free_scene_exactly_once() {
    let temp = Temp::new();
    let root = temp.source();
    let path = root.join("packages/guard/conversations/welcome-a.dialogue.ron");
    let mut graph: dialogue::Dialogue = read(&path);
    node(&mut graph, "greeting").children.clear();
    graph.repeat = dialogue::RepeatPolicy::OnceCompleted;
    write(path, &graph);
    let (project, mut session) = load(&temp, &root);
    let k = pair(A);
    start(&mut session, k);
    let view = session.conversation_view(k).unwrap();
    let advance = || Command::AdvanceLine {
        key: k,
        expected: view.token,
    };
    session.apply(advance()).unwrap();
    assert_eq!(
        session.conversation_view(k).unwrap().status,
        dialogue::RunStatus::Completed
    );
    assert_eq!(history(&mut session, &project, k).completed, 1);
    let before = session.state().clone();
    assert!(session.apply(advance()).is_err());
    assert_eq!(session.state().clone(), before);
    assert!(
        session
            .apply(Command::StartDialogue {
                dialogue: A,
                participant: HERO,
                speaker: MERCHANT,
                bindings: Default::default()
            })
            .is_err()
    );
}
