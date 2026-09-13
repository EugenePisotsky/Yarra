@group(0) @binding(0) var<storage, read_write> comparison: array<vec4<f32>>;

@compute @workgroup_size(64)
fn compare_lod(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if (index >= 99u) { return; }
    let density = array<f32, 3>(0.30, 0.55, 1.0)[index / 33u];
    let rank = f32(index % 33u) / 32.0;
    var profile: Species;
    profile.bounds = vec4(0.35, 0.65, 0.008, 0.020);
    profile.topology = vec4(5.0, 2.0, 2.0, 0.92);
    profile.shape = vec4(1.45, 1.50, 0.0, 0.0);
    profile.shape_secondary = vec4(0.08, 0.45, 1.0, 0.65);
    profile.curve_variant_a = vec4(0.15, 0.40, -0.20, 0.20);
    profile.curve_variant_b = vec4(0.25, 0.40, -0.22, 0.18);
    profile.group_response = vec4(0.20, 0.12, 0.40, 0.0);
    profile.height_packing = vec4(0.0, 0.65, 12.0, 12.0);
    var camera: Camera;
    camera.projection.w = 1.0;
    var config: DebugConfig;
    config.values.y = DENSITY_MODE_BALANCED;
    var instance: ProceduralInstance;
    instance.root_clump.w = bitcast<f32>(pack2x16unorm(vec2(0.37, rank)));
    instance.geometry.x = pack2x16snorm(vec2(1.0, 0.0));
    instance.geometry.w = 12345u | (u32(round(density * 255.0)) << 24u);
    let high = prepare_blade(instance, profile, camera, config, index & 1u);
    instance.geometry.y = 0x80000000u;
    let low = prepare_blade(instance, profile, camera, config, index & 1u);
    instance.geometry.y = LOD_MORPH_MASK << LOD_MORPH_SHIFT;
    let full = prepare_blade(instance, profile, camera, config, index & 1u);
    var endpoint_error = max(length(high.p0_width.xyz - low.p0_width.xyz),
        length(high.p3_amplitude.xyz - low.p3_amplitude.xyz));
    let high_shoulder = vec3(high.side.w, high.wind_forward.w, high.topology.w);
    let low_shoulder = vec3(low.side.w, low.wind_forward.w, low.topology.w);
    endpoint_error = max(endpoint_error, length(high_shoulder - low_shoulder));
    comparison[index] = vec4(high.p0_width.w * high.topology.z,
        low.p0_width.w * low.topology.z, full.p0_width.w, endpoint_error);
}
