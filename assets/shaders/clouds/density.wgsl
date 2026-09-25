#import "shaders/clouds/types.wgsl"::CloudParams
fn density_at(p: vec3<f32>, c: CloudParams, noise: texture_3d<f32>, repeat: sampler) -> f32 {
    let h = (p.y-c.layer.x)/c.layer.y;
    if h <= 0.0 || h >= 1.0 || c.layer.w < 0.5 || max(c.shape.x, c.transition.x) <= 0.0 { return 0.0; }
    let seed = vec3(c.shape.w, c.shape.w*0.37, c.shape.w*0.73);
    let q = (p.xz - c.offset.zw)/c.layer.z + seed.xz;
    // A larger weather domain both opens gaps and warps the smaller body noise.
    // All frequencies remain integer multiples of this domain so wrapping,
    // floating-origin rebases and the shared shadow tile agree exactly.
    let region = textureSampleLevel(noise, repeat, vec3(q.x*0.25, seed.y, q.y*0.25), 0.0);
    // Weather arrives region by region rather than as one global threshold: each part of the
    // field blends from the previous shape at its own time, over ~40% of the change.
    let arrival = smoothstep(0.25, 0.75, mix(region.g, region.b, 0.5));
    let local = smoothstep(0.0, 1.0, (c.transition.w*1.6-arrival)/0.6);
    let local_coverage = mix(c.transition.x, c.shape.x, local);
    let warp = (region.ra-vec2(0.5))*1.4;
    // Sample a full vertical shape even for thin layers: a shallow horizontal
    // slice of the noise produced the old flat, repeated pancakes.
    let body_q = vec3(q.x+warp.x, h*0.45+seed.y, q.y+warp.y);
    let body = textureSampleLevel(noise, repeat, body_q, 0.0).r;
    let coverage = clamp(local_coverage + (region.b-0.5)*0.85, 0.0, 1.0);
    let overcast = smoothstep(0.65,0.95,local_coverage);
    // Raise the threshold with height to round off towers of different heights;
    // dense overcast retains its connected, flatter deck.
    let threshold = 0.76-coverage*0.64 + pow(h,1.7)*mix(0.28,0.10,overcast);
    let profile = smoothstep(0.0,0.10,h)*(1.0-smoothstep(0.85,1.0,h));
    let shape = clamp((body-threshold)/max(1.0-threshold,0.01),0.0,1.0)*profile;
    let detail = textureSampleLevel(noise, repeat, body_q*3.0+vec3(0.17),0.0).g;
    let erosion = mix(c.transition.z, c.shape.z, local);
    // Callers scale by the target extinction; carry the local extinction as a ratio.
    let extinction = select(1.0, mix(c.transition.y, c.shape.y, local)/c.shape.y, c.shape.y > 1e-6);
    return max(shape-(1.0-detail)*erosion*0.18,0.0)*extinction;
}
fn light_transmission(p: vec3<f32>, direction: vec3<f32>, c: CloudParams,
    noise: texture_3d<f32>, repeat: sampler, steps: u32) -> f32 {
    if direction.y <= 0.0 { return 0.0; }
    let distance = min((c.layer.x+c.layer.y-p.y)/max(direction.y,0.04),30000.0);
    let ds = max(distance,0.0)/f32(steps);
    var optical=0.0;
    for(var i=0u;i<steps;i+=1u) {
        optical += density_at(p+direction*((f32(i)+0.5)*ds),c,noise,repeat)*ds*c.shape.y;
    }
    return exp(-optical);
}
