//! Standalone, deterministic authored scenario; runtime state needs no game assets.
use crate::actors::ActorTemplate;
use crate::dialogue::{Dialogue, Node, NodeKind, Repeat, RepeatPolicy, Role, ScopeSelector};
use crate::inventory::{Inventory, InventoryRole, Money, Wallet, fixtures::*};
use crate::rules::{
    Ability, Class, Grants, Modifier, Operation, Periodic, Rules, Sheet, Skill, Stat, StatKind,
    StatusEffect, Use,
};
use crate::{
    ActScope, Action, Condition, ContentManifest, GameContent, GameDefinitions, ReadScope,
    ScriptEngine, ScriptModule, ScriptName, Scripts, SessionState, Test, Value, VariableDefinition,
};
use game_types::*;
use std::collections::BTreeMap;

pub const HERO: ActorId = ActorId::named("hero");
pub const MERCHANT: ActorId = ActorId::named("merchant");
pub const COMPANION: ActorId = ActorId::named("mira");
pub const HERO_BAG: InventoryId = InventoryId::named("hero");
pub const MERCHANT_BAG: InventoryId = InventoryId::named("merchant");
pub const COMPANION_BAG: InventoryId = InventoryId::named("mira");
pub const CHEST: InventoryId = InventoryId::named("chest");
pub const PARTY_WALLET: WalletId = WalletId::named("party");
pub const MERCHANT_WALLET: WalletId = WalletId::named("merchant");
pub const REWARDED: VariableId = VariableId::named("old_gate/rewarded");
pub const GATE_DIALOGUE: DialogueId = DialogueId::named("old_gate/gate");
pub fn key(s: &str) -> Key {
    Key::new(s).unwrap()
}
fn text(s: &str) -> TextRef {
    TextRef::message(TextResourceId::named("core/text"), s).unwrap()
}

/// The fixture's formulas as a game would author them. `FixtureFormulas` computes the same
/// in Rust, so this crate's tests need no script engine.
pub const RULES_LUAU: &str = r#"
local rules = {}

function rules.derive(c: Character): { [string]: number }
    return {
        ["max-health"] = 40 + c.stats.vitality * 5 + c.level * 10,
        ["max-stamina"] = 10 + c.stats.vitality,
        attack = c.stats.strength + 2 * (c.skills.swordsmanship or 0),
    }
end

function rules.check(c: Character, skill: string, difficulty: number, roll: (sides: number) -> number): boolean
    local rank = c.skills[skill] or 0
    return rank * 2 + (c.stats.wisdom - 10) // 2 + roll(20) >= difficulty
end

return rules
"#;
/// What the fixture's abilities do. The user is the scene's player, the target its speaker.
pub const ABILITIES_LUAU: &str = r#"
local abilities = {}

function abilities.strike(game: Game, scene: Scene)
    local damage = game.stat(scene.player, "attack") + game.random(6)
    game.change_resource(scene.speaker, "health", -damage)
end

function abilities.power_strike(game: Game, scene: Scene)
    game.change_resource(scene.speaker, "health", -2 * game.stat(scene.player, "attack"))
end

function abilities.second_wind(game: Game, scene: Scene)
    game.change_resource(scene.player, "health", 25)
end

return abilities
"#;
pub struct FixtureFormulas;
impl ScriptEngine for FixtureFormulas {
    fn exports(&self, name: &ScriptName) -> bool {
        let functions: &[&str] = match name.module.as_str() {
            "rules" => &["derive", "check"],
            "abilities" => &["strike", "power_strike", "second_wind"],
            _ => &[],
        };
        functions.contains(&name.function.as_str())
    }
    fn condition(&self, name: &ScriptName, _: &ReadScope) -> crate::Result<bool> {
        Err(Invalid(format!("{name} is not a condition")).into())
    }
    fn action(&self, name: &ScriptName, scope: &mut ActScope) -> crate::Result<()> {
        let (user, target) = (scope.player, scope.speaker);
        let health = |amount| Action::ChangeResource {
            of: crate::Participant::Player,
            resource: key("health"),
            amount,
        };
        let attack = scope.read().stat(user, &key("attack"))?;
        match name.function.as_str() {
            "strike" => {
                let damage = attack + scope.random(6)? as i32;
                scope.apply(target, &health(-damage))
            }
            "power_strike" => scope.apply(target, &health(-2 * attack)),
            "second_wind" => scope.apply(user, &health(25)),
            _ => Err(Invalid(format!("{name} is not an action")).into()),
        }
    }
    fn derive(&self, _: &ScriptName, c: &Sheet) -> crate::Result<BTreeMap<String, f64>> {
        let health = 40 + c.stats[&key("vitality")] * 5 + c.level as i32 * 10;
        let swordsmanship = i32::from(c.skills.get(&key("swordsmanship")).copied().unwrap_or(0));
        let attack = c.stats[&key("strength")] + 2 * swordsmanship;
        let stamina = 10 + c.stats[&key("vitality")];
        Ok([
            ("max-health", health),
            ("max-stamina", stamina),
            ("attack", attack),
        ]
        .map(|(stat, value)| (stat.to_owned(), f64::from(value)))
        .into())
    }
    fn check(
        &self,
        _: &ScriptName,
        c: &Sheet,
        skill: &Key,
        difficulty: u32,
        roll: &mut dyn FnMut(u32) -> crate::Result<u32>,
    ) -> crate::Result<bool> {
        let rank = i64::from(c.skills.get(skill).copied().unwrap_or(0));
        let wisdom = i64::from(c.stats[&key("wisdom")] - 10).div_euclid(2);
        Ok(rank * 2 + wisdom + i64::from(roll(20)?) >= i64::from(difficulty))
    }
}

pub fn content() -> GameContent {
    let mut items = example_catalog();
    for category in &mut items.categories {
        category.name = text(&format!("category-{}", category.id));
    }
    for item in &mut items.items {
        item.name = text(&format!("item-{}", item.id));
        item.description = text(&format!("item-{}-description", item.id));
    }
    items
        .items
        .iter_mut()
        .find(|i| i.id == POTION)
        .unwrap()
        .mechanics
        .on_use = vec![Use::Restore {
        resource: key("health"),
        amount: 25,
    }];
    let sword = items.items.iter_mut().find(|i| i.id == SWORD).unwrap();
    sword.mechanics.slot = Some(key("hand"));
    sword.mechanics.modifiers = vec![Modifier {
        stat: key("strength"),
        op: Operation::Add(2),
    }];
    let primary = |minimum, maximum| StatKind::Primary { minimum, maximum };
    let derived = |minimum, maximum| StatKind::Derived { minimum, maximum };
    let rules = Rules {
        stats: [
            ("strength", primary(1, 30)),
            ("wisdom", primary(1, 30)),
            ("vitality", primary(1, 30)),
            ("max-health", derived(1, 10_000)),
            ("max-stamina", derived(0, 1000)),
            ("attack", derived(0, 1000)),
            (
                "health",
                StatKind::Resource {
                    maximum: key("max-health"),
                },
            ),
            (
                "stamina",
                StatKind::Resource {
                    maximum: key("max-stamina"),
                },
            ),
        ]
        .into_iter()
        .map(|(id, kind)| Stat {
            id: key(id),
            name: text(&format!("stat-{id}")),
            kind,
        })
        .collect(),
        skills: ["persuasion", "swordsmanship"]
            .into_iter()
            .map(|id| Skill {
                id: key(id),
                name: text(&format!("skill-{id}")),
                ranks: vec![1, 2, 3],
            })
            .collect(),
        slots: [key("hand")].into(),
        effects: vec![
            StatusEffect {
                id: key("fortified"),
                name: text("effect-fortified"),
                modifiers: vec![Modifier {
                    stat: key("strength"),
                    op: Operation::Add(3),
                }],
                periodic: None,
            },
            StatusEffect {
                id: key("poisoned"),
                name: text("effect-poisoned"),
                modifiers: vec![],
                periodic: Some(Periodic {
                    resource: key("health"),
                    amount: -5,
                    period_ms: 1000,
                }),
            },
        ],
        classes: [
            (
                "adventurer",
                [("persuasion", 3), ("swordsmanship", 2)].as_slice(),
                ["strike", "power-strike", "second-wind"].as_slice(),
            ),
            (
                "scholar",
                [("persuasion", 3)].as_slice(),
                ["second-wind"].as_slice(),
            ),
        ]
        .into_iter()
        .map(|(id, skills, abilities)| Class {
            id: key(id),
            name: text(&format!("class-{id}")),
            starting: ["strength", "wisdom", "vitality"]
                .map(|stat| (key(stat), 10))
                .into(),
            per_level: Grants {
                attribute_points: 2,
                learning_points: 3,
                ..Default::default()
            },
            at_level: [
                (
                    1,
                    Grants {
                        abilities: abilities.iter().map(|id| key(id)).collect(),
                        ..Default::default()
                    },
                ),
                (
                    3,
                    Grants {
                        bonuses: [(key("vitality"), 1)].into(),
                        ..Default::default()
                    },
                ),
            ]
            .into(),
            skills: skills
                .iter()
                .map(|(skill, rank)| (key(skill), *rank))
                .collect(),
        })
        .collect(),
        abilities: [
            ("strike", 1500, 0, None, true, "strike"),
            ("power-strike", 1500, 6000, Some(10), true, "power_strike"),
            ("second-wind", 500, 30_000, Some(5), false, "second_wind"),
        ]
        .into_iter()
        .map(
            |(id, duration_ms, cooldown_ms, stamina, targeted, function)| Ability {
                id: key(id),
                name: text(&format!("ability-{id}")),
                duration_ms,
                cooldown_ms,
                costs: stamina
                    .map(|cost| (key("stamina"), cost))
                    .into_iter()
                    .collect(),
                targeted,
                resolve: ScriptName::try_from(format!("abilities.{function}")).unwrap(),
            },
        )
        .collect(),
        levels: vec![100, 300, 600],
        life: key("health"),
        party_size: 4,
        derive: ScriptName::try_from("rules.derive".to_owned()).unwrap(),
        check: ScriptName::try_from("rules.check".to_owned()).unwrap(),
    };
    let template = ActorTemplate {
        interaction: None,
        id: ActorTemplateId::named("traveller"),
        name: text("actor-traveller"),
        class: key("adventurer"),
        level: 1,
        base: Default::default(),
        skills: Default::default(),
        equipment: vec![],
        loot: None,
    };
    let graph = Dialogue {
        id: GATE_DIALOGUE,
        roles: [
            (key("player"), Role::Required),
            (key("speaker"), Role::Required),
        ]
        .into(),
        history_scope: ScopeSelector::Interaction,
        repeat: RepeatPolicy::OnceCompleted,
        mode: Default::default(),
        start: vec![key("greeting")],
        nodes: vec![
            Node {
                id: key("greeting"),
                kind: NodeKind::Line,
                speaker: key("speaker"),
                text: text("dialogue-gate"),
                arguments: Default::default(),
                repeat: Repeat::Always,
                condition: None,
                actions: vec![],
                children: vec![key("return-key")],
            },
            Node {
                id: key("return-key"),
                kind: NodeKind::Choice,
                speaker: key("player"),
                text: text("dialogue-return-key"),
                arguments: Default::default(),
                repeat: Repeat::OnceEver,
                condition: Some(Condition::All(vec![
                    Condition::HasItem {
                        definition: KEY,
                        quantity: 1,
                    },
                    Condition::Variable {
                        variable: REWARDED,
                        of: None,
                        test: Test::Is(Value::Bool(false)),
                    },
                ])),
                actions: vec![
                    Action::ConsumeItem {
                        definition: KEY,
                        quantity: 1,
                    },
                    Action::Check {
                        skill: key("persuasion"),
                        difficulty: 1,
                        success: vec![
                            Action::GrantItem {
                                definition: SWORD,
                                quantity: 1,
                            },
                            Action::AwardExperience { amount: 10 },
                            Action::Set {
                                variable: REWARDED,
                                of: None,
                                value: Value::Bool(true),
                            },
                        ],
                        failure: vec![],
                    },
                ],
                children: vec![],
            },
        ],
    };
    let mut content = GameContent {
        text: vec![],
        manifest: ContentManifest {
            id: ContentId([1; 16]),
            revision: 1,
            world_generation: "standalone-world-v1".into(),
        },
        items,
        scripts: Scripts::new(std::rc::Rc::new(FixtureFormulas)),
        graphs: Default::default(),
        game: GameDefinitions {
            world: Default::default(),
            dialogue_contracts: vec![],
            claims: vec![],
            quests: vec![],
            profiles: vec![],
            predicates: vec![],
            rules,
            actors: vec![template],
            dialogues: vec![graph],
            variables: vec![VariableDefinition {
                id: REWARDED,
                initial: Value::Bool(false),
                scope: Default::default(),
            }],
            scripts: [("abilities", ABILITIES_LUAU), ("rules", RULES_LUAU)]
                .map(|(name, source)| ScriptModule {
                    name: key(name),
                    source: source.into(),
                })
                .into(),
            loot: vec![],
        },
    };
    content.game.dialogue_contracts = content
        .game
        .dialogues
        .iter()
        .map(Dialogue::contract)
        .collect();
    content.text.push(TextContract {
        id: TextResourceId::named("core/text"),
        imports: Default::default(),
        messages: content
            .text_keys()
            .iter()
            .map(|m| (m.key.clone(), MessageContract::default()))
            .collect(),
    });
    content.sort();
    content
}
pub fn state() -> SessionState {
    let content = content();
    let mut state = SessionState::empty(42);
    for (id, bag) in [
        (HERO, HERO_BAG),
        (MERCHANT, MERCHANT_BAG),
        (COMPANION, COMPANION_BAG),
    ] {
        let actor = state
            .spawn(&content, content.game.actors[0].id, id)
            .unwrap();
        if id == HERO {
            actor.resources.insert(key("health"), 50);
        } else if id == MERCHANT {
            actor.position.millimetres = [100000000, 0, 100000000];
        }
        let mut inventory = Inventory::new(OwnerRef::actor(id), InventoryRole::Carried);
        inventory.id = bag;
        if id == HERO {
            inventory.grant(&content.items, POTION, 3).unwrap();
            inventory.grant(&content.items, SWORD, 1).unwrap();
            inventory.grant(&content.items, KEY, 1).unwrap();
        }
        if id == MERCHANT {
            inventory.grant(&content.items, POTION, 10).unwrap();
        }
        state.add_inventory(inventory);
    }
    // The hero travels alone at first, with the party's purse.
    state.party.members.insert(HERO);
    state.party.controlled = Some(HERO);
    state.party.wallet = Some(PARTY_WALLET);
    let owner = |kind| OwnerRef {
        kind,
        id: OwnerId([1; 16]),
    };
    let (chest, party) = (owner(OwnerKind::Object), owner(OwnerKind::Party));
    state.owners.extend([chest, party]);
    let mut inventory = Inventory::new(chest, InventoryRole::Contents);
    inventory.id = CHEST;
    state.add_inventory(inventory);
    for (id, owner) in [
        (PARTY_WALLET, party),
        (MERCHANT_WALLET, OwnerRef::actor(MERCHANT)),
    ] {
        let mut wallet = Wallet::new(owner);
        wallet.id = id;
        wallet.credit(Money::new(100).unwrap()).unwrap();
        state.add_wallet(wallet);
    }
    state.validate(&content).unwrap();
    state
}

/// The persuasion check behind the gate conversation's only choice.
pub fn persuade(content: &mut GameContent) -> &mut Action {
    &mut content.game.dialogues[0].nodes[1].actions[1]
}
