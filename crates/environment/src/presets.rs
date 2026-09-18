//! Project presets and bounded, typed override resolution. No renderer or storage state.
use super::*;
use std::collections::BTreeMap;

pub const MAX_PRESETS: usize = 512;
pub const MAX_PRESET_DEPTH: usize = 8;
pub const MAX_PRESET_CHILDREN: usize = 64;
pub const MAX_PRESET_OUTPUTS: usize = 64;
pub const MAX_PRESET_OVERRIDES: usize = 128;
const MAX_EXPANSION_WORK: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetLibrary {
    pub revision: u64,
    pub presets: Vec<Preset>,
}
impl Default for PresetLibrary {
    fn default() -> Self {
        Self {
            revision: 1,
            presets: vec![],
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: PresetId,
    pub revision: u64,
    pub name: String,
    pub kind: PresetKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PresetKind {
    Ground(GroundTreatment),
    Foliage(VegetationTreatment),
    Exclusion(Exclusion),
    Composition(Vec<PresetUse>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetUse {
    pub id: PresetUseId,
    pub name: String,
    pub preset: PresetId,
    pub overrides: Vec<PresetOverride>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetOverride {
    /// Relative to the referenced root. Empty addresses a root leaf preset.
    pub path: Vec<PresetUseId>,
    pub value: QuickValue,
}
/// These are the entire supported quick-control interface, owned by code, not by authors.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum QuickValue {
    GroundInfluence(f32),
    FoliageDensity(f32),
    FoliageInfluence(f32),
    FoliageSeed(u32),
    ExclusionInfluence(f32),
}
impl QuickValue {
    pub fn key(self) -> u8 {
        match self {
            Self::GroundInfluence(_) => 0,
            Self::FoliageDensity(_) => 1,
            Self::FoliageInfluence(_) => 2,
            Self::FoliageSeed(_) => 3,
            Self::ExclusionInfluence(_) => 4,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedUse {
    pub path: Vec<PresetUseId>,
    pub names: Vec<String>,
    pub preset: PresetId,
    /// Always a leaf; values include enclosing use and layer overrides.
    pub kind: PresetKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedComposition {
    pub ground: Option<GroundTreatment>,
    pub vegetation: Vec<VegetationTreatment>,
    pub exclusions: Vec<Exclusion>,
    /// Only reachable preset revisions; unrelated library changes do not fingerprint this plan.
    pub dependencies: BTreeMap<PresetId, u64>,
}
fn invalid(message: impl Into<String>) -> ValidationError {
    ValidationError::Preset(message.into())
}

impl PresetLibrary {
    pub fn get(&self, id: PresetId) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }
    pub fn validate(&self, plants: &VegetationCatalog) -> Result<(), ValidationError> {
        plants.validate()?;
        if self.revision == 0 || self.presets.len() > MAX_PRESETS {
            return Err(invalid("library budget or revision"));
        }
        let mut ids = BTreeSet::new();
        for preset in &self.presets {
            if preset.revision == 0
                || !ids.insert(preset.id)
                || preset.name.trim().is_empty()
                || preset.name.len() > 256
            {
                return Err(invalid("preset identity"));
            }
            match &preset.kind {
                PresetKind::Ground(g) => {
                    let mut surfaces = BTreeSet::new();
                    if !unit(g.strength)
                        || g.surfaces.is_empty()
                        || g.surfaces.len() > 64
                        || g.surfaces.iter().any(|w| {
                            !w.weight.is_finite() || w.weight <= 0.0 || !surfaces.insert(w.surface)
                        })
                    {
                        return Err(invalid("ground settings"));
                    }
                }
                PresetKind::Foliage(v) => {
                    if !unit(v.strength) || !unit(v.density) {
                        return Err(invalid("foliage settings"));
                    }
                    if !plants.assemblages.iter().any(|a| a.id == v.assemblage) {
                        return Err(ValidationError::MissingAssemblage(v.assemblage));
                    }
                }
                PresetKind::Exclusion(e) if !unit(e.strength) => {
                    return Err(invalid("exclusion settings"));
                }
                PresetKind::Composition(children) => {
                    let mut uses = BTreeSet::new();
                    if children.len() > MAX_PRESET_CHILDREN
                        || children.iter().any(|c| {
                            !uses.insert(c.id)
                                || c.name.trim().is_empty()
                                || c.name.len() > 256
                                || c.overrides.len() > MAX_PRESET_OVERRIDES
                        })
                    {
                        return Err(invalid("composition child identity or budget"));
                    }
                }
                _ => {}
            }
        }
        for preset in &self.presets {
            self.resolve(preset.id, &[])?;
        }
        Ok(())
    }
    pub fn uses(
        &self,
        root: PresetId,
        overrides: &[PresetOverride],
    ) -> Result<Vec<ResolvedUse>, ValidationError> {
        self.expand(root, overrides).map(|(uses, _)| uses)
    }
    fn expand(
        &self,
        root: PresetId,
        overrides: &[PresetOverride],
    ) -> Result<(Vec<ResolvedUse>, BTreeMap<PresetId, u64>), ValidationError> {
        if self.presets.len() > MAX_PRESETS {
            return Err(invalid("library budget"));
        }
        let mut expansion = Expansion {
            library: self,
            stack: vec![],
            leaves: vec![],
            dependencies: BTreeMap::new(),
            work: 0,
        };
        expansion.visit(root, &[], &[])?;
        apply_overrides(&mut expansion.leaves, &[], overrides)?;
        expansion.leaves.sort_by(|a, b| a.path.cmp(&b.path));
        Ok((expansion.leaves, expansion.dependencies))
    }
    pub fn resolve(
        &self,
        root: PresetId,
        overrides: &[PresetOverride],
    ) -> Result<ResolvedComposition, ValidationError> {
        let (uses, dependencies) = self.expand(root, overrides)?;
        let mut result = ResolvedComposition {
            ground: None,
            vegetation: vec![],
            exclusions: vec![],
            dependencies,
        };
        let mut ids = BTreeSet::new();
        let mut replaced = BTreeSet::new();
        for leaf in uses {
            let derive = |id: OutputId| {
                let mut hash = blake3::Hasher::new();
                hash.update(b"yarra.environment.preset-use.v1");
                hash.update(&root.0);
                for child in &leaf.path {
                    hash.update(&child.0);
                }
                hash.update(&leaf.preset.0);
                hash.update(&id.0);
                OutputId(hash.finalize().as_bytes()[..16].try_into().unwrap())
            };
            let output = match leaf.kind {
                PresetKind::Ground(mut g) => {
                    if result.ground.is_some() {
                        return Err(invalid(format!(
                            "multiple ground outputs at {:?}",
                            leaf.path
                        )));
                    }
                    g.id = derive(g.id);
                    g.surfaces.sort_by_key(|w| w.surface);
                    let id = g.id;
                    result.ground = Some(g);
                    id
                }
                PresetKind::Foliage(mut v) => {
                    if v.blend == VegetationBlend::Replace && !replaced.insert(v.channel) {
                        return Err(invalid(format!(
                            "multiple replacements in channel {:?} at {:?}",
                            v.channel, leaf.path
                        )));
                    }
                    v.id = derive(v.id);
                    let id = v.id;
                    result.vegetation.push(v);
                    id
                }
                PresetKind::Exclusion(mut e) => {
                    e.id = derive(e.id);
                    let id = e.id;
                    result.exclusions.push(e);
                    id
                }
                PresetKind::Composition(_) => unreachable!(),
            };
            if !ids.insert(output) {
                return Err(invalid("resolved output identity collision"));
            }
        }
        result.vegetation.sort_by_key(|v| v.id);
        result.exclusions.sort_by_key(|e| e.id);
        Ok(result)
    }
}
struct Expansion<'a> {
    library: &'a PresetLibrary,
    stack: Vec<PresetId>,
    leaves: Vec<ResolvedUse>,
    dependencies: BTreeMap<PresetId, u64>,
    work: usize,
}
impl Expansion<'_> {
    fn visit(
        &mut self,
        id: PresetId,
        path: &[PresetUseId],
        names: &[String],
    ) -> Result<(), ValidationError> {
        self.work += 1;
        if path.len() > MAX_PRESET_DEPTH || self.work > MAX_EXPANSION_WORK {
            return Err(invalid("composition expansion budget"));
        }
        if self.stack.contains(&id) {
            return Err(invalid(format!("composition cycle at {id:?}")));
        }
        let preset = self
            .library
            .get(id)
            .ok_or_else(|| invalid(format!("missing preset {id:?} at {path:?}")))?;
        self.dependencies.insert(id, preset.revision);
        self.stack.push(id);
        match &preset.kind {
            PresetKind::Composition(children) => {
                if children.len() > MAX_PRESET_CHILDREN {
                    return Err(invalid("child budget"));
                }
                let mut ids = BTreeSet::new();
                for child in children {
                    if !ids.insert(child.id) {
                        return Err(invalid("duplicate child use"));
                    }
                    let mut path = path.to_vec();
                    path.push(child.id);
                    let mut names = names.to_vec();
                    names.push(child.name.clone());
                    let start = self.leaves.len();
                    self.visit(child.preset, &path, &names)?;
                    apply_overrides(&mut self.leaves[start..], &path, &child.overrides)?;
                }
            }
            kind => {
                if self.leaves.len() >= MAX_PRESET_OUTPUTS {
                    return Err(invalid("expanded output budget"));
                }
                self.leaves.push(ResolvedUse {
                    path: path.to_vec(),
                    names: names.to_vec(),
                    preset: id,
                    kind: kind.clone(),
                });
            }
        }
        self.stack.pop();
        Ok(())
    }
}
fn apply_overrides(
    leaves: &mut [ResolvedUse],
    prefix: &[PresetUseId],
    overrides: &[PresetOverride],
) -> Result<(), ValidationError> {
    if overrides.len() > MAX_PRESET_OVERRIDES {
        return Err(invalid("override budget"));
    }
    let mut keys = BTreeSet::new();
    for edit in overrides {
        if edit.path.len() + prefix.len() > MAX_PRESET_DEPTH
            || !keys.insert((&edit.path, edit.value.key()))
        {
            return Err(invalid("duplicate override or path budget"));
        }
        let path = prefix.iter().chain(&edit.path).copied().collect::<Vec<_>>();
        let leaf = leaves
            .iter_mut()
            .find(|l| l.path == path)
            .ok_or_else(|| invalid(format!("override targets a missing leaf at {path:?}")))?;
        let target = match (&mut leaf.kind, edit.value) {
            (PresetKind::Ground(g), QuickValue::GroundInfluence(v)) => Some((&mut g.strength, v)),
            (PresetKind::Foliage(f), QuickValue::FoliageDensity(v)) => Some((&mut f.density, v)),
            (PresetKind::Foliage(f), QuickValue::FoliageInfluence(v)) => Some((&mut f.strength, v)),
            (PresetKind::Foliage(f), QuickValue::FoliageSeed(seed)) => {
                f.seed = seed;
                None
            }
            (PresetKind::Exclusion(e), QuickValue::ExclusionInfluence(v)) => {
                Some((&mut e.strength, v))
            }
            _ => {
                return Err(invalid(format!(
                    "override type does not match leaf at {path:?}"
                )));
            }
        };
        if let Some((target, value)) = target {
            if !unit(value) {
                return Err(invalid(format!("override outside 0..1 at {path:?}")));
            }
            *target = value;
        }
    }
    Ok(())
}
