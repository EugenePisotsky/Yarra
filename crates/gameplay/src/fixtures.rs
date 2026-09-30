//! Standalone, deterministic authored scenario; runtime state needs no game assets.
use crate::actors::{Actor, ActorRole, ActorTemplate};
use crate::dialogue::{Choice, ChoiceRepeat, Dialogue, Line, Node, RepeatPolicy, ScopeSelector};
use crate::inventory::{Inventory, Money, Wallet, fixtures::*};
use crate::rules::{Attribute, Effect, Modifier, Rules, Skill};
use crate::{Action, Condition, ContentManifest, GameContent, GameDefinitions, SessionState};
use game_types::*;

pub const HERO: ActorId = ActorId([1; 16]);
pub const MERCHANT: ActorId = ActorId([2; 16]);
pub const COMPANION: ActorId = ActorId([3; 16]);
pub const HERO_BAG: InventoryId = InventoryId([1; 16]);
pub const MERCHANT_BAG: InventoryId = InventoryId([2; 16]);
pub const COMPANION_BAG: InventoryId = InventoryId([3; 16]);
pub const CHEST: InventoryId = InventoryId([4; 16]);
pub const PARTY_WALLET: WalletId = WalletId([1; 16]);
pub const MERCHANT_WALLET: WalletId = WalletId([2; 16]);
pub const GATE_DIALOGUE: DialogueId = DialogueId([1; 16]);
pub fn key(s: &str) -> Key {
    Key::new(s).unwrap()
}
fn text(s: &str) -> TextRef {
    TextRef::message(TextResourceId([8; 16]), s).unwrap()
}

pub fn content() -> GameContent {
    let mut items = example_catalog();
    for category in &mut items.categories {
        category.name = text(&format!("category-{}", category.key));
    }
    for item in &mut items.items {
        item.name = text(&format!("item-{}", item.key));
        item.description = text(&format!("item-{}-description", item.key));
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
        id: ActorTemplateId([1; 16]),
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
        roles: [key("player"), key("speaker")].into(),
        history_scope: ScopeSelector::Interaction,
        repeat: RepeatPolicy::OnceCompleted,
        start: key("gate"),
        nodes: vec![Node {
            id: key("gate"),
            lines: vec![Line {
                id: key("greeting"),
                speaker: key("speaker"),
                text: text("dialogue-gate"),
                arguments: Default::default(),
            }],
            choices: vec![Choice {
                id: key("return-key"),
                text: text("dialogue-return-key"),
                arguments: Default::default(),
                repeat: ChoiceRepeat::OnceEver,
                conditions: vec![key("has-key"), key("not-rewarded")],
                actions: vec![key("consume-key"), key("persuade")],
                next: None,
            }],
        }],
    };
    let mut content = GameContent {
        text: vec![],
        manifest: ContentManifest {
            id: ContentId([1; 16]),
            revision: 1,
            world_generation: "standalone-world-v1".into(),
        },
        items,
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
            facts: [key("gate-rewarded")].into(),
            conditions: [
                (
                    binding("has-key"),
                    Condition::HasItem {
                        definition: KEY,
                        quantity: 1,
                    },
                ),
                (
                    binding("not-rewarded"),
                    Condition::Fact {
                        key: key("gate-rewarded"),
                        value: false,
                    },
                ),
            ]
            .into(),
            actions: [
                (
                    binding("consume-key"),
                    Action::ConsumeItem {
                        definition: KEY,
                        quantity: 1,
                    },
                ),
                (
                    binding("persuade"),
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
                            Action::SetFact {
                                key: key("gate-rewarded"),
                                value: true,
                            },
                        ],
                        failure: vec![],
                    },
                ),
            ]
            .into(),
        },
    };
    content.game.dialogue_contracts = content
        .game
        .dialogues
        .iter()
        .map(Dialogue::contract)
        .collect();
    content.text.push(TextContract {
        id: TextResourceId([8; 16]),
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
        state.actors.push(actor);
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
        state.inventories.push(inventory);
    }
    let chest = OwnerRef::new("chest", OwnerId([1; 16])).unwrap();
    let party = OwnerRef::new("party", OwnerId([1; 16])).unwrap();
    state.owners.extend([chest.clone(), party.clone()]);
    let mut inventory = Inventory::new(chest, "contents").unwrap();
    inventory.id = CHEST;
    state.inventories.push(inventory);
    for (id, owner) in [
        (PARTY_WALLET, party),
        (MERCHANT_WALLET, OwnerRef::actor(MERCHANT)),
    ] {
        let mut wallet = Wallet::new(owner).unwrap();
        wallet.id = id;
        wallet.credit(Money::new(100).unwrap()).unwrap();
        state.wallets.push(wallet);
    }
    state.validate(&content).unwrap();
    state
}

pub fn binding(name: &str) -> BindingId {
    BindingId::new(GATE_DIALOGUE, key(name))
}
