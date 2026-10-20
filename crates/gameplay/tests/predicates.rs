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
            .core
            .game
            .predicates
            .add(NamedPredicate { id, condition });
        condition = Condition::Named(id);
    }

    assert!(
        content
            .core
            .validate_condition(&condition)
            .unwrap_err()
            .to_string()
            .contains("complexity")
    );
}

#[test]
fn a_group_stops_at_the_child_that_decides_it_and_changes_nothing() {
    let content = content();
    let state = state();
    let before = state.clone();
    let holds = Condition::Variable {
        variable: REWARDED,
        of: None,
        test: yarra_gameplay::Test::Is(yarra_gameplay::Value::Bool(false)),
    };

    // A child after the deciding one is not evaluated: here it would be an error.
    let unknown = Condition::Named(PredicateId([9; 16]));
    let evaluate = |condition: &Condition| content.core.evaluate(condition, &state, HERO, MERCHANT);
    assert!(evaluate(&Condition::Any(vec![holds.clone(), unknown.clone()])).unwrap());
    let fails = Condition::Not(Box::new(holds.clone()));
    assert!(!evaluate(&Condition::All(vec![fails, unknown.clone()])).unwrap());
    assert!(evaluate(&Condition::All(vec![holds, unknown])).is_err());
    assert_eq!(state, before);
    assert!(
        content
            .core
            .validate_condition(&Condition::Any(vec![]))
            .is_err()
    );
}
