// The wind every tree shares (crates/engine/src/tree_wind.rs) and how it moves tree meshes
// (tree_wind.wesl) and impostors (tree_impostor.wesl). Bindings stay with those shaders: here
// the pose, the tree's profile and the viewer come in as arguments, so tests run these exact
// functions (crates/engine/src/tree_wind/gpu_tests.rs).

// tree_wind.rs WindBuffer: this frame's pose and the previous one, for temporal motion.
struct WindPose {
    field: vec4<f32>,
    phases: vec4<f32>,
    response: vec4<f32>,
    camera: vec4<f32>,
    hierarchy: vec4<f32>,
    sway_phases: vec4<f32>,
}
struct WindFrames {
    current: WindPose,
    previous: WindPose,
}

// The attributes of a tree vertex the wind reads: tree_wind.wesl's Vertex without Bevy's IO.
struct TreeVertex {
    position: vec3<f32>,
    @if(TREE_HIERARCHY)
    wind_pivot: vec4<f32>,
    @if(TREE_HIERARCHY)
    wind_axis: vec4<f32>,
    @if(VERTEX_UVS_B)
    uv_b: vec2<f32>,
    @if(TREE_BRANCH_CARDS)
    card_pivot: vec3<f32>,
    @if(TREE_BRANCH_CARDS)
    card_axis: vec3<f32>,
    @if(TREE_BRANCH_CARDS)
    card_normal: vec3<f32>,
    @if(TREE_BRANCH_CARDS && TREE_CARD_FACING)
    card_facing: vec2<f32>,
}

// Matches VegetationWind's coherent travelling field. All independent phase
// offsets include the floating origin and time, reduced on the CPU in f64.
fn field_force(anchor: vec2<f32>, pose: WindPose) -> f32 {
    let direction = pose.field.xy;
    let perpendicular = vec2(-direction.y, direction.x);
    let broad_local = dot(anchor, direction) * pose.field.z;
    let cross_local = dot(anchor, perpendicular) * pose.field.z * 0.71;
    let broad = broad_local + pose.phases.x;
    let cross = cross_local + pose.phases.y;
    let gust = broad_local * 0.43 - cross_local * 0.61 + pose.phases.z;
    let broad_amount = sin(broad + sin(cross) * 0.85) * 0.5 + 0.5;
    let gust_coordinate = clamp(sin(gust) * 0.5 + 0.5, 0.0, 1.0);
    let rise = clamp((gust_coordinate - 0.28) / 0.72, 0.0, 1.0);
    let pulse = rise * rise * (3.0 - 2.0 * rise);
    return pose.field.w * clamp(0.25 + broad_amount * 0.18 + pulse * pose.response.x * 0.90, 0.12, 1.30);
}

fn turn_axis(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    return v * c + cross(axis, v) * sin(angle) + axis * dot(axis, v) * (1.0 - c);
}

// Yaw preserves the authored in-plane roll; elevation follow optionally tilts
// the surface to face elevated cameras. Full follow works at overhead angles too.
fn turn_camera(v: vec3<f32>, normal: vec3<f32>, to_view: vec3<f32>, follow: f32) -> vec3<f32> {
    if dot(to_view, to_view) < 1e-8 { return v; }
    let n = normalize(normal);
    let d = normalize(to_view);
    let up = vec3(0.0, 1.0, 0.0);
    var nh = vec3(n.x, 0.0, n.z);
    if dot(nh, nh) < 1e-12 { nh = vec3(0.0, 0.0, 1.0); }
    nh = normalize(nh);
    var dh = vec3(d.x, 0.0, d.z);
    if dot(dh, dh) < 1e-12 { dh = nh; }
    dh = normalize(dh);
    let yaw = atan2(dot(up, cross(nh, dh)), dot(nh, dh));
    let pitch = (atan2(n.y, length(n.xz)) - atan2(d.y, length(d.xz))) * clamp(follow, 0.0, 1.0);
    return turn_axis(turn_axis(v, up, yaw), normalize(cross(up, dh)), pitch);
}

// Use the main camera in shadows, and its previous pose for temporal motion; `viewer` is the
// view's own LOD position.
fn facing_camera(world: vec4<f32>, model: mat4x4<f32>, vertex: TreeVertex, camera: vec4<f32>,
                 viewer: vec3<f32>) -> vec4<f32> {
@if(TREE_BRANCH_CARDS) {
    if dot(vertex.card_axis, vertex.card_axis) < 0.25 {
        return world;  // a card that does not turn
    }
    let pivot = (model * vec4(vertex.card_pivot, 1.0)).xyz;
    let axis = normalize((model * vec4(vertex.card_axis, 0.0)).xyz);
    let rest = (model * vec4(vertex.card_normal, 0.0)).xyz;
    let rest_facing = rest - axis * dot(rest, axis);
    let to_view = select(viewer, camera.xyz, camera.w > 0.5) - pivot;
@if(TREE_CARD_FACING)
    if vertex.card_facing.x > 0.5 {
        return vec4(pivot + turn_camera(world.xyz - pivot, rest, to_view, vertex.card_facing.y), world.w);
    }
    let view_facing = to_view - axis * dot(to_view, axis);
    if dot(rest_facing, rest_facing) < 1e-8 || dot(view_facing, view_facing) < 1e-8 {
        return world;  // looking straight along the branch: any turn is as good
    }
    let a = normalize(rest_facing);
    let b = normalize(view_facing);
    let c = dot(a, b);
    let s = dot(axis, cross(a, b));
    let r = world.xyz - pivot;
    let turned = r * c + cross(axis, r) * s + axis * dot(axis, r) * (1.0 - c);
    return vec4(pivot + turned, world.w);
}
@else
    return world;
}

fn displaced_position(p: vec4<f32>, model: mat4x4<f32>, weights_in: vec2<f32>, pose: WindPose) -> vec4<f32> {
    let weights = clamp(weights_in, vec2(0.0), vec2(1.0));
    let direction = pose.field.xy;
    let perpendicular = vec2(-direction.y, direction.x);
    let flutter_wave = sin(dot(p.xz, vec2(1.91, -1.37)) + p.y * 1.13 + pose.phases.w);
    let branch = pose.response.y * weights.y * field_force(model[3].xz, pose);
    let flutter = pose.response.z * weights.x * pose.field.w * flutter_wave;
    let horizontal = direction * branch + perpendicular * flutter;
    return p + vec4(horizontal.x, flutter * 0.16, horizontal.y, 0.0);
}

// Structural wind. All positions, pivots and directions share this pose.
struct SwayFrame {
    source: vec3<f32>,
    deformed_anchor: vec3<f32>,
    trunk_axis: vec3<f32>,
    trunk_angle: f32,
    branch_axis: vec3<f32>,
    branch_angle: f32,
}

fn sway_vector(v: vec3<f32>, frame: SwayFrame) -> vec3<f32> {
    return turn_axis(turn_axis(v, frame.branch_axis, frame.branch_angle), frame.trunk_axis, frame.trunk_angle);
}
fn sway_point(p: vec3<f32>, frame: SwayFrame) -> vec3<f32> {
    return frame.deformed_anchor + sway_vector(p - frame.source, frame);
}

// Four coherent bands with the amplitude/phase response of a damped oscillator, for a tree
// rooted at `root` (world XZ). This is an artistic approximation, not a fitted Kaimal
// spectrum. Species retain their own response period when wind strength changes; damping
// prevents ringing.
fn sway_signal(root: vec2<f32>, pose: WindPose, period: f32, detail: f32) -> f32 {
    let frequencies = vec4(0.55, 1.3, 2.7, 4.3);
    let spatial = vec4(0.035, 0.07, 0.14, 0.28);
    let phase = pose.sway_phases + dot(root, pose.field.xy) * spatial + vec4(detail);
    let ratio = frequencies * max(0.5, period * pose.hierarchy.w) / 6.2831853;
    let spring = vec4(1.0) - ratio * ratio;
    let damping = 1.5 * ratio;
    let gain = inverseSqrt(spring * spring + damping * damping);
    let lag = atan2(damping, spring);
    // Phases travel backwards in time, so lag is added here.
    return dot(sin(phase + lag) * gain, vec4(0.65, 0.38, 0.19, 0.09));
}

fn structural_frame(local_anchor: vec3<f32>, limb_axis: vec4<f32>, height: f32,
                    model: mat4x4<f32>, pose: WindPose, profile: vec4<f32>) -> SwayFrame {
    let up = normalize(model[1].xyz);
    let direction = vec3(pose.field.x, 0.0, pose.field.y);
    var bend_axis = cross(up, direction);
    if dot(bend_axis, bend_axis) < 1e-8 { bend_axis = vec3(0.0, 0.0, 1.0); }
    bend_axis = normalize(bend_axis);
    let pressure = pose.field.w * pose.field.w;
    let wave = sway_signal(model[3].xz, pose, profile.y, 0.0);
    let lean = clamp(0.11 * pressure * profile.x * profile.w * pose.hierarchy.x
                    * (1.0 + pose.response.x * wave), -0.15, 0.8);
    let tree_height = max(0.1, height * length(model[1].xyz));
    let y = max(0.0, local_anchor.y) * length(model[1].xyz);
    let angle = lean * clamp(y / tree_height, 0.0, 1.0);
    let curvature = lean / tree_height;
    var center = up * y;
    if abs(curvature) > 1e-6 {
        center = up * (sin(angle) / curvature)
                 + cross(bend_axis, up) * ((1.0 - cos(angle)) / curvature);
        // Sprays can extend beyond the authored stem height. Continue along the
        // tip tangent rather than collapsing those anchors onto the treetop.
        center += turn_axis(up, bend_axis, angle) * max(0.0, y - tree_height);
    }
    let source = (model * vec4(local_anchor, 1.0)).xyz;
    // Preserve buried feet and existing trunk lean/curvature in the rest shape.
    let lateral = source - model[3].xyz - up * y;
    let deformed_anchor = model[3].xyz + center + turn_axis(lateral, bend_axis, angle);
    var branch_axis = bend_axis;
    var branch_angle = 0.0;
    if limb_axis.w > 0.0 {
        let axis = normalize((model * vec4(limb_axis.xyz, 0.0)).xyz);
        let force_axis = cross(axis, direction);
        let lever = length(force_axis);
        if lever > 1e-5 { branch_axis = force_axis / lever; }
        let phase = dot(local_anchor, vec3(0.73, 0.41, -0.57));
        branch_angle = clamp(0.10 * pressure * profile.w * limb_axis.w * pose.hierarchy.y * lever
            * (0.6 + pose.response.x * sway_signal(model[3].xz, pose, profile.y * 0.35, phase)), -0.5, 0.5);
    }
    return SwayFrame(source, deformed_anchor, bend_axis, angle, branch_axis, branch_angle);
}

// Where the wind moves `point` of an upright tree rooted at `root`, with a stem `height`
// metres tall: as structural_frame and sway_point move a point of its trunk, for impostors.
// The stem bends into an arc and points beside it turn with it; `amount` scales the lean.
fn sway_point_of_trunk(point: vec3<f32>, root: vec3<f32>, height: f32, profile: vec4<f32>,
                       pose: WindPose, amount: f32) -> vec3<f32> {
    let up = vec3(0.0, 1.0, 0.0);
    let direction = vec3(pose.field.x, 0.0, pose.field.y);
    var bend_axis = cross(up, direction);
    if dot(bend_axis, bend_axis) < 1e-8 { bend_axis = vec3(0.0, 0.0, 1.0); }
    bend_axis = normalize(bend_axis);
    let pressure = pose.field.w * pose.field.w;
    let wave = sway_signal(root.xz, pose, profile.y, 0.0);
    let lean = amount * clamp(0.11 * pressure * profile.x * profile.w * pose.hierarchy.x
                              * (1.0 + pose.response.x * wave), -0.15, 0.8);
    let tree_height = max(0.1, height);
    let y = max(0.0, point.y - root.y);
    let angle = lean * clamp(y / tree_height, 0.0, 1.0);
    let curvature = lean / tree_height;
    var center = up * y;
    if abs(curvature) > 1e-6 {
        center = up * (sin(angle) / curvature)
                 + cross(bend_axis, up) * ((1.0 - cos(angle)) / curvature);
        center += turn_axis(up, bend_axis, angle) * max(0.0, y - tree_height);
    }
    return root + center + turn_axis(point - root - up * y, bend_axis, angle);
}

// The wind hierarchy's frame for a vertex (TREE_HIERARCHY), with the tree's `profile`
// (tree_wind.rs, binding 105 of the tree material).
@if(TREE_HIERARCHY)
fn vertex_frame(vertex: TreeVertex, model: mat4x4<f32>, pose: WindPose, profile: vec4<f32>) -> SwayFrame {
    var anchor = vertex.wind_pivot.xyz;
    if vertex.wind_axis.w == 0.0 {
        anchor = vertex.position;
@if(TREE_BRANCH_CARDS)
        anchor = vertex.card_pivot;
    }
    return structural_frame(anchor, vertex.wind_axis, vertex.wind_pivot.w, model, pose, profile);
}

// A vertex's world position in the wind; `viewer` is the view's LOD position, which cards
// turn toward (shadows and temporal motion use the pose's camera instead).
fn animated_position(vertex: TreeVertex, model: mat4x4<f32>, pose: WindPose, profile: vec4<f32>,
                     viewer: vec3<f32>) -> vec4<f32> {
    var p = model * vec4(vertex.position, 1.0);
@if(TREE_HIERARCHY) {
    let frame = vertex_frame(vertex, model, pose, profile);
    p = vec4(sway_point(p.xyz, frame), 1.0);
@if(TREE_BRANCH_CARDS) {
    let pivot = sway_point((model * vec4(vertex.card_pivot, 1.0)).xyz, frame);
    let axis = sway_vector((model * vec4(vertex.card_axis, 0.0)).xyz, frame);
    let normal = sway_vector((model * vec4(vertex.card_normal, 0.0)).xyz, frame);
    if dot(axis, axis) > 0.25 {
        let to_view = select(viewer, pose.camera.xyz, pose.camera.w > 0.5) - pivot;
        // Cards that face the camera (TREE_CARD_FACING) turn toward it; the rest about their axis.
@if(TREE_CARD_FACING)
        let turns_to_camera = vertex.card_facing.x > 0.5;
@else
        let turns_to_camera = false;
        if turns_to_camera {
@if(TREE_CARD_FACING)
            p = vec4(pivot + turn_camera(p.xyz - pivot, normal, to_view, vertex.card_facing.y), 1.0);
        } else {
            let a = normalize(axis);
            let rest = normal - a * dot(normal, a);
            let facing = to_view - a * dot(to_view, a);
            if dot(rest, rest) > 1e-8 && dot(facing, facing) > 1e-8 {
                let angle = atan2(dot(a, cross(normalize(rest), normalize(facing))), dot(normalize(rest), normalize(facing)));
                p = vec4(pivot + turn_axis(p.xyz - pivot, a, angle), 1.0);
            }
        }
    }
@if(VERTEX_UVS_B) {
    // Entire cards rock a little around their own attachment. A zero-length
    // root vector stays exactly zero, including V-card seams and bare twig cards.
    let leaf = select(0.35, 1.0, vertex.uv_b.x > 0.0);
    let flutter = sin(dot(vertex.card_pivot, vec3(1.91, 1.13, -1.37)) + pose.phases.w
                      + dot(model[3].xz, vec2(1.91, -1.37)))
                  * 0.035 * pose.field.w * pose.hierarchy.z * profile.z * leaf;
    p = vec4(pivot + turn_axis(p.xyz - pivot, frame.trunk_axis, flutter), 1.0);
}
}
    return p;
}
@else {
    var weights = vec2(0.0);
@if(VERTEX_UVS_B)
    weights = vertex.uv_b;
    return displaced_position(facing_camera(p, model, vertex, pose.camera, viewer), model, weights, pose);
}
}

fn animated_vector(v: vec3<f32>, vertex: TreeVertex, model: mat4x4<f32>, pose: WindPose,
                   profile: vec4<f32>) -> vec3<f32> {
@if(TREE_HIERARCHY)
    return sway_vector(v, vertex_frame(vertex, model, pose, profile));
@else
    return v;
}
