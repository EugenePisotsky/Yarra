#import bevy_pbr::{
    decal::clustered::apply_decals,
    forward_io::{FragmentOutput, VertexOutput},
    pbr_fragment::pbr_input_from_vertex_output,
    pbr_functions::{
        apply_pbr_lighting,
        calculate_tbn_mikktspace,
        main_pass_post_lighting_processing,
    },
    pbr_types,
}

struct TerrainMaterialSettings {
    chunk_minimum: vec2<f32>,
    chunk_extent: vec2<f32>,
    surface_layers: vec4<f32>,
    tile_sizes: vec4<f32>,
    normal_settings: vec4<f32>,
    roughness_ranges: vec4<f32>,
    macro_scales: vec4<f32>,
    macro_settings: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> settings: TerrainMaterialSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var weight_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var weight_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var base_color_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var surface_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var normal_material_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var macro_variation_texture: texture_2d<f32>;

fn hash_2d(point: vec2<f32>) -> f32 {
    var value = fract(vec3(point.x, point.y, point.x) * 0.1031);
    value += dot(value, value.yzx + vec3(33.33));
    return fract((value.x + value.y) * value.z);
}

struct StochasticUvPlan {
    uv_0: vec2<f32>,
    uv_1: vec2<f32>,
    uv_2: vec2<f32>,
    dx_0: vec2<f32>,
    dx_1: vec2<f32>,
    dx_2: vec2<f32>,
    dy_0: vec2<f32>,
    dy_1: vec2<f32>,
    dy_2: vec2<f32>,
    weights: vec3<f32>,
}

fn quarter_turn(value: vec2<f32>, turn: f32) -> vec2<f32> {
    if turn < 0.5 {
        return value;
    }
    if turn < 1.5 {
        return vec2(-value.y, value.x);
    }
    if turn < 2.5 {
        return -value;
    }
    return vec2(value.y, -value.x);
}

fn stochastic_vertex_turn(vertex: vec2<f32>, material_layer: i32) -> f32 {
    let layer_seed = f32(material_layer) * 19.19;
    return floor(hash_2d(vertex + vec2(layer_seed, layer_seed * 0.37)) * 4.0);
}

fn stochastic_vertex_offset(vertex: vec2<f32>, material_layer: i32) -> vec2<f32> {
    let layer_seed = f32(material_layer) * 23.71;
    let seeded = vertex + vec2(layer_seed, -layer_seed * 0.41);
    return vec2(
        hash_2d(seeded + vec2(17.0, 3.0)),
        hash_2d(seeded + vec2(5.0, 29.0)),
    ) * 31.0;
}

fn make_stochastic_uv_plan(
    uv: vec2<f32>,
    material_layer: i32,
    uv_dx: vec2<f32>,
    uv_dy: vec2<f32>,
) -> StochasticUvPlan {
    // A triangular lattice gives each stable random vertex a hexagonal
    // influence region. Shaped barycentric weights preserve local contrast
    // instead of averaging three samples into a uniformly muddy surface.
    let lattice = vec2(uv.x + uv.y * 0.5773502692, uv.y * 1.1547005384);
    let cell = floor(lattice);
    let local = fract(lattice);
    var vertex_0: vec2<f32>;
    var vertex_1: vec2<f32>;
    var vertex_2: vec2<f32>;
    var raw_weights: vec3<f32>;
    if local.x + local.y <= 1.0 {
        vertex_0 = cell;
        vertex_1 = cell + vec2(1.0, 0.0);
        vertex_2 = cell + vec2(0.0, 1.0);
        raw_weights = vec3(1.0 - local.x - local.y, local.x, local.y);
    } else {
        vertex_0 = cell + vec2(1.0, 1.0);
        vertex_1 = cell + vec2(0.0, 1.0);
        vertex_2 = cell + vec2(1.0, 0.0);
        raw_weights = vec3(local.x + local.y - 1.0, 1.0 - local.x, 1.0 - local.y);
    }
    let squared = max(raw_weights * raw_weights, vec3(0.0));
    let shaped = squared * squared;
    let weights = shaped / max(shaped.x + shaped.y + shaped.z, 0.000001);
    let turn_0 = stochastic_vertex_turn(vertex_0, material_layer);
    let turn_1 = stochastic_vertex_turn(vertex_1, material_layer);
    let turn_2 = stochastic_vertex_turn(vertex_2, material_layer);
    var plan: StochasticUvPlan;
    plan.uv_0 = quarter_turn(uv, turn_0) + stochastic_vertex_offset(vertex_0, material_layer);
    plan.uv_1 = quarter_turn(uv, turn_1) + stochastic_vertex_offset(vertex_1, material_layer);
    plan.uv_2 = quarter_turn(uv, turn_2) + stochastic_vertex_offset(vertex_2, material_layer);
    plan.dx_0 = quarter_turn(uv_dx, turn_0);
    plan.dx_1 = quarter_turn(uv_dx, turn_1);
    plan.dx_2 = quarter_turn(uv_dx, turn_2);
    plan.dy_0 = quarter_turn(uv_dy, turn_0);
    plan.dy_1 = quarter_turn(uv_dy, turn_1);
    plan.dy_2 = quarter_turn(uv_dy, turn_2);
    plan.weights = weights;
    return plan;
}

fn sample_base_color_plain(
    uv: vec2<f32>,
    material_layer: i32,
    uv_dx: vec2<f32>,
    uv_dy: vec2<f32>,
) -> vec4<f32> {
    return textureSampleGrad(
        base_color_array,
        surface_sampler,
        uv,
        material_layer,
        uv_dx,
        uv_dy,
    );
}

fn sample_base_color_stochastic(
    plan: StochasticUvPlan,
    material_layer: i32,
) -> vec4<f32> {
    return sample_base_color_plain(plan.uv_0, material_layer, plan.dx_0, plan.dy_0)
        * plan.weights.x
        + sample_base_color_plain(plan.uv_1, material_layer, plan.dx_1, plan.dy_1)
        * plan.weights.y
        + sample_base_color_plain(plan.uv_2, material_layer, plan.dx_2, plan.dy_2)
        * plan.weights.z;
}

fn decode_octahedral_normal(sampled: vec2<f32>, y_sign: f32) -> vec3<f32> {
    let encoded = sampled * 2.0 - 1.0;
    var normal = vec3(encoded.x, encoded.y, 1.0 - abs(encoded.x) - abs(encoded.y));
    let fold = clamp(-normal.z, 0.0, 1.0);
    normal.x += select(fold, -fold, normal.x >= 0.0);
    normal.y += select(fold, -fold, normal.y >= 0.0);
    normal = normalize(normal);
    normal.y *= y_sign;
    return normal;
}

struct SurfaceSample {
    base_color: vec4<f32>,
    tangent_normal: vec3<f32>,
    ambient_occlusion: f32,
    roughness: f32,
}

fn sample_surface(
    world_xz: vec2<f32>,
    tile_size: f32,
    layer: i32,
    normal_y_sign: f32,
    normal_strength: f32,
    roughness_min: f32,
    roughness_max: f32,
    anti_tiling: bool,
) -> SurfaceSample {
    let uv = world_xz / max(tile_size, 0.001);
    let uv_dx = dpdx(uv);
    let uv_dy = dpdy(uv);
    var base = sample_base_color_plain(uv, layer, uv_dx, uv_dy);
    if anti_tiling {
        base = sample_base_color_stochastic(
            make_stochastic_uv_plan(uv, layer, uv_dx, uv_dy),
            layer,
        );
    }
    let packed = textureSampleGrad(
        normal_material_array,
        surface_sampler,
        uv,
        layer,
        uv_dx,
        uv_dy,
    );
    let decoded = decode_octahedral_normal(packed.rg, normal_y_sign);
    var result: SurfaceSample;
    result.base_color = base;
    result.tangent_normal = normalize(vec3(
        decoded.xy * clamp(normal_strength, 0.0, 1.0),
        max(decoded.z, 0.001),
    ));
    result.ambient_occlusion = packed.b;
    result.roughness = mix(
        clamp(roughness_min, 0.0, 1.0),
        clamp(roughness_max, roughness_min, 1.0),
        packed.a,
    );
    return result;
}

fn macro_signal(world_xz: vec2<f32>) -> f32 {
    let enabled = settings.macro_settings.y;
    if enabled < 0.5 {
        return 0.0;
    }
    let medium_world = vec2(
        world_xz.x * 0.819 - world_xz.y * 0.574,
        world_xz.x * 0.574 + world_xz.y * 0.819,
    );
    let medium = textureSample(
        macro_variation_texture,
        surface_sampler,
        medium_world / max(settings.macro_scales.y, 0.001) + vec2(0.37, 0.61),
    ).r;
    let large_world = vec2(
        world_xz.x * -0.342 - world_xz.y * 0.940,
        world_xz.x * 0.940 - world_xz.y * 0.342,
    );
    let small = textureSample(
        macro_variation_texture,
        surface_sampler,
        world_xz / max(settings.macro_scales.x, 0.001) + vec2(0.11, 0.73),
    ).r;
    let large = textureSample(
        macro_variation_texture,
        surface_sampler,
        large_world / max(settings.macro_scales.z, 0.001) + vec2(0.83, 0.19),
    ).r;
    let small_signal = (small - 0.5) * 2.0;
    let medium_signal = (medium - 0.5) * 2.0;
    let large_signal = (large - 0.5) * 2.0;
    return small_signal * 0.28 + medium_signal * 0.36 + large_signal * 0.36;
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_vertex_output(in, is_front, false);
    var blend = vec2(1.0, 0.0);
    if settings.surface_layers.z > 1.5 {
        let control_uv = clamp(
            (in.world_position.xz - settings.chunk_minimum) / settings.chunk_extent,
            vec2(0.0),
            vec2(1.0),
        );
        let dimensions = vec2<f32>(textureDimensions(weight_map));
        let weight_uv = (vec2(0.5) + control_uv * (dimensions - vec2(1.0))) / dimensions;
        blend = max(textureSample(weight_map, weight_sampler, weight_uv).rg, vec2(0.0));
        blend /= max(blend.x + blend.y, 0.000001);
    }

    let first = sample_surface(
        in.world_position.xz,
        settings.tile_sizes.x,
        i32(settings.surface_layers.x + 0.5),
        settings.normal_settings.x,
        settings.normal_settings.y,
        settings.roughness_ranges.x,
        settings.roughness_ranges.y,
        settings.macro_settings.z >= 0.5,
    );
    var base = first.base_color;
    var tangent_normal = first.tangent_normal;
    var ambient_occlusion = first.ambient_occlusion;
    var roughness = first.roughness;
    if settings.surface_layers.z > 1.5 {
        let second = sample_surface(
            in.world_position.xz,
            settings.tile_sizes.y,
            i32(settings.surface_layers.y + 0.5),
            settings.normal_settings.z,
            settings.normal_settings.w,
            settings.roughness_ranges.z,
            settings.roughness_ranges.w,
            settings.macro_settings.w >= 0.5,
        );
        base = first.base_color * blend.x + second.base_color * blend.y;
        tangent_normal = normalize(
            first.tangent_normal * blend.x + second.tangent_normal * blend.y,
        );
        ambient_occlusion = first.ambient_occlusion * blend.x
            + second.ambient_occlusion * blend.y;
        roughness = first.roughness * blend.x + second.roughness * blend.y;
    }

    let variation = clamp(
        macro_signal(in.world_position.xz) * settings.macro_settings.x,
        -1.0,
        1.0,
    );
    let macro_response = variation * settings.macro_scales.w;
    base = vec4(
        clamp(base.rgb * exp2(macro_response), vec3(0.0), vec3(1.0)),
        1.0,
    );
    let geometry_normal = normalize(in.world_normal);
    let tbn = calculate_tbn_mikktspace(geometry_normal, in.world_tangent);
    pbr_input.N = normalize(tbn * tangent_normal);
    pbr_input.clearcoat_N = pbr_input.N;
    pbr_input.material.base_color = base;
    pbr_input.material.perceptual_roughness = clamp(roughness, 0.08, 1.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.25);
    pbr_input.diffuse_occlusion = vec3(clamp(ambient_occlusion, 0.0, 1.0));
    pbr_input.specular_occlusion = clamp(ambient_occlusion, 0.0, 1.0);
    pbr_input.material.flags = pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    apply_decals(&pbr_input);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
