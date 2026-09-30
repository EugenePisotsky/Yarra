use game_types::*;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use yarra_localization::*;
const A: TextResourceId = TextResourceId([8; 16]);
const B: TextResourceId = TextResourceId([9; 16]);
fn key(s: &str) -> TextKey {
    TextKey::new(s).unwrap()
}
fn contract(id: TextResourceId, messages: &[(&str, &[(&str, ArgumentType)])]) -> TextContract {
    TextContract {
        id,
        imports: Default::default(),
        messages: messages
            .iter()
            .map(|(k, args)| {
                (
                    key(k),
                    MessageContract {
                        arguments: args
                            .iter()
                            .map(|(n, t)| (n.to_string(), t.clone()))
                            .collect(),
                    },
                )
            })
            .collect(),
    }
}
fn resource(c: &TextContract, locale: &str, source: &str) -> LanguageResource {
    LanguageResource {
        id: c.id,
        locale: locale.into(),
        source: source.into(),
        contract_hash: contract_hash(c).unwrap(),
        reviewed: Default::default(),
    }
}
fn message(id: TextResourceId, k: &str) -> TextRef {
    TextRef::message(id, k).unwrap()
}
#[test]
fn identical_local_keys_are_isolated_and_locales_fall_back() {
    let a = contract(A, &[("greeting", &[])]);
    let b = contract(B, &[("greeting", &[])]);
    let service = Localization::new(
        "en",
        vec![a.clone(), b.clone()],
        vec![
            resource(&a, "en", "greeting = First"),
            resource(&b, "en", "greeting = Second"),
            resource(&b, "uk", "greeting = Другий"),
        ],
    )
    .unwrap();
    assert_eq!(
        service
            .format("uk", &message(A, "greeting"), &Arguments::new())
            .unwrap()
            .value,
        "First"
    );
    let result = service
        .format("uk-UA", &message(B, "greeting"), &Arguments::new())
        .unwrap();
    assert_eq!(result.value, "Другий");
    assert!(result.used_fallback);
    assert_eq!(
        service
            .format("en", &message(B, "greeting"), &Arguments::new())
            .unwrap()
            .value,
        "Second"
    );
    assert_eq!(
        service
            .format("en", &TextRef::Literal("My name".into()), &Arguments::new())
            .unwrap()
            .locale,
        None
    );
}
#[test]
fn typed_arguments_validate_every_select_branch_and_cldr_plurals() {
    let c = contract(
        A,
        &[
            ("count", &[("n", ArgumentType::Number)]),
            ("welcome", &[("name", ArgumentType::Text)]),
        ],
    );
    let source =
        "count = { $n ->\n [one] One item\n *[other] { $n } items\n}\nwelcome = Welcome { $name }";
    let uk = "count = { $n ->\n [one] предмет\n [few] предмети\n *[many] предметів\n}";
    let service = Localization::new(
        "en",
        vec![c.clone()],
        vec![resource(&c, "en", source), resource(&c, "uk", uk)],
    )
    .unwrap();
    for (n, word) in [
        (1, "предмет"),
        (2, "предмети"),
        (5, "предметів"),
        (21, "предмет"),
    ] {
        assert_eq!(
            service
                .format(
                    "uk",
                    &message(A, "count"),
                    &[("n".into(), Argument::Number(n))].into()
                )
                .unwrap()
                .value,
            word
        );
    }
    assert!(
        service
            .format(
                "en",
                &message(A, "count"),
                &[("n".into(), Argument::Text("1".into()))].into()
            )
            .is_err()
    );
    assert!(
        service
            .format("en", &message(A, "welcome"), &Arguments::new())
            .is_err()
    );
    for bad in [
        source.replace("One item", "{ $undeclared }"),
        source.replace("[one]", "[banana]"),
        source.replace("One item", "{ UNSUPPORTED() }"),
    ] {
        let service =
            Localization::new("en", vec![c.clone()], vec![resource(&c, "en", &bad)]).unwrap();
        assert!(
            service
                .scope(A, "en")
                .unwrap()
                .validate(&key("count"))
                .is_err()
        );
    }
}
#[test]
fn enum_selectors_reject_unknown_values_and_type_changes() {
    let c = contract(
        A,
        &[(
            "greeting",
            &[(
                "attitude",
                ArgumentType::Select(["friendly".into(), "hostile".into()].into()),
            )],
        )],
    );
    let service = Localization::new(
        "en",
        vec![c.clone()],
        vec![resource(
            &c,
            "en",
            "greeting = { $attitude ->\n [friendly] Hello\n *[hostile] Go away\n}",
        )],
    )
    .unwrap();
    assert!(
        service
            .scope(A, "en")
            .unwrap()
            .validate(&key("greeting"))
            .is_ok()
    );
    assert!(
        service
            .format(
                "en",
                &message(A, "greeting"),
                &[("attitude".into(), Argument::Text("unknown".into()))].into()
            )
            .is_err()
    );
}
#[test]
fn imports_use_whole_dependency_closure_for_fallback_and_review_revisions() {
    let mut a = contract(A, &[("greeting", &[]), ("unrelated", &[])]);
    a.imports.insert(B);
    let b = contract(B, &[]);
    let build = |source_term: &str| {
        Localization::new(
            "en",
            vec![a.clone(), b.clone()],
            vec![
                resource(&a, "en", "greeting = Hello { -title }\nunrelated = Other"),
                resource(&a, "uk", "greeting = Вітаю { -title }"),
                resource(&b, "en", source_term),
                resource(&b, "uk", ""),
            ],
        )
        .unwrap()
    };
    let first = build("-title = Captain");
    let translated = first
        .format("uk", &message(A, "greeting"), &Arguments::new())
        .unwrap();
    assert_eq!(translated.locale.as_deref(), Some("en"));
    assert!(translated.value.contains("Captain"));
    assert!(!translated.value.contains("Вітаю"));
    let next = build("-title = Commander");
    assert_ne!(
        first
            .scope(A, "en")
            .unwrap()
            .revision(&key("greeting"))
            .unwrap(),
        next.scope(A, "en")
            .unwrap()
            .revision(&key("greeting"))
            .unwrap()
    );
    assert_eq!(
        first
            .scope(A, "en")
            .unwrap()
            .revision(&key("unrelated"))
            .unwrap(),
        next.scope(A, "en")
            .unwrap()
            .revision(&key("unrelated"))
            .unwrap()
    );
}
#[test]
fn invalid_syntax_duplicates_cycles_and_contract_mismatches_fail() {
    let c = contract(A, &[("greeting", &[])]);
    for text in [
        "greeting = {",
        "greeting = One\ngreeting = Two",
        "greeting = { -a }\n-a = { -b }\n-b = { -a }",
        "greeting = { missing }",
        "greeting = { -a }\n-a = { $ambient }",
    ] {
        let service =
            Localization::new("en", vec![c.clone()], vec![resource(&c, "en", text)]).unwrap();
        assert!(
            service
                .scope(A, "en")
                .and_then(|s| s.validate(&key("greeting")))
                .is_err()
        );
    }
    let mut broken = resource(&c, "en", "greeting = Hello");
    broken.contract_hash = [0; 32];
    let service = Localization::new("en", vec![c], vec![broken]).unwrap();
    assert!(service.scope(A, "en").is_err());
}
struct Counting {
    source: MemorySource,
    reads: Rc<RefCell<Vec<(TextResourceId, String)>>>,
}
impl ResourceSource for Counting {
    fn contract(&mut self, id: TextResourceId) -> yarra_localization::Result<TextContract> {
        self.source.contract(id)
    }
    fn resource(
        &mut self,
        id: TextResourceId,
        locale: &str,
    ) -> yarra_localization::Result<Option<LanguageResource>> {
        self.reads.borrow_mut().push((id, locale.into()));
        self.source.resource(id, locale)
    }
}
#[test]
fn parsing_is_lazy_bounded_and_pinned_scopes_cannot_be_evicted() {
    let a = contract(A, &[("greeting", &[])]);
    let b = contract(B, &[("greeting", &[])]);
    let reads = Rc::new(RefCell::new(Vec::new()));
    let source = Counting {
        reads: reads.clone(),
        source: MemorySource::new(
            vec![a.clone(), b.clone()],
            vec![
                resource(&a, "en", "greeting = First"),
                resource(&b, "en", "greeting = Second"),
                resource(&a, "uk", "greeting = Перший"),
            ],
        )
        .unwrap(),
    };
    let service = Localization::with_source(
        "en",
        Box::new(source),
        LocalizationLimits {
            scopes: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(reads.borrow().is_empty());
    let pinned = service.scope(A, "uk").unwrap();
    assert_eq!(reads.borrow().as_slice(), &[(A, "uk".into())]);
    assert!(matches!(
        service.scope(B, "en"),
        Err(LocalizationError::Budget(_))
    ));
    assert_eq!(
        pinned.format(&key("greeting"), &Arguments::new()).unwrap(),
        "Перший"
    );
    drop(pinned);
    service.scope(B, "en").unwrap();
    assert_eq!(service.cached_scopes(), 1);
    assert!(service.cached_charge() < LocalizationLimits::default().charge_bytes);
}
#[test]
fn term_arguments_and_message_attributes_are_validated_transitively() {
    let c = contract(A, &[("greeting", &[]), ("label", &[])]);
    let service = Localization::new("en",vec![c.clone()],vec![resource(&c,"en","greeting = { -title(case: \"formal\") } { label.short }\n-title = { $case ->\n [formal] Captain\n *[other] Friend\n}\nlabel = Ship\n .short = HMS")]).unwrap();
    let text = service
        .format("en", &message(A, "greeting"), &BTreeMap::new())
        .unwrap()
        .value;
    assert!(text.contains("Captain") && text.contains("HMS"));
}

#[test]
fn transitive_message_contracts_and_parser_depth_are_checked() {
    let c = contract(
        A,
        &[
            ("greeting", &[("count", ArgumentType::Text)]),
            ("counter", &[("count", ArgumentType::Number)]),
        ],
    );
    let service = Localization::new(
        "en",
        vec![c.clone()],
        vec![resource(
            &c,
            "en",
            "greeting = { counter }\ncounter = { $count }",
        )],
    )
    .unwrap();
    assert!(
        service
            .scope(A, "en")
            .unwrap()
            .validate_definitions(None)
            .unwrap_err()
            .to_string()
            .contains("incompatible")
    );
    let deep = format!("greeting = {}0{}", "{".repeat(1000), "}".repeat(1000));
    let service =
        Localization::new("en", vec![c.clone()], vec![resource(&c, "en", &deep)]).unwrap();
    assert!(
        service
            .scope(A, "en")
            .err()
            .unwrap()
            .to_string()
            .contains("nesting")
    );
}
