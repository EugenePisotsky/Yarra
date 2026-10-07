// How much of the sun the camera sees (crates/atmosphere/src/sun_glare.rs), for the glare the
// sky composite draws around it. On screen, depth samples across the sun's disc count the sky
// behind it; off screen, the sun's shadow maps at the camera tell whether something blocks it.
// The cloud layer's shadow at the camera dims both. Off screen, shadow samples spread over a
// patch around the camera, and the result eases in over a few frames, so walking through leaf
// shadows does not switch the glare on and off.
#import bevy_render::view::View
#import bevy_pbr::mesh_view_types::{Lights, DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT}
#import "shaders/clouds/types.wgsl"::CloudParams

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> clouds: CloudParams;
#ifdef MULTISAMPLED
@group(0) @binding(2) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(2) var depth: texture_depth_2d;
#endif
@group(0) @binding(3) var<uniform> lights: Lights;
@group(0) @binding(4) var shadow_maps: texture_depth_2d_array;
@group(0) @binding(5) var shadow_sampler: sampler_comparison;
@group(0) @binding(6) var cloud_shadow: texture_2d<f32>;
@group(0) @binding(7) var repeat_sampler: sampler;
// x: share of the sun seen; y: share the cloud layer lets through at the camera; zw: the sun's
// position in main-pass uv.
@group(0) @binding(8) var<storage, read_write> sun_state: vec4<f32>;

const SAMPLES: u32 = 64u;
const GOLDEN_ANGLE: f32 = 2.3999632;
// Radius of the patch of shadow samples around the camera, metres.
const PATCH: f32 = 0.4;
// Share of each new measurement in the result.
const EASE: f32 = 0.25;
var<workgroup> seen: array<f32, SAMPLES>;

// Shadow-casting sun light, or -1.
fn sun_index() -> i32 {
    for (var i = 0u; i < lights.n_directional_lights; i += 1u) {
        let light = &lights.directional_lights[i];
        if ((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u
            && dot((*light).direction_to_light, clouds.sun.xyz) > 0.995 {
            return i32(i);
        }
    }
    return -1;
}

// Sunlight reaching `p` past the first cascade's casters; 1 outside it.
fn shadow_at(index: i32, p: vec3<f32>) -> f32 {
    let light = &lights.directional_lights[index];
    if (*light).num_cascades == 0u {
        return 1.0;
    }
    let cascade = &(*light).cascades[0];
    let lifted = p + (*light).direction_to_light * ((*light).shadow_depth_bias + 2.0 * (*cascade).texel_size);
    let clip = (*cascade).clip_from_world * vec4(lifted, 1.0);
    let ndc = clip.xyz / clip.w;
    if any(abs(ndc.xy) > vec2(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }
    return textureSampleCompareLevel(shadow_maps, shadow_sampler, ndc.xy * vec2(0.5, -0.5) + 0.5,
        i32((*light).depth_texture_base_index), ndc.z);
}

fn cloud_visibility(p: vec3<f32>) -> f32 {
    let sun = clouds.sun.xyz;
    if clouds.layer.w < 0.5 || sun.y <= 0.0 || p.y >= clouds.layer.x + clouds.layer.y {
        return 1.0;
    }
    let hit = p.xz + clouds.offset.xy + sun.xz * (clouds.layer.x - p.y) / max(sun.y, 0.04);
    let shadow = textureSampleLevel(cloud_shadow, repeat_sampler,
        (hit - clouds.offset.zw) / (clouds.layer.z * 4.0), 0.0).r;
    return max(shadow, 0.12);
}

@compute @workgroup_size(64)
fn occlusion(@builtin(local_invocation_index) i: u32) {
    let sun = clouds.sun.xyz;
    let index = sun_index();
    let clip = view.clip_from_world * vec4(sun, 0.0);
    let ndc = clip.xy / max(clip.w, 1e-6);
    let uv = ndc * vec2(0.5, -0.5) + 0.5;
    let viewport = view.main_pass_viewport;
    let radius_angle = select(0.0047, 0.5 * lights.directional_lights[max(index, 0)].sun_disk_angular_size,
        index >= 0);
    let radius = radius_angle * view.clip_from_view[1][1] * 0.5 * viewport.w;
    let centre = uv * viewport.zw;
    let on_screen = clip.w > 0.0 && all(centre >= vec2(radius)) && all(centre <= viewport.zw - radius);
    // A spiral of samples covers the disc evenly.
    let r = radius * sqrt((f32(i) + 0.5) / f32(SAMPLES));
    let a = f32(i) * GOLDEN_ANGLE;
    let at = vec2<i32>(centre + r * vec2(cos(a), sin(a)) + viewport.xy);
    var sky = 0.0;
    if on_screen {
        sky = select(0.0, 1.0, textureLoad(depth, at, 0) == 0.0);
    } else if index >= 0 {
        // Spread across the plane facing the sun.
        let side = normalize(cross(sun, select(vec3(0.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0), abs(sun.y) > 0.9)));
        let up = cross(side, sun);
        let spread = PATCH * sqrt((f32(i) + 0.5) / f32(SAMPLES));
        sky = shadow_at(index, view.world_position + (side * cos(a) + up * sin(a)) * spread);
    } else {
        sky = 1.0;
    }
    seen[i] = sky;
    workgroupBarrier();
    for (var stride = SAMPLES / 2u; stride > 0u; stride /= 2u) {
        if i < stride {
            seen[i] += seen[i + stride];
        }
        workgroupBarrier();
    }
    if i != 0u {
        return;
    }
    let cloud = cloud_visibility(view.world_position) * step(-0.02, sun.y);
    let visible = seen[0] / f32(SAMPLES) * cloud;
    sun_state = vec4(mix(sun_state.x, visible, EASE), cloud, uv);
}
