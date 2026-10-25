//! Shader contract checks: every variant the renderer builds composes and validates, and the
//! composed modules keep the structure and values the Rust side relies on.
use super::gpu_types::shader_defs;
use bevy::shader::ShaderDefVal;
use naga::{Module, Statement};
use shader_check::{Def, Shaders};

const COMPUTE_SHADERS: [&str; 4] = [
    "shaders/vegetation/schedule.wesl",
    "shaders/vegetation/placement.wesl",
    "shaders/vegetation/candidate_cache.wesl",
    "shaders/vegetation/pose.wesl",
];

const DRAW_SHADER: &str = "shaders/vegetation/draw.wesl";
const LIGHTING_SHADER: &str = "shaders/vegetation/deferred.wesl";

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

/// The draw shader's definitions as `pipelines.rs` specializes it, with and without motion.
fn draw_variant(motion: bool) -> Vec<Def> {
    let mut defs = grass_defs();
    defs.push(Def::Flag("ENVIRONMENT_SURFACE".into(), true));
    defs.push(Def::Int("MATERIAL_BIND_GROUP".into(), 2));

    if motion {
        defs.push(Def::Flag("TEMPORAL_GRASS".into(), true));
    }

    defs
}

/// The grass lighting kernel's definitions as `deferred.rs` specializes it.
fn lighting_variant() -> Vec<Def> {
    let mut defs = grass_defs();

    for flag in [
        "SHADOW_FILTER_METHOD_HARDWARE_2X2",
        "ENVIRONMENT_SURFACE",
        "DEPTH_PREPASS",
        "DEFERRED_PREPASS",
        "ATMOSPHERE",
    ] {
        defs.push(Def::Flag(flag.into(), true));
    }

    defs.push(Def::Int("MATERIAL_BIND_GROUP".into(), 2));
    defs
}

fn draw_modules() -> impl Iterator<Item = Module> {
    [false, true]
        .into_iter()
        .map(|motion| composed(DRAW_SHADER, &draw_variant(motion)))
}

/// A composed and validated vegetation shader.
pub(super) fn composed(shader: &str, defs: &[Def]) -> Module {
    let wgsl = Shaders::get()
        .check(shader, defs)
        .unwrap_or_else(|error| panic!("{error}"));
    shader_check::validate(&wgsl).unwrap()
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

    draw_modules().for_each(drop);
    composed(LIGHTING_SHADER, &lighting_variant());
}

#[test]
fn the_gbuffer_pass_binds_no_view_group() {
    // The pass holds the G-buffer as attachments, and the view group binds it for reading
    // (`deferred.rs` binds an empty group in its place).
    for module in draw_modules() {
        assert!(
            module
                .global_variables
                .iter()
                .all(|(_, global)| global.binding.as_ref().is_none_or(|b| b.group != 0)),
            "a G-buffer draw binds group 0"
        );
    }
}

#[test]
fn checked_variants_carry_the_renderer_definitions() {
    let jobs = shader_check::entries::load(Shaders::get()).unwrap();
    let grass = grass_defs();

    for shader in COMPUTE_SHADERS
        .into_iter()
        .chain([DRAW_SHADER, LIGHTING_SHADER])
    {
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
fn blades_bend_once_before_drawing() {
    // The pose pass bends each blade by trampling and the wind; every vertex only reads it.
    let named = |module: &Module, name: &str| {
        module
            .functions
            .iter()
            .any(|(_, f)| f.name.as_deref().is_some_and(|n| n.ends_with(name)))
    };
    let pose = composed("shaders/vegetation/pose.wesl", &grass_defs());
    assert!(named(&pose, "wind_curve") && named(&pose, "trample_blade"));

    for draw in draw_modules() {
        assert!(!named(&draw, "wind_curve") && !named(&draw, "trample_blade"));
    }
}

#[test]
fn drawing_reuses_the_placement_lod() {
    // The draw reads each instance's LOD bin and morph; it never projects blades again.
    for module in draw_modules() {
        for (_, function) in module.functions.iter() {
            let name = function.name.as_deref().unwrap_or_default();
            assert!(
                !name.ends_with("projected_blade_extent_pixels"),
                "the draw shader calls {name}"
            );
        }
    }
}
