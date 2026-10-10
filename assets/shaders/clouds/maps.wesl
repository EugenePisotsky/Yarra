// Lookups into the environment's maps: the cloud layer's shadow, rain shelter and the forest map.
// Materials bind the maps in their material group (clouds/surface.wesl, clouds/forest_shadow.wesl)
// and passes in their own groups, so each lookup takes its map and the placement it needs.

// The cloud layer's shadow map (atmosphere/src/clouds) where a ray from `p` towards `direction`
// meets the layer's base: r the sun's direct beam the clouds let through, g the moon's. `layer`
// and `offset` are the cloud parameters of the same names.
fn cloud_shadow_texel(map: texture_2d<f32>, map_sampler: sampler, layer: vec4<f32>,
    offset: vec4<f32>, direction: vec3<f32>, p: vec3<f32>) -> vec4<f32> {
    let hit = p.xz + offset.xy + direction.xz * (layer.x - p.y) / max(direction.y, 0.04);
    return textureSampleLevel(map, map_sampler, (hit - offset.zw) / (layer.z * 4.0), 0.0);
}

// The sun's direct beam past the cloud layer at `p`: 1 above the layer, without one or with the
// sun down. Light the clouds pass on diffusely lights no beams, and a closing deck (`closed`, 0
// open to 1 closed) hides the beam whatever thin spots the shadow map finds.
fn cloud_beam(map: texture_2d<f32>, map_sampler: sampler, layer: vec4<f32>, offset: vec4<f32>,
    sun: vec3<f32>, closed: f32, p: vec3<f32>) -> f32 {
    if layer.w < 0.5 || sun.y <= 0.0 || p.y >= layer.x + layer.y {
        return 1.0;
    }
    let shadow = cloud_shadow_texel(map, map_sampler, layer, offset, sun, p).r;
    let open = 1.0 - closed;
    return shadow * open * open;
}

// Rain shelter lookup shared by materials and rain: 1 where rain reaches `p`, 0 under full
// cover. Bilinear coverage of texels whose object top lies above `p`, as on the CPU.
fn shelter_exposure(map: texture_2d<f32>, shelter: vec4<f32>, p: vec3<f32>) -> f32 {
    if shelter.w < 0.5 { return 1.0; }
    let size = vec2<i32>(textureDimensions(map));
    let texel = (p.xz - shelter.xy) / shelter.z - 0.5;
    let base = floor(texel);
    let f = texel - base;
    var cover = 0.0;
    for (var i = 0; i < 4; i += 1) {
        let offset = vec2(i & 1, i >> 1u);
        let at = vec2<i32>(base) + offset;
        if any(at < vec2(0)) || any(at >= size) { continue; }
        let value = textureLoad(map, at, 0).rg;
        let w = select(1.0 - f.x, f.x, offset.x == 1) * select(1.0 - f.y, f.y, offset.y == 1);
        cover += select(0.0, value.g * w, p.y < value.r);
    }
    return 1.0 - cover;
}

// Sky light still reaching the floor of a closed stand: scattered through and between crowns.
const SKY_FLOOR: f32 = 0.15;
// Mip level for sky light: texels of 8-16 m, about the patch of crowns a point under them sees
// the sky through. From level 1 on the forest map holds the average share of the sky the crowns
// let through and the highest canopy top (atmosphere/src/forest_shadow.rs, SKY_LEVELS).
const SKY_LEVEL: f32 = 1.5;

// Share of the sky's light reaching `p` past the crowns around and above it, scaled by
// `strength` (`forest_sky.x`); `placement` is the forest map's origin xz and metres per texel
// (`forest_shadow`). 1 without a map, or with no crown above nearby.
fn forest_sky_share(map: texture_2d<f32>, map_sampler: sampler, placement: vec4<f32>,
    strength: f32, p: vec3<f32>) -> f32 {
    if strength <= 0.0 || placement.z <= 0.0 {
        return 1.0;
    }
    let extent = placement.z * vec2<f32>(textureDimensions(map));
    let here = (p.xz - placement.xy) / extent;
    if any(here <= vec2(0.0)) || any(here >= vec2(1.0)) {
        return 1.0;
    }
    let around = textureSampleLevel(map, map_sampler, here, SKY_LEVEL);
    if around.a <= p.y {
        return 1.0;
    }
    return mix(1.0, mix(SKY_FLOOR, 1.0, around.b), strength);
}
