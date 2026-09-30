use game_types::PredicateId;
use yarra_gameplay::inventory;
use yarra_gameplay::{Condition, KeyedMap, NamedPredicate, fixtures::*};

#[test]
fn resolved_depth_is_checked_before_a_valid_local_tree_reaches_runtime() {
    let mut content = content();
    let mut condition = Condition::HasItem {
        definition: inventory::fixtures::KEY,
        quantity: 1,
    };
    for i in 0..3 {
        // Each asset is below the local depth limit; their expansion exceeds evaluation's limit.
        for _ in 0..15 {
            condition = Condition::Not(Box::new(condition));
        }
        let id = PredicateId([i; 16]);
        content
            .game
            .predicates
            .add(NamedPredicate { id, condition });
        condition = Condition::Named(id);
    }
    assert!(
        content
            .validate_condition(&condition)
            .unwrap_err()
            .to_string()
            .contains("complexity")
    );
}

#[test]
fn composite_diagnostics_include_all_branches_without_changing_state() {
    let content = content();
    let state = state();
    let before = state.clone();
    let condition = Condition::Any(vec![
        Condition::HasItem {
            definition: inventory::fixtures::KEY,
            quantity: 2,
        },
        Condition::Not(Box::new(Condition::Variable {
            variable: REWARDED,
            of: None,
            test: yarra_gameplay::Test::Is(yarra_gameplay::Value::Bool(true)),
        })),
    ]);
    content.validate_condition(&condition).unwrap();
    let result = content
        .evaluate(&condition, &state, HERO, MERCHANT)
        .unwrap();
    assert!(result.matched);
    assert_eq!(result.checks.len(), 2);
    assert!(result.checks.iter().all(|c| !c.matched));
    assert_eq!(state, before);
    assert!(content.validate_condition(&Condition::Any(vec![])).is_err());
}
