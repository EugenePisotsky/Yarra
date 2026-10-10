//! Shader contract checks: every variant the renderer builds composes and validates, and the
//! composed modules keep the structure and values the Rust side relies on.
use super::gpu_types::shader_defs;
use crate::{VegetationDensityMode, VegetationLightingMode};
use bevy::shader::ShaderDefVal;
use naga::{Expression, Literal, Module, Statement};
use shader_check::{Def, Shaders};

const COMPUTE_SHADERS: [&str; 4] = [
    "shaders/vegetation/schedule.wesl",
    "shaders/vegetation/placement.wesl",
    "shaders/vegetation/candidate_cache.wesl",
    "shaders/vegetation/prepare.wesl",
];
const DRAW_SHADER: &str = "shaders/vegetation/draw.wesl";

/// The definitions every vegetation pipeline passes, as the shader checker spells them.
pub(super) fn grass_defs() -> Vec<Def> {
    shader_defs()
        .into_iter()
        .map(|def| match def {
            ShaderDefVal::UInt(name, value) => Def::Int(name.into(), value.into()),
            ShaderDefVal::Int(name, value) => Def::Int(name.into(), value.into()),
            ShaderDefVal::Bool(name, on) => Def::Flag(name.into(), on),
        })
        .collect()
}

/// The draw shader's definitions as `pipelines.rs` specializes it.
fn draw_variant(temporal: bool, environment: bool) -> Vec<Def> {
    let mut defs = grass_defs();
    defs.push(Def::Flag("SHADOW_FILTER_METHOD_HARDWARE_2X2".into(), true));
    if environment {
        defs.push(Def::Flag("ENVIRONMENT_SURFACE".into(), true));
        defs.push(Def::Int("MATERIAL_BIND_GROUP".into(), 2));
        defs.push(Def::Flag("ATMOSPHERE".into(), true));
    }
    if temporal {
        defs.push(Def::Flag("TEMPORAL_GRASS".into(), true));
    }
    defs
}

/// A composed and validated vegetation shader.
pub(super) fn composed(shader: &str, defs: &[Def]) -> Module {
    let wgsl = Shaders::get()
        .check(shader, defs)
        .unwrap_or_else(|error| panic!("{error}"));
    shader_check::validate(&wgsl).unwrap()
}

/// A module-scope constant's value; composition prefixes imported names, so match the suffix.
pub(super) fn constant(module: &Module, name: &str) -> f64 {
    let (_, constant) = module
        .constants
        .iter()
        .find(|(_, c)| c.name.as_deref().is_some_and(|n| n.ends_with(name)))
        .unwrap_or_else(|| panic!("no constant {name}"));
    match module.global_expressions[constant.init] {
        Expression::Literal(Literal::U32(value)) => value.into(),
        Expression::Literal(Literal::F32(value)) => value.into(),
        ref other => panic!("{name} is not a literal: {other:?}"),
    }
}

fn calls(module: &Module, block: &naga::Block, name: &str) -> usize {
    block
        .iter()
        .map(|statement| match statement {
            Statement::Call { function, .. } => usize::from(
                module.functions[*function]
                    .name
                    .as_deref()
                    .is_some_and(|n| n.ends_with(name)),
            ),
            Statement::Block(block) => calls(module, block, name),
            Statement::If { accept, reject, .. } => {
                calls(module, accept, name) + calls(module, reject, name)
            }
            Statement::Switch { cases, .. } => {
                cases.iter().map(|c| calls(module, &c.body, name)).sum()
            }
            Statement::Loop {
                body, continuing, ..
            } => calls(module, body, name) + calls(module, continuing, name),
            _ => 0,
        })
        .sum()
}

#[test]
fn shaders_compose_and_validate() {
    for shader in COMPUTE_SHADERS {
        composed(shader, &grass_defs());
    }
    for temporal in [false, true] {
        for clouds in [false, true] {
            composed(DRAW_SHADER, &draw_variant(temporal, clouds));
        }
    }
}

#[test]
fn checked_variants_carry_the_renderer_definitions() {
    let jobs = shader_check::entries::load(Shaders::get()).unwrap();
    let grass = grass_defs();
    for shader in COMPUTE_SHADERS.into_iter().chain([DRAW_SHADER]) {
        let variants: Vec<_> = jobs.iter().filter(|(s, _)| s == shader).collect();
        assert!(!variants.is_empty(), "entries.ron does not check {shader}");
        for (_, defs) in variants {
            for def in &grass {
                assert!(defs.contains(def), "{shader}: entries.ron lacks {def:?}");
            }
        }
    }
}

#[test]
fn placement_classifies_each_candidate_once() {
    let module = composed("shaders/vegetation/placement.wesl", &grass_defs());
    let entries: Vec<_> = module
        .entry_points
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(entries, ["generate", "finalize"]);
    let generate = &module.entry_points[0].function;
    assert_eq!(calls(&module, &generate.body, "evaluate_candidate"), 1);
}

#[test]
fn drawing_reuses_the_placement_lod() {
    // The draw reads each instance's topology bin and morph; it never projects blades again.
    for temporal in [false, true] {
        let module = composed(DRAW_SHADER, &draw_variant(temporal, true));
        for (_, function) in module.functions.iter() {
            let name = function.name.as_deref().unwrap_or_default();
            assert!(
                !name.ends_with("projected_blade_extent_pixels")
                    && !name.ends_with("blade_extent_limits_pixels"),
                "the draw shader calls {name}"
            );
        }
    }
}

#[test]
fn shader_modes_match_the_renderer() {
    let placement = composed("shaders/vegetation/placement.wesl", &grass_defs());
    for (name, mode) in [
        ("DENSITY_MODE_BALANCED", VegetationDensityMode::Balanced),
        (
            "DENSITY_MODE_FULL_REFERENCE",
            VegetationDensityMode::FullReference,
        ),
    ] {
        assert_eq!(constant(&placement, name), f64::from(mode as u32));
    }
    let draw = composed(DRAW_SHADER, &draw_variant(false, false));
    for (name, mode) in [
        (
            "LIGHTING_MODE_UNLIT_DIAGNOSTIC",
            VegetationLightingMode::UnlitDiagnostic,
        ),
        (
            "LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC",
            VegetationLightingMode::VertexOnlyDiagnostic,
        ),
    ] {
        assert_eq!(constant(&draw, name), f64::from(mode as u32));
    }
}
