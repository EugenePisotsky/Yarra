// Offline diagnostic only. Never loaded by the game/editor renderer.
struct Triangle { a: vec4<f32>, b: vec4<f32>, c: vec4<f32> }
struct Fixture {
    // x: triangle count, y: instance count, z: sky samples, w: exposure
    counts: vec4<f32>,
    // Reserved for compatibility with the original direct-shadow fixture.
    forward_length: vec4<f32>,
    // x: unused, y: start bias, z: full reference range
    trace: vec4<f32>,
    // Reserved.
    contact: vec4<f32>,
}
@group(0) @binding(0) var<uniform> fixture: Fixture;
@group(0) @binding(1) var<storage, read_write> exported: array<Triangle>;
@group(0) @binding(2) var<storage, read> requests: array<vec4<u32>>;
@group(2) @binding(0) var<storage, read> triangles: array<Triangle>;
struct BvhNode { minimum: vec4<f32>, maximum: vec4<f32>, links: vec4<u32> }
@group(2) @binding(1) var<storage, read> nodes: array<BvhNode>;

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
    out.color = vec3(0.105,0.12,0.065);
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


const SKY_DIRECTIONS = array<vec3<f32>, 64>(
vec3<f32>(0.999969482, 0.007812500, 0.000000000),
vec3<f32>(-0.737166326, 0.023437500, 0.675304740),
vec3<f32>(0.087358999, 0.039062500, -0.995410733),
vec3<f32>(0.607528344, 0.054687500, 0.792413143),
vec3<f32>(-0.982276333, 0.070312500, -0.173750852),
vec3<f32>(0.840633847, 0.085937500, -0.534742443),
vec3<f32>(-0.258261933, 0.101562500, 0.960721517),
vec3<f32>(-0.457731287, 0.117187500, -0.881333739),
vec3<f32>(0.931000019, 0.132812500, 0.339999714),
vec3<f32>(-0.914105463, 0.148437500, 0.377329447),
vec3<f32>(0.418102859, 0.164062500, -0.893461524),
vec3<f32>(0.294412643, 0.179687500, 0.938633900),
vec3<f32>(-0.848548159, 0.195312500, -0.491751003),
vec3<f32>(0.954700130, 0.210937500, -0.209888145),
vec3<f32>(-0.560174125, 0.226562500, 0.796790049),
vec3<f32>(-0.124684858, 0.242187500, -0.962184442),
vec3<f32>(0.738800011, 0.257812500, 0.622661432),
vec3<f32>(-0.961068370, 0.273437500, 0.039743194),
vec3<f32>(0.678569648, 0.289062500, -0.675267432),
vec3<f32>(-0.043995152, 0.304687500, 0.951435733),
vec3<f32>(-0.606951420, 0.320312500, -0.727330651),
vec3<f32>(0.933472700, 0.335937500, 0.125597431),
vec3<f32>(-0.768458363, 0.351562500, 0.534673314),
vec3<f32>(0.204149925, 0.367187500, -0.907466885),
vec3<f32>(0.459308649, 0.382812500, 0.801554836),
vec3<f32>(-0.873805514, 0.398437500, -0.278767792),
vec3<f32>(0.826315475, 0.414062500, -0.381778708),
vec3<f32>(-0.348604532, 0.429687500, 0.832972708),
vec3<f32>(-0.303041783, 0.445312500, -0.842533356),
vec3<f32>(0.785545965, 0.460937500, 0.412860943),
vec3<f32>(-0.850102585, 0.476562500, 0.224084313),
vec3<f32>(0.470793416, 0.492187500, -0.732191932),
vec3<f32>(0.145912089, 0.507812500, 0.849020687),
vec3<f32>(-0.673661602, 0.523437500, -0.521721410),
vec3<f32>(0.839389997, 0.539062500, -0.069541745),
vec3<f32>(-0.565033918, 0.554687500, 0.610785109),
vec3<f32>(0.004007153, 0.570312500, -0.821418039),
vec3<f32>(0.544466342, 0.585937500, 0.600194676),
vec3<f32>(-0.795416909, 0.601562500, -0.073719060),
vec3<f32>(0.626745801, 0.617187500, -0.475677718),
vec3<f32>(-0.138587896, 0.632812500, 0.761801638),
vec3<f32>(-0.405455540, 0.648437500, -0.644309409),
vec3<f32>(0.721087548, 0.664062500, 0.197620202),
vec3<f32>(-0.652586511, 0.679687500, 0.334896623),
vec3<f32>(0.249834828, 0.695312500, -0.673886553),
vec3<f32>(0.265157206, 0.710937500, 0.651352076),
vec3<f32>(-0.620902284, 0.726562500, -0.294257179),
vec3<f32>(0.640444308, 0.742187500, -0.197455825),
vec3<f32>(-0.329904625, 0.757812500, 0.562923755),
vec3<f32>(-0.133083054, 0.773437500, -0.619744572),
vec3<f32>(0.500816804, 0.789062500, 0.355756799),
vec3<f32>(-0.589140915, 0.804687500, 0.073423498),
vec3<f32>(0.370698211, 0.820312500, -0.435511468),
vec3<f32>(0.020000939, 0.835937500, 0.548460079),
vec3<f32>(-0.367980421, 0.851562500, -0.373405569),
vec3<f32>(0.497332092, 0.867187500, 0.025428924),
vec3<f32>(-0.362111476, 0.882812500, 0.299194534),
vec3<f32>(0.060675084, 0.898437500, -0.434889173),
vec3<f32>(0.230009037, 0.914062500, 0.334044292),
vec3<f32>(-0.358968989, 0.929687500, -0.082597926),
vec3<f32>(0.283784660, 0.945312500, -0.160781044),
vec3<f32>(-0.085403972, 0.960937500, 0.263258965),
vec3<f32>(-0.089319768, 0.976562500, -0.195825592),
vec3<f32>(0.114847396, 0.992187500, 0.048724127)
);

fn blocked(position: vec3<f32>, direction: vec3<f32>) -> bool {
    let origin = position + direction * fixture.trace.y;
    let safe_direction = select(vec3(-1e-7), vec3(1e-7), direction >= vec3(0.0));
    let inverse_direction = 1.0 / select(safe_direction, direction, abs(direction) > vec3(1e-7));
    var index = 0u;
    let end = nodes[0].links.z;
    loop {
        if (index >= end) { break; }
        let node = nodes[index];
        let t1 = (node.minimum.xyz - origin) * inverse_direction;
        let t2 = (node.maximum.xyz - origin) * inverse_direction;
        let near = min(t1, t2);
        let far = max(t1, t2);
        let enter = max(max(near.x, near.y), max(near.z, 0.0));
        let leave = min(min(far.x, far.y), min(far.z, fixture.trace.z));
        if (enter > leave) { index = node.links.z; continue; }
        if (node.links.y == 0u) { index++; continue; }
        for (var j = 0u; j < node.links.y; j++) {
            let tri = triangles[node.links.x + j];
            let e1 = tri.b.xyz - tri.a.xyz;
            let e2 = tri.c.xyz - tri.a.xyz;
            let p = cross(direction, e2);
            let determinant = dot(e1, p);
            if (abs(determinant) < 1e-9) { continue; }
            let reciprocal = 1.0 / determinant;
            let relative = origin - tri.a.xyz;
            let u = dot(relative, p) * reciprocal;
            let q = cross(relative, e1);
            let v = dot(direction, q) * reciprocal;
            let t = dot(e2, q) * reciprocal;
            if (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.00001 && t < fixture.trace.z) { return true; }
        }
        index = node.links.z;
    }
    return false;
}

// Uniform upper-hemisphere sky sampled in world space. Thin leaves receive light on both
// faces, using |N.L|; the ground uses its upward face. Normalization isolates visibility
// from the production material's existing sky orientation response. No camera-dependent AO.
fn ambient_visibility(input: VertexOutput) -> f32 {
    let normal = normalize3_or(input.world_normal, vec3(0.0, 1.0, 0.0));
    var visible = 0.0;
    var total = 0.0;
    for (var sample = 0u; sample < 64u; sample++) {
        let direction = SKY_DIRECTIONS[sample];
        let weight = abs(dot(normal, direction));
        total += weight;
        if (!blocked(input.world_position, direction)) { visible += weight; }
    }
    return visible / max(total, 0.0001);
}

fn shaded(input: VertexOutput, sun: f32, sky: f32) -> vec4<f32> {
    if (input.identity == 0u) {
        let exposed_ambient = camera.ambient_radiance.xyz * fixture.counts.w;
        let direct = max(camera.sun_direction.y, 0.0) * 0.7;
        return vec4(input.color * (exposed_ambient * sky + vec3(direct * sun)), 1.0);
    }
    return shade_blade(input, sun, sky);
}
struct Comparison {
    @location(0) baseline: vec4<f32>,
    @location(1) ambient: vec4<f32>,
    @location(2) combined: vec4<f32>,
    // sky visibility, sun visibility, unused, receiver ID
    @location(3) visibility: vec4<f32>,
    @location(4) mask: vec4<f32>,
}
@fragment
fn compare(input: VertexOutput) -> Comparison {
    let sky = ambient_visibility(input);
    let sun = select(1.0, 0.0, blocked(input.world_position, normalize(camera.sun_direction.xyz)));
    return Comparison(shaded(input, 1.0, 1.0), shaded(input, 1.0, sky), shaded(input, sun, sky),
        vec4(sky, sun, 0.0, f32(receiver_identity(input) + 1u)), vec4(vec3(sky), 1.0));
}
