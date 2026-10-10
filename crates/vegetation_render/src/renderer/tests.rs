//! Shader contract checks for production and diagnostic variants.
use crate::VegetationBladeBands;
use bevy::prelude::*;

use shader_check::{Def, Shaders};

/// The draw shader's definitions as `pipelines.rs` specializes it.
fn draw_variant(mode: VegetationBladeBands, temporal: bool, clouds: bool) -> Vec<Def> {
    let mut defs = vec![Def::Flag("SHADOW_FILTER_METHOD_HARDWARE_2X2".into(), true)];
    if clouds {
        defs.push(Def::Flag("YARRA_CLOUDS".into(), true));
        defs.push(Def::Int("MATERIAL_BIND_GROUP".into(), 2));
        defs.push(Def::Flag("ATMOSPHERE".into(), true));
    }
    if temporal {
        defs.push(Def::Flag("TEMPORAL_GRASS".into(), true));
    }
    if mode != VegetationBladeBands::Off {
        defs.push(Def::Flag("BLADE_BAND_STUDY".into(), true));
        let strength = if mode == VegetationBladeBands::Subtle {
            54
        } else {
            82
        };
        defs.push(Def::Int("BLADE_BAND_STRENGTH".into(), strength));
        if matches!(
            mode,
            VegetationBladeBands::Mask | VegetationBladeBands::MotionMask
        ) {
            defs.push(Def::Flag("BLADE_BAND_MASK".into(), true));
        }
    }
    defs
}

#[test]
fn shaders_compose_and_validate() {
    let shaders = Shaders::get();
    for compute in [
        "shaders/vegetation_schedule_compute.wesl",
        "shaders/vegetation_debug_compute.wesl",
        "shaders/vegetation_prepare_blades.wesl",
    ] {
        shaders.check(compute, &[]).unwrap();
    }
    for mode in VegetationBladeBands::ALL {
        for temporal in [false, true] {
            for clouds in [false, true] {
                let defs = draw_variant(mode, temporal, clouds);
                if let Err(error) = shaders.check("shaders/vegetation_debug_draw.wesl", &defs) {
                    panic!("{mode:?} temporal={temporal} clouds={clouds}: {error}");
                }
            }
        }
    }
}

#[test]
fn production_generation_classifies_each_candidate_once() {
    let compute = include_str!("../../../../assets/shaders/vegetation_debug_compute.wesl");
    assert_eq!(compute.matches("evaluate_candidate(").count(), 2);
    assert!(compute.contains("fn generate("));
    assert!(!compute.contains("fn count("));
    assert!(!compute.contains("fn plan("));
    assert!(!compute.contains("fn emit("));
    assert!(!compute.contains("capacity_histogram"));
}

#[test]
fn lod_uses_the_full_authored_blade_envelope() {
    let schedule = include_str!("../../../../assets/shaders/vegetation_schedule_compute.wesl");
    let compute = include_str!("../../../../assets/shaders/vegetation_debug_compute.wesl");
    let draw = concat!(
        include_str!("../../../../assets/shaders/vegetation_debug_draw.wesl"),
        include_str!("../../../../assets/shaders/vegetation_blade.wesl")
    );

    assert!(schedule.contains("fn maximum_projected_extent("));
    assert!(compute.contains("fn projected_blade_extent_pixels("));
    assert!(compute.contains("fn blade_extent_limits_pixels("));
    assert!(!draw.contains("fn projected_blade_extent_pixels("));
    assert!(!draw.contains("fn blade_extent_limits_pixels("));
    assert!(compute.contains("evaluation.lod_morph"));
    assert!(draw.contains("let lod_morph = f32((instance.geometry.y"));
    assert!(compute.contains("let maximum_reach = bitcast<f32>(choice.metadata.z)"));
    assert!(compute.contains("maximum_height * camera.wind.z * 1.65"));
    assert!(compute.contains("fn generated_candidate_height("));
    assert!(compute.contains("fn candidate_topology_class("));
    assert!(compute.contains("candidate.seed & 0x00ffffffu"));
    assert!(compute.contains("choice.packing.y, choice.packing.z"));
    assert!(compute.contains("topology_class * 2u + lod"));
    assert!(draw.contains("let seed = instance.geometry.w & 0x00ffffffu;"));
    assert!(draw.contains("let source_height_coordinate = mix("));
    assert!(draw.contains("let height_exponent = exp2(-2.0 * profile.height_packing.x);"));
    assert!(draw.contains("blade_count = select(1u, 2u, height <= profile.height_packing.y);"));
    assert!(schedule.contains("fn maximum_projected_population_spacing("));
    assert!(compute.contains("fn projected_population_spacing_pixels("));
    assert!(compute.contains("fn population_lod_density("));
    assert!(compute.contains("fn balanced_population_lod_density("));
    assert!(compute.contains("fn mobile_population_lod_density("));
    assert!(compute.contains("MOBILE_POPULATION_LOD_BIT"));
    assert!(compute.contains("FORCE_LOW_TOPOLOGY_BIT"));
    assert!(compute.contains("fn population_lod_retention_limit("));
    assert!(compute.contains("debug_config.values.y == DENSITY_MODE_BALANCED"));
    assert!(schedule.contains("debug_config.values.y != 0u"));
    assert!(draw.contains("let population_density = f32(instance.geometry.w >> 24u) / 255.0;"));
    assert!(draw.contains("let density_width = select("));
    assert!(draw.contains("let authored_half_width = mix("));
    assert!(draw.contains("let projected_authored_half_width = authored_half_width"));
    assert!(draw.contains("FAR_WIDTH_TARGET_HALF_PIXELS"));
    assert!(draw.contains("FAR_WIDTH_MAXIMUM_SCALE"));
    assert!(draw.contains("LOW_LOD_COVERAGE_WIDTH_EXPONENT"));
    assert!(draw.contains("let low_lod_coverage_scale = blade.topology.z;"));
    assert!(draw.contains("half_width * far_width_scale * taper"));
    assert!(draw.contains("let half_band = BALANCED_DENSITY_FADE_BAND * 0.5;"));
    assert!(!draw.contains("let density_scale = select("));
    assert!(compute.contains("let staggered_high_radius = bounded_high_radius * mix("));
    assert!(!draw.contains("let staggered_high_radius = bounded_high_radius * mix("));
    assert!(draw.contains("local_ribbon_side = normalize3_or("));
    assert!(draw.contains("let signed_alignment = dot("));
    assert!(draw.contains("let opening_tangent = min("));
    assert!(draw.contains("var rendered_ribbon_side = local_ribbon_side;"));
    assert!(draw.contains("rendered_ribbon_side = normalize3_or("));
    assert!(draw.contains("output.world_normal = physical_normal;"));
    assert!(draw.contains("output.ribbon_side_rounding = vec4<f32>("));
    assert!(!draw.contains("let view_opening_weight = smoothstep("));
    assert!(!draw.contains("cross(rendered_ribbon_side, curve_tangent)"));
    assert!(draw.contains("dot(input.world_normal, view_direction) >= 0.0"));
    assert!(!draw.contains("@builtin(front_facing)"));
    assert!(!draw.contains("fn apply_edge_on_fullness("));
    assert!(!draw.contains("let view_fullness = mix(1.0, 1.24"));
    assert!(!compute.contains("fn projected_height_pixels("));
    assert!(!schedule.contains("fn maximum_projected_height("));
}

#[test]
fn production_draw_uses_exposure_aware_rounded_gloss_and_shadow_reception() {
    let draw = include_str!("../../../../assets/shaders/vegetation_debug_draw.wesl");
    assert!(draw.contains("shadows::fetch_directional_shadow("));
    assert!(draw.contains("camera.sun_direction.xyz"));
    assert!(draw.contains("let received_shadow = mix("));
    assert!(draw.contains("let shadow_floor = mix(0.16, 0.42, ambient_occlusion);"));
    assert!(draw.contains("lighting as pbr_lighting"));
    assert!(draw.contains("view_bindings::view.exposure"));
    assert!(draw.contains("fn stable_clump_normal("));
    assert!(draw.contains("fn analytic_rounded_normal("));
    assert!(draw.contains("fn ggx_foliage_specular("));
    assert!(draw.contains("let shading_normal = normalize3_or("));
    assert!(draw.contains("mix(blade_normal, clump_normal, distance_stability)"));
    assert!(draw.contains("let filtered_alpha_roughness = clamp("));
    assert!(draw.contains("let filtered_broad_specular = ggx_foliage_specular("));
    assert!(draw.contains("local_sheen_specular = ggx_foliage_specular("));
    assert!(draw.contains("let local_sheen_weight = 1.0 - smoothstep("));
    assert!(draw.contains("let broad_specular_weight = mix(0.16, 0.26, distance_stability);"));
    assert!(draw.contains("local_sheen_specular * local_sheen_weight"));
    assert!(draw.contains("dot(diffuse_normal, light_direction)"));
    assert!(draw.contains("let upper_ribbon = smoothstep("));
    assert!(draw.contains("let far_highlight_weight = mix("));
    assert!(draw.contains("debug_config.values.z == LIGHTING_MODE_LEGACY"));
    assert!(draw.contains("debug_config.values.z == LIGHTING_MODE_UNLIT_DIAGNOSTIC"));
    assert!(draw.contains("debug_config.values.z == LIGHTING_MODE_VERTEX_ONLY_DIAGNOSTIC"));
}

#[test]
fn strong_wind_deforms_one_shared_curve_and_expands_visibility_bounds() {
    let schedule = include_str!("../../../../assets/shaders/vegetation_schedule_compute.wesl");
    let compute = include_str!("../../../../assets/shaders/vegetation_debug_compute.wesl");
    let draw = concat!(
        include_str!("../../../../assets/shaders/vegetation_debug_draw.wesl"),
        include_str!("../../../../assets/shaders/vegetation_blade.wesl")
    );

    assert!(schedule.contains("item.bounds.x * camera.wind.z * 1.65"));
    assert!(compute.contains("maximum_height * camera.wind.z * 1.65"));
    assert!(draw.contains("let broad_wave = sin("));
    assert!(draw.contains("let gust_wave = sin("));
    assert!(draw.contains("1.0 - smoothstep(24.0, 72.0, camera_distance)"));
    assert!(draw.contains("let mixed_phase = mix(clump_phase, blade_phase, 0.72);"));
    assert!(draw.contains("let forward_phase = blade_wind_phase + t * 3.20;"));
    assert!(draw.contains("p1 += coherent_push * 0.06;"));
    assert!(draw.contains("p2 += coherent_push * 0.58"));
    assert!(draw.contains("p3 += coherent_push + bob_offset"));
    assert!(draw.contains("bob * 0.072"));
    assert!(draw.contains("sin(side_phase) * 0.76"));
}
