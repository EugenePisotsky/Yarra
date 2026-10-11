// Blade vertices. There are no vertex streams: `vertex_index` picks a row and side of the blade
// (or its tip) and `instance_index` the blade placement emitted. Each vertex evaluates its blade's
// curve and wind; the vertex budget is fixed per LOD bin.
import package::shaders::vegetation::types::{Camera, PI, DENSITY_FADE_BAND};
import package::shaders::vegetation::blade::{
    normalize3_or, cubic_bezier, cubic_bezier_derivative, decode_instance, rest_blade,
    rest_curve, twin_blade, wind_curve,
};
import package::shaders::vegetation::draw::bindings::{VertexOutput, instances, types, camera};

const HIGH_ROWS: u32 = constants::HIGH_ROWS;
const LOW_ROWS: u32 = constants::LOW_ROWS;
// The width line turns toward the camera by at most this angle (tangent) where a blade is seen
// edge-on, so blades never vanish into lines: the talk's glancing-angle adjustment.
const MAX_VIEW_OPENING: f32 = 0.47;
// Thinned populations widen their survivors sublinearly, up to this much.
const MAXIMUM_WIDTH_SCALE: f32 = 1.75;
const COVERAGE_WIDTH_EXPONENT: f32 = 0.35;
// Distant blades keep at least this many pixels of half width.
const TARGET_HALF_PIXELS: f32 = 0.6;
const SUBPIXEL_FADE: vec2<f32> = vec2(14.0, 30.0);
// Blade shape across its length: full width low down, narrowing to the point.
const TAPER: f32 = 0.55;
// The field's canopy top, as a share of the type's tallest blades (which lean over).
const CANOPY_SHARE: f32 = 0.55;

// A shallow, clump-stable tilt of the ground normal: neighbouring blades enter and leave the
// broad highlight together.
fn clump_normal(surface_normal: vec3<f32>, clump: f32) -> vec3<f32> {
    let tangent = normalize3_or(cross(vec3(0.0, 1.0, 0.0), surface_normal), vec3(1.0, 0.0, 0.0));
    let bitangent = normalize3_or(cross(surface_normal, tangent), vec3(0.0, 0.0, 1.0));
    let angle = clump * 2.0 * PI;
    return normalize3_or(
        surface_normal + (tangent * cos(angle) + bitangent * sin(angle)) * 0.11,
        surface_normal,
    );
}

// Twin types (Ghost of Tsushima's doubled blades) share one blade's strip between two blades in
// a V from one root: the strip runs down the first blade from its tip to the root, then up the
// second. These are the first blade's segments in the high- and low-detail strips; the second
// takes the rest (high 4 + 5, low 1 + 2: at low detail the first blade is straight).
const TWIN_FIRST_HIGH: u32 = 4u;
const TWIN_FIRST_LOW: u32 = 1u;

fn blade_vertex(vertex_index: u32, instance_index: u32, pose: Camera, previous: bool) -> VertexOutput {
    var instance = instances[instance_index];
    if previous {
        // Both poses use the same blade; only the render origin may differ.
        instance.root_clump.x += camera.render_origin.x - pose.render_origin.x;
        instance.root_clump.z += camera.render_origin.y - pose.render_origin.y;
    }
    var b = decode_instance(instance);
    let grass = types[b.type_index];

    let rows = select(HIGH_ROWS, LOW_ROWS, b.low);
    let tip = vertex_index >= 2u * rows;
    // Position along the strip: its rows, then the tip.
    let path = f32(select(vertex_index >> 1u, rows, tip));
    let side_sign = select(select(-1.0, 1.0, (vertex_index & 1u) != 0u), 0.0, tip);
    var t = path / f32(rows);
    // Segments of this blade in the low-detail strip, which high detail morphs toward.
    var low_segments = f32(LOW_ROWS);
    if (grass.form.x > 0.5) {
        let split = f32(select(TWIN_FIRST_HIGH, TWIN_FIRST_LOW, b.low));
        let first = path < split;
        t = select((path - split) / (f32(rows) - split), (split - path) / split, first);
        low_segments = select(f32(LOW_ROWS - TWIN_FIRST_LOW), f32(TWIN_FIRST_LOW), first);
        b = twin_blade(b, first);
    }
    let blade = rest_blade(b, grass);
    let c = wind_curve(rest_curve(blade), blade, grass, pose);

    var position = cubic_bezier(c.p0, c.p1, c.p2, c.p3, t);
    let tangent = normalize3_or(cubic_bezier_derivative(c.p0, c.p1, c.p2, c.p3, t), blade.up);
    var taper = pow(max(1.0 - t, 0.0), TAPER);
    if (!b.low && b.lod_morph < 1.0) {
        // Near the edge of the high-detail disk, rows slide onto the low mesh's straight
        // segments so the switch to it never pops.
        let s = t * low_segments;
        let k = min(floor(s), low_segments - 1.0);
        let f = s - k;
        let t0 = k / low_segments;
        let t1 = (k + 1.0) / low_segments;
        let low = mix(
            cubic_bezier(c.p0, c.p1, c.p2, c.p3, t0),
            cubic_bezier(c.p0, c.p1, c.p2, c.p3, t1),
            f,
        );
        let low_taper = mix(pow(1.0 - t0, TAPER), pow(max(1.0 - t1, 0.0), TAPER), f);
        position = mix(low, position, b.lod_morph);
        taper = mix(low_taper, taper, b.lod_morph);
    }

    // The width axis, carried along the curve; the normal faces where the blade leans.
    let side = normalize3_or(blade.side - tangent * dot(blade.side, tangent), blade.side);
    let normal = normalize3_or(cross(side, tangent), blade.forward);
    let to_camera = normalize3_or(pose.camera_position.xyz - position, normal);
    // Turn the width line toward the camera-facing one by at most MAX_VIEW_OPENING where the
    // blade is seen edge-on. Lighting keeps the physical frame.
    let facing_side = normalize3_or(cross(tangent, to_camera), side);
    let alignment = dot(facing_side, side);
    let aligned = select(-facing_side, facing_side, alignment >= 0.0);
    let remainder = aligned - side * abs(alignment);
    let remainder_length = length(remainder);
    let opening = min(MAX_VIEW_OPENING, remainder_length / max(abs(alignment), 1e-4));
    let drawn_side = normalize3_or(side + remainder / max(remainder_length, 1e-4) * opening, side);

    // Thinned populations: survivors widen, and blades past the kept share narrow away.
    var width_scale = 1.0;
    if (b.density < 0.999) {
        width_scale = 1.0 - smoothstep(
            max(b.density - DENSITY_FADE_BAND * 0.5, 0.0),
            min(b.density + DENSITY_FADE_BAND * 0.5, 1.0),
            b.lod_rank,
        );
    }
    let distance_to_camera = max(distance(pose.camera_position.xyz, position), 0.05);
    let coverage_scale = clamp(
        pow(1.0 / max(b.density, 0.125), COVERAGE_WIDTH_EXPONENT), 1.0, MAXIMUM_WIDTH_SCALE,
    );
    let projected_half_width = blade.width * 0.5 * pose.projection.x
        * sqrt(max(1.0 - dot(drawn_side, to_camera) * dot(drawn_side, to_camera), 0.0))
        / distance_to_camera;
    let subpixel_scale = mix(
        1.0,
        clamp(TARGET_HALF_PIXELS / max(projected_half_width, 1e-4), 1.0, MAXIMUM_WIDTH_SCALE),
        smoothstep(SUBPIXEL_FADE.x, SUBPIXEL_FADE.y, distance_to_camera),
    );
    let half_width = blade.width * 0.5 * taper * width_scale * max(coverage_scale, subpixel_scale);
    position += drawn_side * side_sign * half_width;

    var output: VertexOutput;
    output.clip_position = pose.clip_from_world * vec4<f32>(position, 1.0);
    output.world_position = position;
    output.world_normal = normal;
    output.side = side;
    output.blade_t = t;
    output.blade_u = side_sign;
    output.color = mix(blade.root_color, blade.tip_color, smoothstep(0.0, 0.55, t));
    // Deep in the canopy the blades shade each other: a lying tip near the ground is as dark as
    // a root, and blades rise out of the dark.
    let canopy_height = max(grass.size.y * CANOPY_SHARE, 0.02);
    let depth = clamp(1.0 - dot(position - blade.root, blade.up) / canopy_height, 0.0, 1.0);
    output.material = vec4<f32>(
        grass.material.y,
        grass.material.z,
        grass.material.x,
        mix(0.15, 1.0, smoothstep(0.0, 0.8, 1.0 - depth)),
    );
    output.canopy = vec2<f32>(depth, canopy_height);
    output.clump_normal = clump_normal(b.surface_normal, b.clump);
    return output;
}
