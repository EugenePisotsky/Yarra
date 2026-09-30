//! Standalone, deterministic authored scenario; runtime state needs no game assets.
use crate::actors::{Actor, ActorRole, ActorTemplate};
use crate::dialogue::{Dialogue, Node, NodeKind, Repeat, RepeatPolicy, Role, ScopeSelector};
use crate::inventory::{Inventory, Money, Wallet, fixtures::*};
use crate::rules::{Attribute, Effect, Modifier, Rules, Skill};
use crate::{
    Action, Condition, ContentManifest, GameContent, GameDefinitions, SessionState, Test, Value,
    VariableDefinition,
};
use game_types::*;

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
        .on_use = vec![Effect::Heal(25)];
    let sword = items.items.iter_mut().find(|i| i.id == SWORD).unwrap();
    sword.mechanics.slot = Some(key("hand"));
    sword.mechanics.modifiers = vec![Modifier {
        attribute: key("strength"),
        amount: 2,
    }];
    let rules = Rules {
        attributes: [
            ("max-health", 1, 1000),
            ("strength", 1, 100),
            ("wisdom", 1, 100),
        ]
        .into_iter()
        .map(|(id, minimum, maximum)| Attribute {
            id: key(id),
            name: text(&format!("attribute-{id}")),
            minimum,
            maximum,
        })
        .collect(),
        skills: ["persuasion", "swordsmanship"]
            .into_iter()
            .map(|id| Skill {
                id: key(id),
                name: text(&format!("skill-{id}")),
            })
            .collect(),
        slots: [key("hand")].into(),
        health_attribute: key("max-health"),
    };
    let template = ActorTemplate {
        interaction: None,
        id: ActorTemplateId::named("traveller"),
        name: text("actor-traveller"),
        base: [
            (key("max-health"), 100),
            (key("strength"), 10),
            (key("wisdom"), 10),
        ]
        .into(),
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
                        test: Test::Is(Value::Bool(false)),
                    },
                ])),
                actions: vec![
                    Action::ConsumeItem {
                        definition: KEY,
                        quantity: 1,
                    },
                    Action::SkillCheck {
                        skill: key("persuasion"),
                        difficulty: 1,
                        success: vec![
                            Action::GrantItem {
                                definition: SWORD,
                                quantity: 1,
                            },
                            Action::AwardExperience {
                                skill: key("persuasion"),
                                amount: 10,
                            },
                            Action::Set {
                                variable: REWARDED,
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
        scripts: Default::default(),
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
            }],
            scripts: vec![],
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
    content
}
pub fn state() -> SessionState {
    let content = content();
    let mut state = SessionState::empty(42);
    for (id, bag, role) in [
        (HERO, HERO_BAG, ActorRole::Player),
        (MERCHANT, MERCHANT_BAG, ActorRole::Npc),
        (COMPANION, COMPANION_BAG, ActorRole::Companion),
    ] {
        let mut actor =
            Actor::from_template(&content.game.actors[0], &content.game.rules, role).unwrap();
        actor.id = id;
        if id == HERO {
            actor.health = 50;
        } else if id == MERCHANT {
            actor.position.millimetres = [100000000, 0, 100000000];
        }
        state.add_actor(actor);
        let mut inventory = Inventory::new(OwnerRef::actor(id), "carried").unwrap();
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
    let chest = OwnerRef::new("chest", OwnerId([1; 16])).unwrap();
    let party = OwnerRef::new("party", OwnerId([1; 16])).unwrap();
    state.owners.extend([chest.clone(), party.clone()]);
    let mut inventory = Inventory::new(chest, "contents").unwrap();
    inventory.id = CHEST;
    state.add_inventory(inventory);
    for (id, owner) in [
        (PARTY_WALLET, party),
        (MERCHANT_WALLET, OwnerRef::actor(MERCHANT)),
    ] {
        let mut wallet = Wallet::new(owner).unwrap();
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
