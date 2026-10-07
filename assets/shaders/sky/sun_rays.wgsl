// Sun rays on screen (crates/atmosphere/src/sun_glare.rs): how much of the glare veil around
// the sun reaches each pixel. The veil is light scattered towards the eye from around the sun;
// trees, ridges and buildings between a pixel and the sun on screen hold it back, so beams fan
// out from their silhouettes. This reaches past the shadow maps that light shafts march
// through, to a sun behind distant ridges. Each texel, at a quarter of the main-pass
// resolution, sums the sky seen along its line towards the sun, weighted by the veil there.
#import bevy_render::view::View
#import "shaders/clouds/types.wgsl"::CloudParams

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> clouds: CloudParams;
#ifdef MULTISAMPLED
@group(0) @binding(2) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(2) var depth: texture_depth_2d;
#endif
// x: share of the sun seen; y: share the cloud layer lets through at the camera.
@group(0) @binding(3) var<storage, read> sun_state: vec4<f32>;
@group(0) @binding(4) var rays: texture_storage_2d<rgba16float, write>;

const SAMPLES: u32 = 32u;
// GLARE_VEIL_WIDTH in composite.wgsl, radians.
const VEIL_WIDTH: f32 = 0.12;

fn gradient_noise(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2(0.06711056, 0.00583715))));
}

@compute @workgroup_size(8, 8)
fn sun_rays(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(rays);
    if any(id.xy >= size) {
        return;
    }
    let viewport = view.main_pass_viewport;
    let pixel = (vec2<f32>(id.xy) + 0.5) * viewport.zw / vec2<f32>(size);
    let clip = view.clip_from_world * vec4(clouds.sun.xyz, 0.0);
    var visible = sun_state.x;
    if clip.w > 1e-4 {
        let sun = (clip.xy / clip.w * vec2(0.5, -0.5) + 0.5) * viewport.zw;
        // Pixels per radian near the middle of the view.
        let focal = view.clip_from_view[1][1] * 0.5 * viewport.w;
        let jitter = gradient_noise(vec2<f32>(id.xy));
        var sum = 0.0;
        var weight = 0.0;
        for (var i = 0u; i < SAMPLES; i += 1u) {
            // Samples crowd towards the sun, where the veil is brightest; the weight carries
            // the spacing (2t) and the veil there.
            let t = (f32(i) + jitter) / f32(SAMPLES);
            let q = sun + (pixel - sun) * t * t;
            let u = length(q - sun) / (focal * VEIL_WIDTH);
            let w = t / ((1.0 + u * u) * sqrt(sqrt(1.0 + u * u)));
            // Off screen, the share of the sun seen stands in.
            var sky = sun_state.x;
            if all(q >= vec2(0.0)) && all(q < viewport.zw) {
                let z = textureLoad(depth, vec2<i32>(q + viewport.xy), 0);
                sky = select(0.0, sun_state.y, z == 0.0);
            }
            sum += w * sky;
            weight += w;
        }
        visible = sum / max(weight, 1e-12);
    }
    textureStore(rays, id.xy, vec4(visible, 0.0, 0.0, 1.0));
}
