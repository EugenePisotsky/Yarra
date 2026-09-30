use game_types::VariableId;
use gameplay::dialogue::HistoryEvent;
use gameplay::inventory::fixtures::{KEY, SWORD};
use gameplay::{
    Action, Command, Condition, ConversationKey, GameContent, GameEvent, GameSession, ScriptModule,
    ScriptName, ToolContent, Value, VariableDefinition, fixtures::*,
};
use yarra_scripting::{API_DEFINITIONS, LuauScripts};

const GATE: &str = r#"
local gate = {}

function gate.has_key(game: Game, scene: Scene): boolean
    return game.item_count(scene.player, "old_gate_key") >= 1
        and game.get("old_gate/rewarded") == false
end

function gate.hand_over(game: Game, scene: Scene)
    game.consume_item(scene.player, "old_gate_key", 1)
    -- A script sees its own changes straight away.
    assert(game.item_count(scene.player, "old_gate_key") == 0)
    if game.roll(scene.player, "persuasion", 1) then
        game.give_item(scene.player, "iron_sword", 1)
        game.award_experience(scene.player, "persuasion", 10)
        game.adjust_relationship(scene.speaker, scene.player, 25)
        game.set("old_gate/rewarded", true)
        game.add("old_gate/visits", 2)
        game.set("old_gate/password", "mellon")
        -- What this particular NPC remembers.
        assert(game.get("old_gate/thanked", scene.speaker) == false)
        game.set("old_gate/thanked", true, scene.speaker)
        game.add("old_gate/favours", 3, scene.speaker)
    end
end

function gate.greedy(game: Game, scene: Scene)
    game.set("old_gate/rewarded", true)
    game.give_item(scene.player, "iron_sword", 1)
    game.consume_item(scene.player, "old_gate_key", 5)
end

return gate
"#;

fn script(name: &str) -> ScriptName {
    ScriptName::try_from(name.to_owned()).unwrap()
}
/// The fixture conversation, with its choice decided and carried out by scripts.
fn content(source: &str, action: &str) -> GameContent {
    let mut content = gameplay::fixtures::content();
    for (name, initial) in [
        ("old_gate/visits", Value::Int(0)),
        ("old_gate/password", Value::Text(String::new())),
    ] {
        content.game.variables.push(VariableDefinition {
            id: VariableId::try_from(name.to_owned()).unwrap(),
            initial,
            scope: Default::default(),
        });
    }
    for (name, initial) in [
        ("old_gate/thanked", Value::Bool(false)),
        ("old_gate/favours", Value::Int(0)),
    ] {
        content.game.variables.push(VariableDefinition {
            id: VariableId::try_from(name.to_owned()).unwrap(),
            initial,
            scope: gameplay::VariableScope::Actor,
        });
    }
    content.game.scripts = vec![ScriptModule {
        name: key("gate"),
        source: source.into(),
    }];
    content.scripts = LuauScripts::install(&content.game.scripts).unwrap();
    let choice = &mut content.game.dialogues[0].nodes[1];
    choice.condition = Some(Condition::Script(script("gate.has_key")));
    choice.actions = vec![Action::Script(script(action))];
    content.validate().unwrap();
    content
}
const TALK: ConversationKey = ConversationKey {
    dialogue: GATE_DIALOGUE,
    participant: HERO,
    speaker: MERCHANT,
};
fn at_choice(content: GameContent) -> GameSession<ToolContent> {
    let mut session = GameSession::new(ToolContent::new(content).unwrap(), state()).unwrap();
    session
        .apply(Command::StartDialogue {
            bindings: Default::default(),
            dialogue: GATE_DIALOGUE,
            participant: HERO,
            speaker: MERCHANT,
        })
        .unwrap();
    let expected = session.conversation_view(TALK).unwrap().token;
    session
        .apply(Command::AdvanceLine {
            key: TALK,
            expected,
        })
        .unwrap();
    session
}
fn choose(session: &mut GameSession<ToolContent>) -> gameplay::Result<gameplay::CommandOutcome> {
    let expected = session.conversation_view(TALK)?.token;
    session.apply(Command::Choose {
        expected,
        dialogue: GATE_DIALOGUE,
        participant: HERO,
        speaker: MERCHANT,
        choice: key("return-key"),
    })
}
fn keys(session: &GameSession<ToolContent>) -> usize {
    let bag = session.state().carried(HERO).unwrap();
    bag.entries.iter().filter(|e| e.definition == KEY).count()
}

#[test]
fn a_scripted_choice_reads_state_and_changes_it_through_the_same_rules() {
    let mut session = at_choice(content(GATE, "gate.hand_over"));
    assert_eq!(
        session
            .available_choices(GATE_DIALOGUE, HERO, MERCHANT)
            .unwrap(),
        [key("return-key")]
    );
    let outcome = choose(&mut session).unwrap();
    assert!(
        outcome
            .events
            .iter()
            .any(|e| matches!(e, GameEvent::SkillChecked { passed: true, .. }))
    );
    let state = session.state();
    assert_eq!(keys(&session), 0);
    let content = session.content();
    let variable = |name: &str| state.variable(content, VariableId::named(name)).unwrap();
    assert_eq!(variable("old_gate/rewarded"), Value::Bool(true));
    assert_eq!(variable("old_gate/visits"), Value::Int(2));
    assert_eq!(variable("old_gate/password"), Value::Text("mellon".into()));
    let remembered = |name: &str, actor| {
        let key = gameplay::VariableKey {
            variable: VariableId::named(name),
            actor: Some(actor),
        };
        state.variable(content, key).unwrap()
    };
    assert_eq!(remembered("old_gate/thanked", MERCHANT), Value::Bool(true));
    assert_eq!(remembered("old_gate/favours", MERCHANT), Value::Int(3));
    assert_eq!(remembered("old_gate/thanked", HERO), Value::Bool(false));
    assert_eq!(state.actor(HERO).unwrap().skills[&key("persuasion")], 10);
    let attitude = state.relationship(gameplay::actors::RelationshipKey {
        from: MERCHANT,
        to: HERO,
    });
    assert_eq!(attitude.attitude, 25);
    let swords = state.carried(HERO).unwrap();
    assert_eq!(
        swords
            .entries
            .iter()
            .filter(|e| e.definition == SWORD)
            .count(),
        2
    );
    assert_eq!(
        state
            .histories
            .values()
            .next()
            .unwrap()
            .count(&HistoryEvent::Node(key("return-key"))),
        1
    );
}
#[test]
fn a_script_that_fails_part_way_leaves_nothing_behind_and_says_where() {
    let mut session = at_choice(content(GATE, "gate.greedy"));
    let before = session.state().clone();
    let error = choose(&mut session).unwrap_err().to_string();
    assert!(error.contains("script gate.greedy"), "{error}");
    assert!(
        error.contains("not enough items") && error.contains("in function 'greedy'"),
        "{error}"
    );
    assert_eq!(session.state(), &before);
    assert_eq!(keys(&session), 1);
}
#[test]
fn conditions_are_read_only_strict_and_bounded() {
    let cases = [
        ("return game.give_item ~= nil", "true or false"),
        (
            "game.give_item(scene.player, 'iron_sword', 1) return true",
            "nil value",
        ),
        ("return 1", "true or false"),
        ("while true do end return true", "step budget"),
        (
            "return game.item_count(scene.player, 'no_such_item') > 0",
            "not found",
        ),
        (
            "return game.item_count('Bad Name', 'iron_sword') > 0",
            "invalid name",
        ),
        ("return math.random() > 0.5", "nil value"),
        ("return os.time() > 0", "nil value"),
        (
            "return game.get('old_gate/missing') == true",
            "unknown variable",
        ),
    ];
    for (body, expected) in cases {
        let source = format!(
            "local gate = {{}}\nfunction gate.has_key(game, scene)\n{body}\nend\n\
             function gate.hand_over(game, scene) end\nreturn gate"
        );
        let content = content(&source, "gate.hand_over");
        let state = state();
        let before = state.clone();
        let result = content.evaluate(
            &Condition::Script(script("gate.has_key")),
            &state,
            HERO,
            MERCHANT,
        );
        match result {
            // The first case is a plain false: effect functions do not exist for conditions.
            Ok(evaluation) => assert!(!evaluation.matched && body.contains("~= nil"), "{body}"),
            Err(error) => {
                let error = error.to_string();
                assert!(error.contains(expected), "{body}: {error}");
            }
        }
        assert_eq!(state, before);
    }
}
#[test]
fn nothing_a_script_keeps_survives_to_the_next_call() {
    let source = r#"
        local calls = 0
        local gate = {}
        function gate.has_key(game, scene)
            calls += 1
            return calls == 1
        end
        function gate.hand_over(game, scene) end
        return gate
    "#;
    let content = content(source, "gate.hand_over");
    let state = state();
    for _ in 0..3 {
        let condition = Condition::Script(script("gate.has_key"));
        assert!(
            content
                .evaluate(&condition, &state, HERO, MERCHANT)
                .unwrap()
                .matched
        );
    }
}
#[test]
fn publication_rejects_broken_modules_and_missing_functions() {
    let module = |source: &str| {
        vec![ScriptModule {
            name: key("gate"),
            source: source.into(),
        }]
    };
    let error = |source: &str| LuauScripts::new(&module(source)).err().unwrap();
    assert!(error("local gate = {").contains("script gate"));
    assert!(error("return 5").contains("table of functions"));
    assert!(error("return { ready = true }").contains("not a function"));
    let mut content = content(GATE, "gate.hand_over");
    content.game.dialogues[0].nodes[1].actions = vec![Action::Script(script("gate.missing"))];
    assert!(
        content
            .validate()
            .unwrap_err()
            .to_string()
            .contains("unknown script gate.missing")
    );
    content.scripts = Default::default();
    assert!(
        content
            .validate()
            .unwrap_err()
            .to_string()
            .contains("no script engine")
    );
    assert!(ScriptName::try_from("no-dot".to_owned()).is_err());
}
#[test]
fn the_type_definitions_list_exactly_the_functions_scripts_are_given() {
    // Ask a script what it was handed, for a condition and for an action.
    let source = r#"
        local gate = {}
        local function names(game)
            local list = {}
            for name in game do table.insert(list, name) end
            table.sort(list)
            return table.concat(list, ",")
        end
        function gate.has_key(game, scene) return true end
        function gate.read_names(game, scene) error("reads=" .. names(game)) end
        function gate.hand_over(game, scene) error("all=" .. names(game)) end
        return gate
    "#;
    let content = content(source, "gate.hand_over");
    let listed = |error: String, tag: &str| -> Vec<String> {
        let start = error.find(tag).unwrap() + tag.len();
        let list = error[start..].split_whitespace().next().unwrap();
        list.split(',').map(str::to_owned).collect()
    };
    let reads = listed(
        content
            .evaluate(
                &Condition::Script(script("gate.read_names")),
                &state(),
                HERO,
                MERCHANT,
            )
            .unwrap_err()
            .to_string(),
        "reads=",
    );
    let mut session = at_choice(content);
    let all = listed(choose(&mut session).unwrap_err().to_string(), "all=");
    let declared = |block: &str| -> Vec<String> {
        let start = API_DEFINITIONS.find(block).unwrap();
        let body = &API_DEFINITIONS[start..];
        let body = &body[..body.find("\n}").unwrap()];
        let mut names: Vec<String> = body
            .lines()
            .filter_map(|l| l.trim().split_once(": (").map(|(name, _)| name.to_owned()))
            .collect();
        names.sort();
        names
    };
    assert_eq!(reads, declared("type Reads = {"));
    let mut everything = declared("type Reads = {");
    everything.extend(declared("type Game = Reads & {"));
    everything.sort();
    assert_eq!(all, everything);
}

#[test]
fn variables_keep_their_type() {
    let cases = [
        ("game.set('old_gate/rewarded', 1)", "different type"),
        ("game.set('old_gate/visits', 1.5)", "whole number"),
        ("game.set('old_gate/visits', {})", "whole number"),
        ("game.add('old_gate/rewarded', 1)", "whole-number"),
        ("game.set('old_gate/nothing', true)", "unknown variable"),
        ("game.set('old_gate/thanked', true)", "per actor"),
        (
            "game.set('old_gate/rewarded', true, scene.speaker)",
            "per actor",
        ),
        ("game.get('old_gate/thanked')", "scope mismatch"),
    ];
    for (body, expected) in cases {
        let source = format!(
            "local gate = {{}}\nfunction gate.has_key(game, scene) return true end\n\
             function gate.hand_over(game, scene)\n{body}\nend\nreturn gate"
        );
        let mut session = at_choice(content(&source, "gate.hand_over"));
        let before = session.state().clone();
        let error = choose(&mut session).unwrap_err().to_string();
        assert!(error.contains(expected), "{body}: {error}");
        assert_eq!(session.state(), &before);
    }
}

#[test]
fn a_script_sends_an_actor_walking_and_queues_a_conversation() {
    use gameplay::{Movement, Pending};
    const POST: game_types::AreaId = game_types::AreaId::named("old_gate/post");
    let source = r#"
local gate = {}
function gate.has_key(game: Game, scene: Scene): boolean return true end
function gate.hand_over(game: Game, scene: Scene)
    game.move_to(scene.speaker, "old_gate/post", 5000)
    game.start_dialogue("old_gate/gate")
end
return gate
"#;
    let mut content = content(source, "gate.hand_over");
    content.game.world.areas.insert(POST);
    content.validate().unwrap();
    let mut session = at_choice(content);
    choose(&mut session).unwrap();
    let world = &session.state().world;
    assert!(matches!(
        world.movements.get(&MERCHANT),
        Some(Movement {
            to: POST,
            deadline: Some(_),
            ..
        })
    ));
    assert!(world.pending.contains(&Pending::Start {
        dialogue: GATE_DIALOGUE,
        participant: HERO,
        speaker: MERCHANT,
    }));

    // An area nobody declared is an error in the script, and the choice leaves nothing behind.
    let source = source.replace("old_gate/post", "old_gate/nowhere");
    let mut session = at_choice(self::content(&source, "gate.hand_over"));
    let before = session.state().clone();
    let error = choose(&mut session).unwrap_err().to_string();
    assert!(error.contains("script gate.hand_over"), "{error}");
    assert_eq!(session.state(), &before);
}
