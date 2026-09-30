use game_types::{Invalid, PlaythroughId, require};
use localization::Arguments;
use save::{SaveDirectory, SaveSlot};
use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};
use yarra_game_content::{
    ANALYZER_VARIABLE, Asset, AssetId, ContentRepository, LoadedProject, Result, ScriptAnalysis,
};
const USAGE: &str = "Usage:
  yarra-game-content validate SOURCE_DIR|BUNDLE.sqlite [--language PACK.sqlite]...
  yarra-game-content scenario SOURCE_DIR SCENARIO.ron
  yarra-game-content build SOURCE_DIR NEW_BUNDLE.sqlite
  yarra-game-content build-language SOURCE_DIR LOCALE NEW_PACK.sqlite
  yarra-game-content inspect BUNDLE.sqlite [--item NAME]... [--dialogue NAME]...
  yarra-game-content script-api
  yarra-game-content demo SOURCE_DIR|BUNDLE.sqlite [--language PACK.sqlite]... [--locale LOCALE] [--save-dir NEW_DIR]

Source directories declare packages, conversations, message contracts and Fluent files.
Validation and builds run the scenario in isolation. Demo writes fresh save slots.
Inspect reads only the requested assets; it never runs a scenario or loads translations.
Existing bundle paths and demo save directories are never overwritten.";
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
fn load(path: &Path, languages: &[PathBuf]) -> Result<LoadedProject> {
    if path.is_dir() {
        require(
            languages.is_empty(),
            "source projects declare their own language resources",
        )?;
        LoadedProject::load_directory(path)
    } else {
        LoadedProject::materialize_with_languages_for_tools(path, languages)
    }
}
fn report(project: &LoadedProject) -> Result<()> {
    println!(
        "Validated: {} categories, {} items, {} actor templates, {} dialogues, {} scenario steps.",
        project.content().items.categories.len(),
        project.content().items.items.len(),
        project.content().game.actors.len(),
        project.content().game.dialogues.len(),
        project.scenario().steps.len()
    );
    let fingerprint: String = project
        .content()
        .fingerprint()?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("Content fingerprint: {fingerprint}");
    for warning in project.warnings() {
        eprintln!("warning: {warning}");
    }
    match project.analyze_scripts()? {
        ScriptAnalysis::Unavailable => eprintln!(
            "note: luau-analyze not found; scripts were compiled but not type-checked \
             (install Luau or set {ANALYZER_VARIABLE})"
        ),
        ScriptAnalysis::Checked { modules, warnings } => {
            println!("Scripts type-checked: {modules}");
            for warning in warnings {
                eprintln!("warning: {warning}");
            }
        }
    }
    Ok(())
}
fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let command = args.next().ok_or_else(|| Invalid(USAGE.into()))?;
    if command == "--help" || command == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    if command == "script-api" {
        // Type definitions of the host API, for luau-analyze and editors.
        print!("{}", scripting::API_DEFINITIONS);
        return Ok(());
    }
    let input = PathBuf::from(args.next().ok_or_else(|| Invalid(USAGE.into()))?);
    match command.to_str() {
        Some("scenario") => {
            let scenario = args
                .next()
                .and_then(|s| s.into_string().ok())
                .ok_or_else(|| Invalid(USAGE.into()))?;
            require(args.next().is_none(), USAGE)?;
            let project = LoadedProject::load_directory_with_scenario(&input, &scenario)?;
            report(&project)?;
            println!("Scenario passed: {scenario}");
            Ok(())
        }
        Some("inspect") => {
            let mut roots = Vec::new();
            while let Some(flag) = args.next() {
                require(roots.len() < 128, "inspect accepts at most 128 roots")?;
                let value = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .ok_or_else(|| Invalid("inspect option needs an asset UUID".into()))?;
                roots.push(match flag.to_str() {
                    Some("--item") => AssetId::Item(game_types::ItemDefinitionId::try_from(value)?),
                    Some("--dialogue") => {
                        AssetId::Dialogue(game_types::DialogueId::try_from(value)?)
                    }
                    _ => return Err(Invalid(format!("unknown inspect option {flag:?}")).into()),
                });
            }
            let repository = ContentRepository::open(input)?;
            println!(
                "Publication: {} revision {}",
                repository.manifest().content.id,
                repository.manifest().content.revision
            );
            for id in &roots {
                match repository.read(id)? {
                    Asset::Item(item) => println!("Item {}: {} g", item.id, item.weight_grams),
                    Asset::Dialogue(graph) => {
                        println!("Dialogue {}: {} nodes", graph.id, graph.nodes.len())
                    }
                    _ => println!("Resolved {id:?}"),
                }
            }
            Ok(())
        }
        Some("validate") => {
            let mut languages = Vec::new();
            while let Some(flag) = args.next() {
                require(flag == "--language", USAGE)?;
                languages.push(PathBuf::from(
                    args.next().ok_or_else(|| Invalid(USAGE.into()))?,
                ));
            }
            report(&load(&input, &languages)?)
        }
        Some("build-language") => {
            let locale = args
                .next()
                .and_then(|v| v.into_string().ok())
                .ok_or_else(|| Invalid(USAGE.into()))?;
            let output = PathBuf::from(args.next().ok_or_else(|| Invalid(USAGE.into()))?);
            require(args.next().is_none(), USAGE)?;
            let project = LoadedProject::load_directory(&input)?;
            project.build_language_pack(&locale, &output)?;
            println!("Published {} language pack: {}", locale, output.display());
            Ok(())
        }
        Some("build") => {
            let output = PathBuf::from(args.next().ok_or_else(|| Invalid(USAGE.into()))?);
            require(args.next().is_none(), USAGE)?;
            let project = LoadedProject::load_directory(&input)?;
            project.build(&output)?;
            report(&project)?;
            println!("Published {}", output.display());
            Ok(())
        }
        Some("demo") => {
            let mut languages = Vec::new();
            let mut locale = None;
            let mut directory = None;
            while let Some(flag) = args.next() {
                match flag.to_str() {
                    Some("--language") => languages.push(PathBuf::from(
                        args.next().ok_or_else(|| Invalid(USAGE.into()))?,
                    )),
                    Some("--locale") if locale.is_none() => {
                        locale = Some(
                            args.next()
                                .and_then(|a| a.into_string().ok())
                                .ok_or_else(|| Invalid("--locale needs a UTF-8 locale".into()))?,
                        );
                    }
                    Some("--save-dir") if directory.is_none() => {
                        directory = Some(PathBuf::from(
                            args.next()
                                .ok_or_else(|| Invalid("--save-dir needs a path".into()))?,
                        ));
                    }
                    _ => {
                        return Err(
                            Invalid(format!("unknown/duplicate option {flag:?}\n{USAGE}")).into(),
                        );
                    }
                }
            }
            let project = load(&input, &languages)?;
            report(&project)?;
            demo(
                &project,
                locale.as_deref().unwrap_or(project.source_locale()),
                directory,
            )
        }
        _ => Err(Invalid(USAGE.into()).into()),
    }
}
fn demo(project: &LoadedProject, locale: &str, directory: Option<PathBuf>) -> Result<()> {
    let args = Arguments::new();
    // Validate presentation before creating the requested output directory.
    for key in project.content().text_keys() {
        if project
            .content()
            .text
            .iter()
            .find(|c| c.id == key.resource)
            .is_some_and(|c| !c.messages[&key.key].arguments.is_empty())
        {
            continue;
        }
        project
            .localization()
            .format(locale, &game_types::TextRef::Message(key.clone()), &args)?;
    }
    let directory = directory.unwrap_or_else(|| {
        env::temp_dir().join(format!("yarra-gameplay-{}", PlaythroughId::new()))
    });
    std::fs::create_dir(&directory)?;
    let publication = directory.join("published.sqlite");
    project.build(&publication)?;
    let library = yarra_game_content::ContentLibrary::new(directory.join("content"))?;
    let identity = library.retain(&publication)?;
    std::fs::remove_file(publication)?;
    // Play the scenario against the published bundle, not the authored sources.
    let seed = project.start()?.into_state();
    let mut session = gameplay::GameSession::new(library.open(&identity)?, seed)?;
    for step in &project.scenario().steps {
        step.apply(&mut session)?;
    }
    let state = session.state();
    // Resolve all presentation before writing files, including invalid locale errors.
    let mut summary = Vec::new();
    for actor in state.actors.values() {
        let template = project.content().template(actor.template)?;
        let name = project
            .localization()
            .format(
                locale,
                actor.name_override.as_ref().unwrap_or(&template.name),
                &args,
            )?
            .value;
        let rules = &project.content().game.rules;
        let named = |text: &game_types::TextRef| project.localization().format(locale, text, &args);
        summary.push(format!(
            "{name} ({}): level {} {}, {} {}",
            actor.id,
            actor.level,
            named(&rules.class(&actor.class)?.name)?.value,
            named(&rules.stat(&rules.life)?.name)?.value,
            actor.resources[&rules.life]
        ));
        for (skill, rank) in &actor.skills {
            let name = &rules.skill(skill)?.name;
            summary.push(format!("  {}: rank {rank}", named(name)?.value));
        }
        // An inventory nobody has looked into is not listed.
        for entry in state.carried(actor.id).iter().flat_map(|bag| &bag.entries) {
            let name = &project.content().items.item(entry.definition)?.name;
            summary.push(format!(
                "  {} × {}",
                project.localization().format(locale, name, &args)?.value,
                entry.quantity
            ));
        }
    }
    for wallet in state.wallets.values() {
        summary.push(format!(
            "{} wallet {}: {} gold",
            wallet.owner.kind,
            wallet.id,
            wallet.balance.units()
        ));
    }
    let saves = SaveDirectory::new(&directory, 3)?;
    saves.save(SaveSlot::Manual(1), &session, "Authored scenario")?;
    saves.quicksave(&session)?;
    saves.autosave(&session)?;
    let loaded = library.load(&saves, SaveSlot::Manual(1))?;
    require(
        loaded.state() == state,
        "save roundtrip differs from authored scenario result",
    )?;
    println!(
        "Dialogue graphs loaded: {} of {}",
        session.content().game.dialogues.len(),
        project.content().game.dialogues.len()
    );
    for line in summary {
        println!("{line}");
    }
    println!("Save reopened; 3 saves in {}", directory.display());
    Ok(())
}
