// Editor-only artistic ground-material experiment. No dynamic shadow visibility.
struct StudyGroundSettings {
    bounds: vec4<f32>, // world min xz, reciprocal world extent xz
    controls: vec4<f32>, // mode, darken-only mean, inverse detail tile metres, reserved
    canopy_appearance: vec4<f32>, canopy_shape: vec4<f32>,
    canopy_distance: vec4<f32>, canopy_origin: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> study_ground: StudyGroundSettings;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var study_coverage: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var study_coverage_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var study_detail: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var study_detail_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var study_canopy: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var study_canopy_sampler: sampler;

fn study_canopy_visibility(world_position: vec3<f32>) -> f32 {
    let world_xz = world_position.xz;
    if study_ground.controls.x < 3.5 { return 1.0; }
    let uv = (world_xz - study_ground.bounds.xy) * study_ground.bounds.zw;
    let inside = all(uv >= vec2(0.0)) && all(uv <= vec2(1.0));
    let cover = select(vec2(0.0), textureSample(study_canopy, study_canopy_sampler, uv).rg, inside);
    // Source coverage gates the same patch/height response used on the lower blades.
    let visibility = canopy_visibility_at(world_xz, 0.0,
        distance(canopy_view::view.world_position, world_position),
        study_ground.canopy_appearance, study_ground.canopy_shape,
        study_ground.canopy_distance, study_ground.canopy_origin, study_ground.canopy_appearance.y,
        cover.g * 4.0, distance(canopy_view::view.world_position.xz, world_xz));
    return mix(1.0, visibility, cover.r);
}

fn study_ground_coverage(world_xz: vec2<f32>) -> f32 {
    let uv = (world_xz - study_ground.bounds.xy) * study_ground.bounds.zw;
    let inside = all(uv >= vec2(0.0)) && all(uv <= vec2(1.0));
    return select(0.0, textureSample(study_coverage, study_coverage_sampler, uv).r, inside);
}

fn study_ground_multiplier(world_xz: vec2<f32>, coverage: f32) -> vec3<f32> {
    var factor = study_ground.controls.y;
    if study_ground.controls.x > 1.5 {
        // Nonlinear cavity response was baked BEFORE mip generation. Distant mips
        // retain average darkness rather than applying a power to an averaged AO.
        factor = textureSample(study_detail, study_detail_sampler,
            world_xz * study_ground.controls.z).r;
    }
    return mix(vec3(1.0), vec3(0.82, 0.90, 0.72) * factor, coverage);
}
