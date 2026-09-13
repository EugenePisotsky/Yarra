// Offline diagnostic only. Never loaded by the game/editor renderer.
struct Triangle { a: vec4<f32>, b: vec4<f32>, c: vec4<f32> }
struct Fixture {
    // x: triangle count, y: instance count, z: steps, w: exposure
    counts: vec4<f32>,
    // xyz: camera forward, w: short ray length in metres
    forward_length: vec4<f32>,
    // x: depth thickness, y: start bias, z: full reference range
    trace: vec4<f32>,
    // method (0 points, 1 pixels), maximum direct-light reduction, plane rejection, range fade start
    contact: vec4<f32>,
}
@group(0) @binding(0) var<uniform> fixture: Fixture;
@group(0) @binding(1) var<storage, read_write> exported: array<Triangle>;
@group(0) @binding(2) var<storage, read> requests: array<vec4<u32>>;
@group(2) @binding(0) var<storage, read> triangles: array<Triangle>;
@group(2) @binding(1) var captured_position: texture_2d<f32>;

@compute @workgroup_size(64)
fn export_geometry(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= u32(fixture.counts.x)) { return; }
    let r = requests[id.x];
    exported[id.x] = Triangle(
        vec4(geometry_vertex(r.x, r.w).world_position, 1.0),
        vec4(geometry_vertex(r.y, r.w).world_position, 1.0),
        vec4(geometry_vertex(r.z, r.w).world_position, 1.0),
    );
}

// Same isotropic GGX helpers as Bevy 0.19 pbr_lighting.wgsl (MIT/Apache-2.0).
fn D_GGX(roughness: f32, NdotH: f32) -> f32 {
    let a = NdotH * roughness;
    let k = roughness / (1.0 - NdotH * NdotH + a * a);
    return k * k / PI;
}
fn V_SmithGGXCorrelated(roughness: f32, NdotV: f32, NdotL: f32) -> f32 {
    let a2 = roughness * roughness;
    let v = NdotL * sqrt((NdotV - a2 * NdotV) * NdotV + a2);
    let l = NdotV * sqrt((NdotL - a2 * NdotL) * NdotL + a2);
    return 0.5 / (v + l);
}

@vertex
fn fixture_vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> VertexOutput {
    if (i < u32(fixture.counts.y)) { return geometry_vertex(v, i); }
    let corners = array<vec2<f32>, 4>(vec2(-2.6,-1.8),vec2(2.6,-1.8),vec2(-2.6,1.8),vec2(2.6,1.8));
    var out: VertexOutput;
    out.world_position = vec3(corners[v].x, -0.003, corners[v].y);
    out.clip_position = camera.clip_from_world * vec4(out.world_position, 1.0);
    out.world_normal = vec3(0.0,1.0,0.0);
    out.color = vec3(0.22,0.20,0.16);
    out.material = vec4(0.8,0.0,1.0,1.0);
    out.ribbon_side_rounding = vec4(1.0,0.0,0.0,0.0);
    out.surface_normal_clump = vec4(0.0,1.0,0.0,0.5);
    out.identity = 0u;
    return out;
}

// Shared base vertices carry the pair ID; interior companion fragments retain their own ID.
fn receiver_identity(input: VertexOutput) -> u32 {
    return input.identity + select(0u, 1u, input.companion_t > 0.000001);
}

@fragment
fn capture_position(input: VertexOutput) -> @location(0) vec4<f32> {
    return vec4(input.world_position, f32(receiver_identity(input) + 1u));
}

// Two-sided Moller-Trumbore ray/triangle intersection. Reference uses exactly the
// rasterized, wind-deformed triangles. No bounding canopy or unrelated proxy.
fn blocker_distance(position: vec3<f32>) -> f32 {
    let direction = normalize(camera.sun_direction.xyz);
    let origin = position + direction * fixture.trace.y;
    var closest = fixture.trace.z + 1.0;
    for (var i = 0u; i < u32(fixture.counts.x); i++) {
        let tri = triangles[i];
        let edge1 = tri.b.xyz - tri.a.xyz;
        let edge2 = tri.c.xyz - tri.a.xyz;
        let p = cross(direction, edge2);
        let det = dot(edge1, p);
        if (abs(det) < 1e-9) { continue; }
        let inv = 1.0 / det;
        let relative = origin - tri.a.xyz;
        let u = dot(relative, p) * inv;
        let q = cross(relative, edge1);
        let v = dot(direction, q) * inv;
        let t = dot(edge2, q) * inv;
        if (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.00001) {
            closest = min(closest, t + fixture.trace.y);
        }
    }
    return closest;
}

// Deliberately simple single-depth-layer trial. No hidden geometry access, temporal
// history, receiver-ID rejection, noise mask or reference information is used here.
fn point_contact(position: vec3<f32>) -> ContactTrace {
    var reads = 1u;
    let direction = normalize(camera.sun_direction.xyz);
    let dimensions = vec2<i32>(textureDimensions(captured_position));
    let steps = u32(fixture.counts.z) - 1u;
    for (var step = 0u; step < steps; step++) {
        let t = fixture.trace.y + (f32(step) + 0.5) / f32(steps)
            * (fixture.forward_length.w - fixture.trace.y);
        let ray = position + direction * t;
        let clip = camera.clip_from_world * vec4(ray, 1.0);
        if (clip.w <= 0.0) { break; }
        let uv = clip.xy / clip.w * vec2(0.5,-0.5) + vec2(0.5);
        let pixel = vec2<i32>(floor(uv * vec2<f32>(dimensions)));
        if (any(pixel < vec2(0)) || any(pixel >= dimensions)) { break; }
        let sample = textureLoad(captured_position, pixel, 0);
        reads++;
        let behind = dot(ray - sample.xyz, fixture.forward_length.xyz);
        if (sample.w > 0.0 && behind > fixture.trace.y && behind < fixture.trace.x) {
            return ContactTrace(0.0, fixture.forward_length.w, reads);
        }
    }
    return ContactTrace(1.0, fixture.forward_length.w, reads);
}

fn shaded(input: VertexOutput, visibility: f32) -> vec4<f32> {
    if (input.identity == 0u) {
        let direct = max(camera.sun_direction.y, 0.0) * 0.7;
        return vec4(input.color * (0.30 + direct * visibility), 1.0);
    }
    return shade_blade(input, visibility);
}
struct Comparison {
    @location(0) baseline: vec4<f32>,
    @location(1) reference: vec4<f32>,
    @location(2) screen: vec4<f32>,
    // full reference, reference within supported reach, raw contact visibility, receiver ID
    @location(3) metrics: vec4<f32>,
    // reach / world limit, reads / 16, reference over world limit, authored AO
    @location(4) detail: vec4<f32>,
}
@fragment
fn compare(input: VertexOutput) -> Comparison {
    let distance = blocker_distance(input.world_position);
    let full = select(1.0,0.0,distance <= fixture.trace.z);
    // Reconstruct from the captured visible surface, including helper pixels. This
    // avoids giving the experiment a geometric normal unavailable to a screen pass.
    // The receiver read is INCLUDED in the total 8/16-read cap for both methods.
    let receiver = textureLoad(captured_position, vec2<i32>(input.clip_position.xy), 0).xyz;
    let dx = dpdx(receiver);
    let dy = dpdy(receiver);
    let cross_plane = cross(dx, dy);
    let footprint = dot(receiver - camera.camera_position.xyz, fixture.forward_length.xyz) / camera.projection.x;
    let reliable_plane = max(length(dx), length(dy)) <= 4.0 * footprint && dot(cross_plane, cross_plane) > 1e-16;
    // Zero normal disables plane rejection at a depth edge; the thin intersection
    // interval still applies there. No neighbor IDs or hidden geometry are consulted.
    var plane = vec3(0.0);
    if (reliable_plane) { plane = normalize(cross_plane); }
    var trace: ContactTrace;
    if (fixture.contact.x > 0.5) { trace = pixel_contact(receiver, plane); }
    else { trace = point_contact(receiver); }
    let short = select(1.0, 0.0, distance <= trace.reach);
    let world_limit = select(1.0, 0.0, distance <= fixture.forward_length.w);
    let strength = fixture.contact.y;
    return Comparison(shaded(input,1.0), shaded(input,mix(1.0,full,strength)),
        shaded(input,mix(1.0,trace.visibility,strength)),
        vec4(full,short,trace.visibility,f32(receiver_identity(input) + 1u)),
        vec4(trace.reach / fixture.forward_length.w, f32(trace.reads) / 16.0,
            world_limit, input.material.z));
}
