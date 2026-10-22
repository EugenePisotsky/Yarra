// Lookups into the environment's maps: the cloud layer's shadow, rain shelter, the forest map and
// the ground albedo map.
// Materials bind the maps in their material group (lighting/surface.wesl,
// lighting/forest_shadow.wesl) and passes in their own groups, so each lookup takes its map and
// the placement it needs.

// The cloud layer's shadow map (atmosphere/src/clouds) where a ray from `p` towards `direction`
// meets the layer's base: r the sun's direct beam the clouds let through, g the moon's. `layer`
// and `offset` are the environment parameters of the same names.
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

// The ground around the camera, seen from above (crates/atmosphere/src/ground_albedo.rs, filled
// by terrain/ground_albedo.wesl) at render XZ `p`: its albedo, and the cosine at which the sun's
// beam meets it past the terrain's horizon. `near` and `far` place its layers (`ground_near`,
// `ground_far`); the near layer lies over the far one and fades into it at its edge, and ground
// neither holds is `open`.
fn ground_texel(map: texture_2d_array<f32>, map_sampler: sampler, near: vec4<f32>,
    far: vec4<f32>, p: vec2<f32>, open: vec4<f32>) -> vec4<f32> {
    return ground_layers(map, map_sampler, near, far, p, 0u, open);
}

// The ground the map draws at render XZ `p`: its height, the top of the snow where there is some,
// and the share of the sky the terrain around leaves it; `open` and the whole sky where neither
// layer holds it.
fn ground_relief(map: texture_2d_array<f32>, map_sampler: sampler, near: vec4<f32>,
    far: vec4<f32>, p: vec2<f32>, open: f32) -> vec2<f32> {
    let relief = ground_layers(map, map_sampler, near, far, p, 2u, vec4(open - near.w, 1.0, 0.0,
        0.0));
    return vec2(relief.r + near.w, relief.g);
}

// Snow country at render XZ `p`: 1 where the ground map's far layer, each of its 32 m texels the
// mean of the ground it covers, shows mostly snow, so bare patches within snowfields count and
// broad meadows do not; 0 beyond the map.
fn snow_country(map: texture_2d_array<f32>, map_sampler: sampler, near: vec4<f32>,
    far: vec4<f32>, p: vec2<f32>) -> f32 {
    if near.z <= 0.0 {
        return 0.0;
    }
    let size = f32(textureDimensions(map).x);
    let weight = ground_layer_weight(p, far, size, 1.0);
    let albedo = ground_layer_texel(map, map_sampler, p, far, size, 1u, weight).rgb;
    return weight * smoothstep(SNOW_COUNTRY.x, SNOW_COUNTRY.y,
        dot(albedo, vec3(0.2126, 0.7152, 0.0722)));
}

// Mean albedo over which ground counts as snow country.
const SNOW_COUNTRY: vec2<f32> = vec2(0.3, 0.55);

// Fresh snow's albedo.
const FRESH_SNOW: vec3<f32> = vec3(0.8, 0.82, 0.86);

// Share of the ground at render XZ `p` fresh snow covers on average: `fresh` (the environment's
// weather.w) in snow country.
fn fresh_snow_share(map: texture_2d_array<f32>, map_sampler: sampler, near: vec4<f32>,
    far: vec4<f32>, p: vec2<f32>, fresh: f32) -> f32 {
    if fresh <= 0.0 {
        return 0.0;
    }
    return fresh * snow_country(map, map_sampler, near, far, p);
}

// Where fresh snow covering `share` of the ground lies on a surface: first in its hollows, where
// its `rise` (0 lowest, 1 highest) is low, and less on slopes (`up`, its normal's height); the
// highest tufts and stones stay bare. 0 bare, 1 white.
fn fresh_snow_cover(share: f32, rise: f32, up: f32) -> f32 {
    let level = 1.05 * share;
    return (1.0 - smoothstep(level - 0.1, level + 0.1, rise)) * smoothstep(0.3, 0.8, up)
        * min(4.0 * share, 1.0);
}

// The map's near and far texels at `p` from the layer pair starting at `first`.
fn ground_layers(map: texture_2d_array<f32>, map_sampler: sampler, near: vec4<f32>,
    far: vec4<f32>, p: vec2<f32>, first: u32, open: vec4<f32>) -> vec4<f32> {
    if near.z <= 0.0 {
        return open;
    }
    let size = f32(textureDimensions(map).x);
    let near_weight = ground_layer_weight(p, near, size, GROUND_NEAR_FADE);
    let near_texel = ground_layer_texel(map, map_sampler, p, near, size, first, near_weight);
    if near_weight >= 1.0 {
        return near_texel;
    }
    let far_weight = ground_layer_weight(p, far, size, 1.0);
    let far_texel = ground_layer_texel(map, map_sampler, p, far, size, first + 1u, far_weight);
    return mix(mix(open, far_texel, far_weight), near_texel, near_weight);
}

// The ground map's near layer fades into the far one over this many of its texels.
const GROUND_NEAR_FADE: f32 = 8.0;

// How much of a layer placed at `placement` (`size` texels a side) holds at `p`: none past its
// edge, all from `fade` texels inside it, measured from the outer texels' centres so filtering
// never reaches past them.
fn ground_layer_weight(p: vec2<f32>, placement: vec4<f32>, size: f32, fade: f32) -> f32 {
    let texels = (p - placement.xy) / placement.z;
    let inside = min(min(texels.x, texels.y), size - max(texels.x, texels.y)) - 0.5;
    return clamp(inside / fade, 0.0, 1.0);
}

fn ground_layer_texel(map: texture_2d_array<f32>, map_sampler: sampler, p: vec2<f32>,
    placement: vec4<f32>, size: f32, layer: u32, weight: f32) -> vec4<f32> {
    if weight <= 0.0 {
        return vec4(0.0);
    }
    let uv = (p - placement.xy) / (placement.z * size);
    return textureSampleLevel(map, map_sampler, uv, layer, 0.0);
}
