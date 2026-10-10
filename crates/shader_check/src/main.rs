//! `cargo run -p yarra-shader-check -- [OPTIONS] [SHADER...]`
//!
//! With no shaders, checks every entry of `crates/shader_check/entries.ron`. Otherwise composes
//! and validates each asset shader (`shaders/x/y.wesl`) with the given definitions.
//!
//! - `--defs "A,B,!C,N=3"`: shader definitions, as the game's pipeline log prints them.
//! - `--each-flag`: also check with each `@if` flag of the shader (and the project modules it
//!   imports) flipped from its state in `--defs`, one at a time.
//! - `--print`: print the composed WGSL of the first variant.
//! - `--parse-all`: only parse every `.wesl` file under `assets/` and report syntax errors.

use std::process::ExitCode;

use yarra_shader_check::{Def, Shaders};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut defs = Vec::new();
    let mut each_flag = false;
    let mut print = false;
    let mut shaders = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--defs" => match args.next().map(|list| Def::parse_list(&list)) {
                Some(Ok(list)) => defs.extend(list),
                Some(Err(error)) => return fail(&error),
                None => return fail("--defs needs a list"),
            },
            "--each-flag" => each_flag = true,
            "--print" => print = true,
            "--parse-all" => return parse_all(),
            _ if arg.starts_with("--") => return fail(&format!("unknown option {arg}")),
            _ => shaders.push(arg.trim_start_matches("assets/").to_string()),
        }
    }

    let checker = Shaders::get();
    let mut jobs: Vec<(String, Vec<Def>)> = Vec::new();
    if shaders.is_empty() {
        match yarra_shader_check::entries::load(checker) {
            Ok(entries) => jobs.extend(entries),
            Err(error) => return fail(&error),
        }
    }
    for shader in &shaders {
        jobs.push((shader.clone(), defs.clone()));
        if each_flag {
            let flags = match checker.flags(shader) {
                Ok(flags) => flags,
                Err(error) => return fail(&error),
            };
            for flag in flags {
                let mut variant: Vec<Def> =
                    defs.iter().filter(|d| d.name() != flag).cloned().collect();
                let on = defs
                    .iter()
                    .any(|d| d.name() == flag && *d != Def::Flag(flag.clone(), false));
                if !on {
                    variant.push(Def::Flag(flag, true));
                }
                jobs.push((shader.clone(), variant));
            }
        }
    }

    let mut failures = 0;
    for (index, (shader, defs)) in jobs.iter().enumerate() {
        match checker.check(shader, defs) {
            Ok(wgsl) => {
                if print && index == 0 {
                    println!("{wgsl}");
                }
            }
            Err(error) => {
                failures += 1;
                let defs: Vec<String> = defs.iter().map(def_text).collect();
                eprintln!("FAIL {shader} [{}]\n{error}\n", defs.join(" "));
            }
        }
    }
    eprintln!(
        "{} of {} variants composed and validated",
        jobs.len() - failures,
        jobs.len()
    );
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn parse_all() -> ExitCode {
    let shaders = Shaders::get();
    let files = shaders.parse_all();
    let mut failures = 0;
    for (file, error) in &files {
        if let Some(error) = error {
            failures += 1;
            let shown = file
                .strip_prefix(shaders.assets())
                .unwrap_or(file)
                .display();
            eprintln!("PARSE {shown}: {error}");
        }
    }
    eprintln!("{} of {} files parse", files.len() - failures, files.len());
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn def_text(def: &Def) -> String {
    match def {
        Def::Flag(name, true) => name.clone(),
        Def::Flag(name, false) => format!("!{name}"),
        Def::Int(name, value) => format!("{name}={value}"),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}
