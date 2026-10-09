#import "shaders/clouds/types.wgsl"::CloudParams
// Broken cloud is cumulus. The body noise is sampled stretched upwards, so each cloud is a
// column with one footprint from its flat base, and the threshold rising with height rounds its
// top: strong cores tower, edges stay low. Sampled at the layer's share of the noise period (5x
// faster upwards than across), each noise cell was a flat blob floating in the layer: lenses.
// Billows of Worley noise are eaten out of the outline, and the edge is sharp; density rising
// over hundreds of metres from the outline drew every cloud as a smudge.
// As the sky closes the deck keeps the layered sampling and the density that follows the noise:
// its thin and thick parts are what give an overcast sky its texture.
const BODY_STRETCH: f32 = 0.5;
const DECK_LAYERING: f32 = 0.45;
const BILLOW_SCALE: f32 = 3.0;
const BILLOW_DEPTH: f32 = 0.6;
const DECK_BILLOW_DEPTH: f32 = 0.18;
const EDGE: f32 = 0.08;
// Cumulus cores are denser than a deck's average.
const CUMULUS_DENSITY: f32 = 2.5;
fn density_at(p: vec3<f32>, c: CloudParams, noise: texture_3d<f32>, repeat: sampler) -> f32 {
    let h = (p.y-c.layer.x)/c.layer.y;
    if h <= 0.0 || h >= 1.0 || c.layer.w < 0.5 || max(c.shape.x, c.transition.x) <= 0.0 { return 0.0; }
    let seed = vec3(c.shape.w, c.shape.w*0.37, c.shape.w*0.73);
    let q = (p.xz - c.offset.zw)/c.layer.z + seed.xz;
    // Height above the base in noise periods, the scale of `q`.
    let up = (p.y-c.layer.x)/c.layer.z;
    // A larger weather domain both opens gaps and warps the smaller body noise.
    // All frequencies remain integer multiples of this domain so wrapping,
    // floating-origin rebases and the shared shadow tile agree exactly.
    let region = textureSampleLevel(noise, repeat, vec3(q.x*0.25, seed.y, q.y*0.25), 0.0);
    // Weather arrives region by region rather than as one global threshold: each part of the
    // field blends from the previous shape at its own time, over ~40% of the change.
    let arrival = smoothstep(0.25, 0.75, mix(region.g, region.b, 0.5));
    let local = smoothstep(0.0, 1.0, (c.transition.w*1.6-arrival)/0.6);
    let local_coverage = mix(c.transition.x, c.shape.x, local);
    let overcast = smoothstep(0.65,0.95,local_coverage);
    let warp = (region.ra-vec2(0.5))*1.4;
    let body_q = vec3(q.x+warp.x, mix(up*BODY_STRETCH, h*DECK_LAYERING, overcast)+seed.y, q.y+warp.y);
    let body = textureSampleLevel(noise, repeat, body_q, 0.0).r;
    let coverage = clamp(local_coverage + (region.b-0.5)*0.85, 0.0, 1.0);
    // Dense overcast retains its connected, flatter deck.
    let threshold = 0.76-coverage*0.64 + pow(h,1.7)*mix(0.28,0.10,overcast);
    // 0 on the outline, 1 in the core.
    let core = clamp((body-threshold)/max(1.0-threshold,0.01),0.0,1.0);
    let profile = smoothstep(0.0,0.04,h)*(1.0-smoothstep(0.85,1.0,h));
    let billow_q = vec3(body_q.x, up+seed.z, body_q.z)*BILLOW_SCALE+vec3(0.17);
    let billows = 1.0-textureSampleLevel(noise, repeat, billow_q, 0.0).g;
    let erosion = mix(c.transition.z, c.shape.z, local);
    let eaten = core-billows*erosion*mix(BILLOW_DEPTH, DECK_BILLOW_DEPTH, overcast);
    let cumulus = smoothstep(0.0,EDGE,eaten)*CUMULUS_DENSITY;
    // Callers scale by the target extinction; carry the local extinction as a ratio.
    let extinction = select(1.0, mix(c.transition.y, c.shape.y, local)/c.shape.y, c.shape.y > 1e-6);
    return mix(cumulus, max(eaten,0.0), overcast)*profile*extinction;
}
fn light_transmission(p: vec3<f32>, direction: vec3<f32>, c: CloudParams,
    noise: texture_3d<f32>, repeat: sampler, steps: u32) -> f32 {
    // Out through the top towards a light above, or through the base towards one just below the
    // horizontal (a sun that has set for the ground but not for the clouds).
    let exit = select(p.y-c.layer.x, c.layer.x+c.layer.y-p.y, direction.y >= 0.0);
    let distance = min(max(exit,0.0)/max(abs(direction.y),0.04),30000.0);
    let ds = max(distance,0.0)/f32(steps);
    var optical=0.0;
    for(var i=0u;i<steps;i+=1u) {
        optical += density_at(p+direction*((f32(i)+0.5)*ds),c,noise,repeat)*ds*c.shape.y;
    }
    return exp(-optical);
}
