//! Optional type checking of scripts with Luau's own analyzer. Publication always compiles
//! scripts and checks that referenced functions exist; this adds the checks a compiler
//! gives: misspelled API calls, wrong argument types, fields that are not there.
use crate::{ContentError, LoadedProject, Result};
use std::process::Command;

/// Set to the analyzer's path when it is not on `PATH`.
pub const ANALYZER_VARIABLE: &str = "YARRA_LUAU_ANALYZE";

#[derive(Debug, PartialEq, Eq)]
pub enum ScriptAnalysis {
    /// `luau-analyze` is not installed; scripts were compiled but not type-checked.
    Unavailable,
    Checked {
        modules: usize,
        warnings: Vec<String>,
    },
}
impl LoadedProject {
    /// Type errors fail; lints are returned as warnings. Locations refer to the script as
    /// authored.
    pub fn analyze_scripts(&self) -> Result<ScriptAnalysis> {
        let modules = &self.content.game.scripts;
        let analyzer = std::env::var_os(ANALYZER_VARIABLE).unwrap_or_else(|| "luau-analyze".into());
        let directory = std::env::temp_dir().join(format!(
            "yarra-script-check-{}",
            game_types::PackageId::random().raw()
        ));
        std::fs::create_dir(&directory)?;
        let result = (|| -> Result<ScriptAnalysis> {
            let prelude = scripting::API_DEFINITIONS;
            let offset = prelude.lines().count();
            let mut command = Command::new(&analyzer);
            command.current_dir(&directory).arg("--mode=strict");
            for module in modules {
                let file = format!("{}.luau", module.name.as_str());
                let mut text = String::with_capacity(prelude.len() + module.source.len() + 1);
                text.push_str(prelude);
                if !prelude.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&module.source);
                std::fs::write(directory.join(&file), text)?;
                command.arg(file);
            }
            let output = match command.output() {
                Ok(output) => output,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(ScriptAnalysis::Unavailable);
                }
                Err(error) => return Err(error.into()),
            };
            let mut errors = Vec::new();
            let mut warnings = Vec::new();
            let text = [output.stdout, output.stderr].concat();
            for line in String::from_utf8_lossy(&text).lines() {
                let line = relocate(line.trim_start_matches("./"), offset);
                if line.contains("): TypeError:") || line.contains("): SyntaxError:") {
                    errors.push(line);
                } else if !line.is_empty() {
                    warnings.push(line);
                }
            }
            if !errors.is_empty() {
                return Err(ContentError::Validation {
                    context: "scripts".into(),
                    message: errors.join("\n"),
                });
            }
            if !output.status.success() && warnings.is_empty() {
                return Err(ContentError::Validation {
                    context: "scripts".into(),
                    message: format!("{} failed", analyzer.to_string_lossy()),
                });
            }
            Ok(ScriptAnalysis::Checked {
                modules: modules.len(),
                warnings,
            })
        })();
        let _ = std::fs::remove_dir_all(&directory);
        result
    }
}
/// `name.luau(line,column): message`, with the line moved back past the definitions.
fn relocate(line: &str, offset: usize) -> String {
    let moved = (|| {
        let (file, rest) = line.split_once('(')?;
        let (position, message) = rest.split_once(')')?;
        let (row, column) = position.split_once(',')?;
        let row: usize = row.parse().ok()?;
        Some(format!(
            "{file}({},{column}){message}",
            row.checked_sub(offset)?
        ))
    })();
    moved.unwrap_or_else(|| line.to_owned())
}
