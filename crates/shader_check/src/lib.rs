//! Offline composition and validation of the project's WESL shaders.
//!
//! Bevy composes WESL at runtime, when a pipeline is first specialized, so a broken branch only
//! shows up once some view needs that variant. This crate runs the same `wesl` compiler with the
//! same options Bevy's shader cache uses (`bevy_shader::shader_cache`), resolving Bevy's own
//! modules from its crate sources and ours from `assets/`, then validates the WGSL with naga.
//!
//! Module paths follow Bevy: an asset shader `shaders/a/b.wesl` is `package::shaders::a::b`,
//! an embedded Bevy shader `bevy_pbr/src/render/x.wesl` is `bevy_pbr::render::x`, and integer
//! shader definitions are `constants::NAME` (they also enable a flag of the same name).

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use wesl::syntax::{ModulePath, PathOrigin};

/// A shader definition as Bevy's `ShaderDefVal` carries it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Def {
    Flag(String, bool),
    Int(String, i64),
}

impl Def {
    /// `NAME`, `!NAME` or `NAME=VALUE`, as the game's pipeline log prints them.
    pub fn parse(text: &str) -> Result<Def, String> {
        let text = text.trim();
        if let Some(name) = text.strip_prefix('!') {
            return Ok(Def::Flag(name.into(), false));
        }
        match text.split_once('=') {
            Some((name, value)) => value
                .trim()
                .parse()
                .map(|value| Def::Int(name.trim().into(), value))
                .map_err(|_| format!("shader def {text:?}: value is not an integer")),
            None if text.is_empty() => Err("empty shader def".into()),
            None => Ok(Def::Flag(text.into(), true)),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Def::Flag(name, _) | Def::Int(name, _) => name,
        }
    }

    pub fn parse_list(text: &str) -> Result<Vec<Def>, String> {
        text.split([',', ' '])
            .filter(|t| !t.trim().is_empty())
            .map(Def::parse)
            .collect()
    }
}

/// Shader sources of the workspace and of the Bevy crates it builds against.
pub struct Shaders {
    workspace: PathBuf,
    packages: HashMap<String, PathBuf>,
}

impl Shaders {
    /// The workspace's shaders, with Bevy's crate sources found through `cargo metadata`.
    /// Discovery runs once per process.
    pub fn get() -> &'static Shaders {
        static SHADERS: OnceLock<Shaders> = OnceLock::new();
        SHADERS.get_or_init(|| Shaders::discover().unwrap_or_else(|e| panic!("{e}")))
    }

    fn discover() -> Result<Shaders, String> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .map_err(|e| format!("workspace root: {e}"))?;
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--offline",
                "--manifest-path",
            ])
            .arg(workspace.join("Cargo.toml"))
            .output()
            .map_err(|e| format!("cargo metadata: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("cargo metadata output: {e}"))?;
        let mut packages = HashMap::new();
        for package in metadata["packages"].as_array().into_iter().flatten() {
            let (Some(name), Some(manifest)) =
                (package["name"].as_str(), package["manifest_path"].as_str())
            else {
                continue;
            };
            if name.starts_with("bevy_") {
                let source = Path::new(manifest).with_file_name("src");
                packages.insert(name.to_string(), source);
            }
        }
        Ok(Shaders {
            workspace,
            packages,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// The project's asset directory.
    pub fn assets(&self) -> PathBuf {
        self.workspace.join("assets")
    }

    /// Composes an asset shader (`shaders/x/y.wesl`) into WGSL with these definitions.
    pub fn compose(&self, asset_path: &str, defs: &[Def]) -> Result<String, String> {
        let module = asset_module(asset_path)?;
        let mut options = wesl::CompileOptions {
            imports: true,
            condcomp: true,
            visibility: false,
            ..Default::default()
        };
        // As Bevy does: integers enable a flag of their name and become `constants::NAME`.
        let mut constants = BTreeMap::new();
        for def in bevy_defs().iter().chain(defs) {
            match def {
                Def::Flag(name, on) => {
                    options.features.flags.insert(name.clone(), (*on).into());
                }
                Def::Int(name, value) => {
                    options.features.flags.insert(name.clone(), true.into());
                    constants.insert(name.clone(), *value);
                }
            }
        }
        let constants: String = constants
            .iter()
            .map(|(name, value)| format!("const {name} = {value};\n"))
            .collect();
        let resolver = Resolver {
            shaders: self,
            constants,
        };
        wesl::compile(&module, &options, &resolver)
            .map(|compiled| compiled.to_string())
            .map_err(|error| error.diagnostic().render_plain())
    }

    /// Composes and validates; returns the WGSL.
    pub fn check(&self, asset_path: &str, defs: &[Def]) -> Result<String, String> {
        let wgsl = self.compose(asset_path, defs)?;
        validate(&wgsl).map_err(|error| format!("{asset_path}: {error}"))?;
        Ok(wgsl)
    }

    /// Every `.wesl` file under `assets/shaders`, with the error of each that fails to parse.
    pub fn parse_all(&self) -> Vec<(PathBuf, Option<String>)> {
        let mut files = Vec::new();
        let mut pending = vec![self.assets().join("shaders")];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|e| e == "wesl") {
                    let error = std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|source| {
                            source
                                .parse::<wesl::syntax::TranslationUnit>()
                                .map(drop)
                                .map_err(|e| e.to_string())
                        })
                        .err();
                    files.push((path, error));
                }
            }
        }
        files.sort();
        files
    }

    /// The source of a module path, if it names one of ours or Bevy's.
    fn source(&self, path: &ModulePath) -> Option<PathBuf> {
        let relative = path.components.join("/") + ".wesl";
        match &path.origin {
            PathOrigin::Absolute => Some(self.assets().join(relative)),
            // A dependency's dependency is named `outer/inner`; Bevy resolves it as `inner`.
            PathOrigin::Package(package) => {
                let package = package.rsplit('/').next().unwrap_or(package);
                self.packages.get(package).map(|s| s.join(relative))
            }
            PathOrigin::Relative(_) => None,
        }
    }

    /// Feature flags tested by `@if`/`@elif` in a shader and the project modules it imports.
    pub fn flags(&self, asset_path: &str) -> Result<BTreeSet<String>, String> {
        let mut flags = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut pending = vec![self.assets().join(asset_path)];
        while let Some(file) = pending.pop() {
            if !seen.insert(file.clone()) {
                continue;
            }
            let source =
                std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            flags.extend(condition_flags(&source));
            for import in project_imports(&source, &file, &self.assets()) {
                pending.push(import);
            }
        }
        Ok(flags)
    }
}

/// Applies only WESL conditional compilation (`@if`/`@elif`/`@else`) to a piece of shader text,
/// for tests that evaluate a slice of a production shader without its imports or bindings.
pub fn resolve_conditions(source: &str, defs: &[Def]) -> Result<String, String> {
    struct Text<'a>(&'a str);
    impl wesl::Resolver for Text<'_> {
        fn resolve_source<'b>(
            &'b self,
            _: &ModulePath,
        ) -> Result<Cow<'b, str>, wesl::error::ResolveError> {
            Ok(Cow::Borrowed(self.0))
        }
    }
    let mut options = wesl::CompileOptions {
        imports: false,
        condcomp: true,
        visibility: false,
        strip: false,
        validate: false,
        sourcemap: false,
        ..Default::default()
    };
    for def in defs {
        let on = !matches!(def, Def::Flag(_, false));
        options
            .features
            .flags
            .insert(def.name().to_string(), on.into());
    }
    let root = ModulePath {
        origin: PathOrigin::Absolute,
        components: vec!["slice".into()],
    };
    wesl::compile(&root, &options, &Text(source))
        .map(|compiled| compiled.to_string())
        .map_err(|error| error.diagnostic().render_plain())
}

/// Definitions Bevy adds to every composition on this Mac, ahead of a pipeline's own: the
/// pipeline cache's global ones (`pipeline_cache.rs`) and those `mesh_view_types` is loaded with
/// (`bevy_pbr` `mesh.rs`), which reach every shader importing it.
pub fn bevy_defs() -> Vec<Def> {
    let int = |name: &str, value| Def::Int(name.into(), value);
    let flag = |name: &str| Def::Flag(name.into(), true);
    vec![
        int("AVAILABLE_STORAGE_BUFFER_BINDINGS", 31),
        flag("AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_3"),
        flag("AVAILABLE_STORAGE_BUFFER_BINDINGS__GE_6"),
        int("MAX_DIRECTIONAL_LIGHTS", 10),
        int("MAX_CASCADES_PER_LIGHT", 4),
        int("MAX_RECT_LIGHTS", 8),
    ]
}

/// `shaders/x/y.wesl` → `package::shaders::x::y`.
pub fn asset_module(asset_path: &str) -> Result<ModulePath, String> {
    let stem = asset_path
        .strip_suffix(".wesl")
        .ok_or_else(|| format!("{asset_path}: not a .wesl shader"))?;
    Ok(ModulePath {
        origin: PathOrigin::Absolute,
        components: stem.split('/').map(str::to_string).collect(),
    })
}

/// Naga validation of composed WGSL, with every capability allowed (the device decides those).
pub fn validate(wgsl: &str) -> Result<naga::Module, String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| e.emit_to_string(wgsl))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| e.emit_to_string(wgsl))?;
    Ok(module)
}

/// Identifiers named in `@if(...)` and `@elif(...)` conditions.
pub fn condition_flags(source: &str) -> BTreeSet<String> {
    let mut flags = BTreeSet::new();
    for (index, _) in source.match_indices('@') {
        let rest = &source[index + 1..];
        let Some(condition) = rest
            .strip_prefix("if")
            .or_else(|| rest.strip_prefix("elif"))
            .and_then(|r| r.trim_start().strip_prefix('('))
        else {
            continue;
        };
        let mut depth = 1;
        let end = condition
            .char_indices()
            .find(|&(_, c)| {
                depth += match c {
                    '(' => 1,
                    ')' => -1,
                    _ => 0,
                };
                depth == 0
            })
            .map_or(condition.len(), |(i, _)| i);
        flags.extend(
            condition[..end]
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter(|w| !w.is_empty() && !matches!(*w, "true" | "false"))
                .map(str::to_string),
        );
    }
    flags
}

/// Project shader files a source imports with `package::` or `super::` paths.
fn project_imports(source: &str, file: &Path, assets: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for statement in source.split(';') {
        let Some(start) = statement.find("import ") else {
            continue;
        };
        let path = statement[start + 7..].trim();
        let path: String = path.chars().filter(|c| !c.is_whitespace()).collect();
        let segments: Vec<&str> = path.split("::").collect();
        let base = match segments.first() {
            Some(&"package") => Some((assets.to_path_buf(), &segments[1..])),
            Some(&"super") => {
                let supers = segments.iter().take_while(|s| **s == "super").count();
                let mut dir = file.parent().map(Path::to_path_buf);
                for _ in 1..supers {
                    dir = dir.and_then(|d| d.parent().map(Path::to_path_buf));
                }
                dir.map(|d| (d, &segments[supers..]))
            }
            _ => None,
        };
        let Some((mut dir, rest)) = base else {
            continue;
        };
        // Walk down until a file exists; the remainder names items.
        for segment in rest {
            let segment = segment.trim_start_matches('{');
            if segment.is_empty() {
                break;
            }
            let candidate = dir.join(format!("{segment}.wesl"));
            if candidate.is_file() {
                files.push(candidate);
                break;
            }
            dir = dir.join(segment);
        }
    }
    files
}

struct Resolver<'a> {
    shaders: &'a Shaders,
    constants: String,
}

impl wesl::Resolver for Resolver<'_> {
    fn resolve_source<'b>(
        &'b self,
        path: &ModulePath,
    ) -> Result<Cow<'b, str>, wesl::error::ResolveError> {
        // Inside a Bevy module `constants` arrives as `bevy_pbr/constants`; Bevy's resolver keeps
        // the last segment of such package names.
        let constants = matches!(&path.origin,
            PathOrigin::Package(package) if package.rsplit('/').next() == Some("constants"));
        if constants && path.components.is_empty() {
            return Ok(Cow::Borrowed(&self.constants));
        }
        let not_found = |why: String| wesl::error::ResolveError::ModuleNotFound(path.clone(), why);
        let file = self
            .shaders
            .source(path)
            .ok_or_else(|| not_found("not a project or Bevy module".into()))?;
        std::fs::read_to_string(&file)
            .map(Cow::Owned)
            .map_err(|e| not_found(format!("{}: {e}", file.display())))
    }

    fn display_name(&self, path: &ModulePath) -> Option<String> {
        self.shaders.source(path).map(|p| p.display().to_string())
    }

    // As Bevy's `ShaderResolver`: `outer/inner` names the same module as `inner`, so a module
    // reached through two packages is composed once.
    fn canonical_path(&self, path: &ModulePath) -> ModulePath {
        match &path.origin {
            PathOrigin::Package(package) if package.contains('/') => ModulePath {
                origin: PathOrigin::Package(package.rsplit('/').next().unwrap().to_string()),
                components: path.components.clone(),
            },
            _ => path.clone(),
        }
    }
}

/// The checked variants: `crates/shader_check/entries.ron`.
pub mod entries {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::{Def, Shaders};

    #[derive(Deserialize)]
    struct File {
        /// Named definition lists; `@name` in a variant expands to one.
        bases: BTreeMap<String, Vec<String>>,
        entries: Vec<Entry>,
    }

    #[derive(Deserialize)]
    struct Entry {
        shader: String,
        variants: Vec<Vec<String>>,
    }

    /// Every (shader, definitions) pair to check.
    pub fn load(shaders: &Shaders) -> Result<Vec<(String, Vec<Def>)>, String> {
        let path = shaders.workspace().join("crates/shader_check/entries.ron");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file: File = ron::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut jobs = Vec::new();
        for entry in &file.entries {
            for variant in &entry.variants {
                let mut defs = Vec::new();
                for token in variant {
                    match token.strip_prefix('@') {
                        Some(base) => {
                            let list = file
                                .bases
                                .get(base)
                                .ok_or_else(|| format!("{}: unknown base @{base}", entry.shader))?;
                            for def in list {
                                defs.push(Def::parse(def)?);
                            }
                        }
                        None => defs.push(Def::parse(token)?),
                    }
                }
                jobs.push((entry.shader.clone(), defs));
            }
        }
        Ok(jobs)
    }
}
