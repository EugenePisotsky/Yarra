//! Shader contract checks for production and diagnostic variants.
use crate::VegetationBladeBands;
use bevy::prelude::*;

// Standalone validation does not run Bevy's import/define preprocessor.
fn preprocess_variant(source: &str, mode: VegetationBladeBands, temporal: bool) -> String {
    let mut enabled = vec![true];
    let mut output = String::new();
    for line in source.lines() {
        match line {
            "#ifdef TEMPORAL_GRASS" => enabled.push(temporal),
            "#ifdef ATMOSPHERE" | "#ifdef YARRA_CLOUDS" => enabled.push(false),
            "#ifdef BLADE_BAND_STUDY" => enabled.push(mode != VegetationBladeBands::Off),
            "#ifdef BLADE_BAND_MASK" => enabled.push(matches!(
                mode,
                VegetationBladeBands::Mask | VegetationBladeBands::MotionMask
            )),
            "#else" => {
                let last = enabled.last_mut().unwrap();
                *last = !*last;
            }
            "#endif" => {
                enabled.pop();
            }
            _ if enabled.iter().all(|v| *v) => {
                output.push_str(line);
                output.push('\n');
            }
            _ => {}
        }
    }
    assert_eq!(enabled, vec![true]);
    output.replace(
        "#{BLADE_BAND_STRENGTH}",
        if mode == VegetationBladeBands::Subtle {
            "54"
        } else {
            "82"
        },
    )
}

#[test]
fn shaders_parse_as_wgsl() {
    for source in [
        include_str!("../../../../assets/shaders/vegetation_schedule_compute.wgsl"),
        include_str!("../../../../assets/shaders/vegetation_debug_compute.wgsl"),
    ] {
        let module = naga::front::wgsl::parse_str(source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    let preparation = include_str!("../../../../assets/shaders/vegetation_prepare_blades.wgsl");
    let preparation = format!(
        "{}\n{}",
        include_str!("../../../../assets/shaders/vegetation_blade.wgsl"),
        &preparation[preparation.find("struct DrawArgs").unwrap()..]
    );
    let module = naga::front::wgsl::parse_str(&preparation).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();

    // Naga's standalone WGSL parser does not run Bevy's #import preprocessor. Validate the
    // complete draw shader with only the imported CSM adapter replaced by an identity stub.
    let draw = include_str!("../../../../assets/shaders/vegetation_debug_draw.wgsl");
    let declarations = draw.find("// Vegetation V2 procedural").unwrap();
    let shadow_adapter = draw.find("fn directional_shadow_visibility").unwrap();
    let post_adapter = draw.find("fn radiance_tint").unwrap();
    let sanitized = format!(
        "{}\n{}fn directional_shadow_visibility(_input: VertexOutput) -> f32 {{ return 1.0; }}\n{}",
        include_str!("../../../../assets/shaders/vegetation_blade.wgsl"),
        format!(
            "{}\n{}",
            include_str!("../../../../assets/shaders/grass_canopy.wgsl"),
            &draw[declarations..shadow_adapter]
        ),
        &draw[post_adapter..],
    )
    .replace("pbr_lighting::D_GGX", "test_d_ggx")
    .replace(
        "pbr_lighting::V_SmithGGXCorrelated",
        "test_v_smith_ggx_correlated",
    )
    .replace("view_bindings::view.exposure", "1.0");
    let sanitized = format!(
        "fn test_d_ggx(_roughness: f32, _n_dot_h: f32) -> f32 {{ return 1.0; }}\n\
         fn test_v_smith_ggx_correlated(_roughness: f32, _n_dot_v: f32, _n_dot_l: f32) -> f32 {{ return 1.0; }}\n\
         {sanitized}"
    );
    for (mode, temporal) in VegetationBladeBands::ALL
        .into_iter()
        .flat_map(|m| [(m, false), (m, true)])
    {
        let variant = preprocess_variant(&sanitized, mode, temporal);
        let module = naga::front::wgsl::parse_str(&variant).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}

#[test]
fn production_generation_classifies_each_candidate_once() {
    let compute = include_str!("../../../../assets/shaders/vegetation_debug_compute.wgsl");
    assert_eq!(compute.matches("evaluate_candidate(").count(), 2);
    assert!(compute.contains("fn generate("));
    assert!(!compute.contains("fn count("));
    assert!(!compute.contains("fn plan("));
    assert!(!compute.contains("fn emit("));
    assert!(!compute.contains("capacity_histogram"));
}

#[test]
fn lod_uses_the_full_authored_blade_envelope() {
    let schedule = include_str!("../../../../assets/shaders/vegetation_schedule_compute.wgsl");
    let compute = include_str!("../../../../assets/shaders/vegetation_debug_compute.wgsl");
    let draw = concat!(
        include_str!("../../../../assets/shaders/vegetation_debug_draw.wgsl"),
        include_str!("../../../../assets/shaders/vegetation_blade.wgsl")
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
    let draw = include_str!("../../../../assets/shaders/vegetation_debug_draw.wgsl");
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
    let schedule = include_str!("../../../../assets/shaders/vegetation_schedule_compute.wgsl");
    let compute = include_str!("../../../../assets/shaders/vegetation_debug_compute.wgsl");
    let draw = concat!(
        include_str!("../../../../assets/shaders/vegetation_debug_draw.wgsl"),
        include_str!("../../../../assets/shaders/vegetation_blade.wgsl")
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
