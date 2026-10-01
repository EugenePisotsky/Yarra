use game_types::*;
use std::collections::BTreeMap;
use yarra_localization::*;
const A: TextResourceId = TextResourceId([8; 16]);
const B: TextResourceId = TextResourceId([9; 16]);
fn key(s: &str) -> TextKey {
    TextKey::new(s).unwrap()
}
fn message(id: TextResourceId, k: &str) -> TextRef {
    TextRef::message(id, k).unwrap()
}
/// Resources as a package has them: the source language first, which sets the contract.
fn localization(
    resources: &[(TextResourceId, &[(&str, &str)])],
) -> yarra_localization::Result<Localization> {
    let mut contracts = Vec::new();
    let mut wording = Vec::new();
    for (id, locales) in resources {
        let source = contract(*id, locales[0].1)?;
        for (locale, text) in *locales {
            wording.push(LanguageResource {
                id: *id,
                locale: (*locale).into(),
                source: (*text).into(),
                contract_hash: contract_hash(&source)?,
            });
        }
        contracts.push(source);
    }
    Localization::new("en", contracts, wording)
}
fn args(values: &[(&str, Argument)]) -> Arguments {
    values
        .iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect()
}
#[test]
fn the_contract_is_what_the_source_language_uses() {
    let source = "\
count = { $n ->
    [one] One item
   *[other] { $n } items
}
welcome = Welcome, { $name }!
greeting = { $attitude ->
    [friendly] Hello
   *[hostile] Go away
}
captain = { -title(case: \"formal\") } { welcome } { label.short }
-title = { $case ->
    [formal] Captain
   *[other] Friend
}
label = Ship
    .short = HMS
";
    let found = contract(A, source).unwrap();
    let arguments = |name: &str| found.messages[&key(name)].arguments.clone();
    assert_eq!(
        arguments("count"),
        [("n".into(), ArgumentType::Number)].into()
    );
    assert_eq!(
        arguments("welcome"),
        [("name".into(), ArgumentType::Text)].into()
    );
    let attitude = ArgumentType::Select(["friendly".into(), "hostile".into()].into());
    assert_eq!(
        arguments("greeting"),
        [("attitude".into(), attitude)].into()
    );
    // What a referenced message takes, its caller takes too; a term's `$case` is its own.
    assert_eq!(
        arguments("captain"),
        [("name".into(), ArgumentType::Text)].into()
    );
    assert!(arguments("label").is_empty());
    assert!(!found.messages.contains_key(&key("title")));
}
#[test]
fn identical_keys_are_kept_apart_and_locales_fall_back() {
    let service = localization(&[
        (A, &[("en", "greeting = First")]),
        (
            B,
            &[("en", "greeting = Second"), ("uk", "greeting = Другий")],
        ),
    ])
    .unwrap();
    let say = |locale, text| service.format(locale, &text, &Arguments::new()).unwrap();
    assert_eq!(say("uk", message(A, "greeting")).value, "First");
    let result = say("uk-UA", message(B, "greeting"));
    assert_eq!(result.value, "Другий");
    assert!(result.used_fallback);
    assert_eq!(say("en", message(B, "greeting")).value, "Second");
    assert_eq!(say("en", TextRef::Literal("My name".into())).locale, None);
}
#[test]
fn arguments_are_checked_against_the_contract() {
    let source = "count = { $n ->\n [one] One item\n *[other] { $n } items\n}\nwelcome = Welcome { $name }\ngreeting = { $attitude ->\n [friendly] Hello\n *[hostile] Go away\n}";
    let uk = "count = { $n ->\n [one] предмет\n [few] предмети\n *[many] предметів\n}";
    let service = localization(&[(A, &[("en", source), ("uk", uk)])]).unwrap();
    let format = |locale, name, values: &[(&str, Argument)]| {
        service.format(locale, &message(A, name), &args(values))
    };
    for (n, word) in [
        (1, "предмет"),
        (2, "предмети"),
        (5, "предметів"),
        (21, "предмет"),
    ] {
        let said = format("uk", "count", &[("n", Argument::Number(n))]).unwrap();
        assert_eq!(said.value, word);
    }
    // Text shows numbers as well; a number does not take text.
    let shown = format("en", "welcome", &[("name", Argument::Number(3))]).unwrap();
    assert_eq!(shown.value, "Welcome \u{2068}3\u{2069}");
    assert!(format("en", "count", &[("n", Argument::Text("1".into()))]).is_err());
    assert!(format("en", "welcome", &[]).is_err());
    let unknown = Argument::Text("unknown".into());
    assert!(format("en", "greeting", &[("attitude", unknown)]).is_err());
}
#[test]
fn translations_may_leave_messages_out_but_not_add_or_change_them() {
    let source = "greeting = Hello { -brand }\nfarewell = Bye\n-brand = Yarra";
    let check = |uk: &str| {
        let service = localization(&[(A, &[("en", source), ("uk", uk)])])?;
        let english = service.scope(A, "en")?;
        service
            .scope(A, "uk")?
            .validate_definitions(Some(&english))?;
        Ok::<_, LocalizationError>(service)
    };
    // Leaning on a term only the source defines, the message falls back whole.
    let service = check("greeting = Привіт { -brand }").unwrap();
    let said = service
        .format("uk", &message(A, "greeting"), &Arguments::new())
        .unwrap();
    assert_eq!(
        (said.value.as_str(), said.used_fallback),
        ("Hello Yarra", true)
    );
    assert!(
        service
            .format("uk", &message(A, "farewell"), &Arguments::new())
            .unwrap()
            .used_fallback
    );
    // A message the source does not have, or an argument it does not take, is an error.
    assert!(check("welcome = Ласкаво просимо").is_err());
    assert!(check("greeting = Привіт { $name }").is_err());
}
#[test]
fn invalid_syntax_duplicates_cycles_and_unknown_references_fail() {
    for text in [
        "greeting = {",
        "greeting = One\ngreeting = Two",
        "greeting = { -a }\n-a = { -b }\n-b = { -a }",
        "greeting = { missing }",
        "greeting = { UNSUPPORTED() }",
    ] {
        assert!(contract(A, text).is_err(), "{text}");
    }
    // Wording made for another contract is refused.
    let source = contract(A, "greeting = Hello").unwrap();
    let service = Localization::new(
        "en",
        vec![source],
        vec![LanguageResource {
            id: A,
            locale: "en".into(),
            source: "greeting = Hello".into(),
            contract_hash: [0; 32],
        }],
    )
    .unwrap();
    assert!(service.scope(A, "en").is_err());
}
#[test]
fn term_arguments_attributes_and_deep_nesting() {
    let service = localization(&[(
        A,
        &[(
            "en",
            "greeting = { -title(case: \"formal\") } { label.short }\n-title = { $case ->\n [formal] Captain\n *[other] Friend\n}\nlabel = Ship\n .short = HMS",
        )],
    )])
    .unwrap();
    let text = service
        .format("en", &message(A, "greeting"), &BTreeMap::new())
        .unwrap()
        .value;
    assert!(text.contains("Captain") && text.contains("HMS"));
    let deep = format!("greeting = {}0{}", "{".repeat(1000), "}".repeat(1000));
    let error = contract(A, &deep).unwrap_err().to_string();
    assert!(error.contains("nesting"), "{error}");
}
