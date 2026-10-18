// Trees on the GPU (crates/engine/src/trees/gpu.rs): every tree and shrub of the resident
// far-object blocks is one instance here. Each frame `lod` picks what every instance draws
// from its projected height, dissolving changes over FADE_SECONDS as the CPU did
// (crates/engine/src/object_lod/timed.rs), and culls each of its primitives against the
// main view and each shadow cascade; the visible ones are appended to the bucket of their
// draw entry in each view. `finalize` then writes each bucket's count into its entry's
// indirect draw.

// An instance, relative to its batch's origin.
struct Instance {
    position_yaw: vec4<f32>,
    scale: f32,
    form: u32,
    origin: u32,
    // ALIVE, and the forced representation (FORCED_MASK: 0 the distance picks, 1 + n
    // representation n, FORCED_NOTHING none).
    flags: u32,
}

// A form's representations (mesh LODs, then its impostor) at unit scale.
struct Form {
    // Each representation's distance band from the root, between the midpoints of the
    // crossfades distance LOD would use; negative for a representation without one.
    band_start: array<vec4<f32>, 2>,
    band_end: array<vec4<f32>, 2>,
    // Each representation's buckets: the first and how many (one per primitive).
    first_bucket: array<vec4<u32>, 2>,
    buckets: array<vec4<u32>, 2>,
    representations: u32,
    meshes: u32,
    ready: u32,
    // The object's height at unit scale.
    height: f32,
}

// One draw entry's bucket: the bounds of what it draws at unit scale in object space, wind
// included (a box's centre and half extents, or with centre.w 1 a sphere's centre and radius
// in extent.x), and its first record and room in a view.
struct Bucket {
    centre: vec4<f32>,
    extent: vec4<f32>,
    records: u32,
    capacity: u32,
    padding: vec2<u32>,
}

struct Camera {
    // Eye (render space) and the frame's time step in seconds.
    eye_dt: vec4<f32>,
    // The main view's frustum half-spaces (normal, distance), Bevy's order.
    planes: array<vec4<f32>, 6>,
    // Each shadow view's, MAX_SHADOW_VIEWS of them.
    shadow_planes: array<vec4<f32>, 48>,
    // Pool length, shadow views, shadow LOD bias, records per view.
    counts: vec4<u32>,
    // Buckets per view, unused, hidden (1 draws nothing), unused.
    sizes: vec4<u32>,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
@group(0) @binding(2) var<storage, read> origins: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> forms: array<Form>;
@group(0) @binding(4) var<storage, read_write> states: array<u32>;
@group(0) @binding(5) var<storage, read_write> counters: array<atomic<u32>>;
@group(0) @binding(6) var<storage, read_write> records: array<u32>;
@group(0) @binding(7) var<storage, read> buckets: array<Bucket>;
@group(0) @binding(8) var<storage, read_write> args: array<u32>;
// Each instance's world-from-local transform (rows; render space), which the tree vertex
// shaders read through the records instead of turning every vertex themselves.
@group(0) @binding(9) var<storage, read_write> transforms: array<array<vec4<f32>, 3>>;

const MAX_SHADOW_VIEWS: u32 = 8u;
const FAR_PLANE: u32 = 5u;
const NEAR_PLANE: u32 = 4u;

const ALIVE: u32 = 16u;
const FORCED_MASK: u32 = 15u;
const FORCED_NOTHING: u32 = 15u;

const FADE_SECONDS: f32 = 0.6;
// A representation keeps drawing until the root is this share past its band.
const HYSTERESIS: f32 = 0.04;

// State: shown (bits 0-3, NONE without), fading from (4-7) to (8-11), FADING, INITIALISED,
// progress (16-31, 0-1).
const NONE: u32 = 15u;
const FADING: u32 = 4096u;
const INITIALISED: u32 = 8192u;
const PROGRESS_SCALE: f32 = 65535.0;

// Records: the instance (bits 0-23) and its dither level + 16 (24-29).
const LEVEL_SHIFT: u32 = 24u;

fn band_start(form: Form, r: u32) -> f32 {
    return form.band_start[r / 4u][r % 4u];
}
fn band_end(form: Form, r: u32) -> f32 {
    return form.band_end[r / 4u][r % 4u];
}

// The representation the distance asks for, keeping `current` until the distance is
// clearly past its band (timed.rs `target`).
fn wanted_representation(form: Form, distance: f32, current: u32) -> u32 {
    if current != NONE {
        let start = band_start(form, current);
        if start >= 0.0 && distance <= band_end(form, current) * (1.0 + HYSTERESIS)
            && distance >= start * (1.0 - HYSTERESIS) {
            return current;
        }
    }
    for (var r = 0u; r < form.representations; r++) {
        let start = band_start(form, r);
        if start >= 0.0 && distance >= start && distance < band_end(form, r) {
            return r;
        }
    }
    return current;
}

fn rotate_y(v: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return vec3(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

// Bounds as a box turned about the vertical by its yaw (centre and half extents, as Bevy
// culls an entity's bounds), or as a sphere (a radius instead).
struct Bounds {
    centre: vec3<f32>,
    extent: vec3<f32>,
    // The box's x and z axes in the world.
    x_axis: vec3<f32>,
    z_axis: vec3<f32>,
    radius: f32,
}

fn sphere_bounds(centre: vec3<f32>, radius: f32) -> Bounds {
    return Bounds(centre, vec3(0.0), vec3(0.0), vec3(0.0), radius);
}

// Where a bucket's primitive of an instance can reach.
fn bucket_bounds(bucket: Bucket, root: vec3<f32>, scale: f32, yaw: f32) -> Bounds {
    let centre = root + rotate_y(bucket.centre.xyz * scale, yaw);
    if bucket.centre.w > 0.5 {
        return sphere_bounds(centre, bucket.extent.x * scale);
    }
    return Bounds(centre, bucket.extent.xyz * scale, rotate_y(vec3(1.0, 0.0, 0.0), yaw),
                  rotate_y(vec3(0.0, 0.0, 1.0), yaw), 0.0);
}

// How far the bounds reach along a plane's (unit) normal.
fn reach(bounds: Bounds, normal: vec3<f32>) -> f32 {
    return bounds.radius + abs(dot(normal, bounds.x_axis)) * bounds.extent.x
        + abs(normal.y) * bounds.extent.y + abs(dot(normal, bounds.z_axis)) * bounds.extent.z;
}

fn in_main_view(bounds: Bounds) -> bool {
    for (var p = 0u; p < 6u; p++) {
        let plane = camera.planes[p];
        if p != FAR_PLANE && dot(plane.xyz, bounds.centre) + plane.w + reach(bounds, plane.xyz) <= 0.0 {
            return false;
        }
    }
    return true;
}

// A cascade's near plane does not cull: a caster between the light and the cascade still
// shades it.
fn in_shadow_view(view: u32, bounds: Bounds) -> bool {
    for (var p = 0u; p < 6u; p++) {
        let plane = camera.shadow_planes[view * 6u + p];
        if p != NEAR_PLANE && dot(plane.xyz, bounds.centre) + plane.w + reach(bounds, plane.xyz) <= 0.0 {
            return false;
        }
    }
    return true;
}

fn append(view: u32, index: u32, bucket: Bucket, record: u32) {
    let slot = atomicAdd(&counters[view * camera.sizes.x + index], 1u);
    if slot < bucket.capacity {
        records[view * camera.counts.w + bucket.records + slot] = record;
    }
}

fn record_of(index: u32, level: i32) -> u32 {
    return index | (u32(level + 16) << LEVEL_SHIFT);
}

@compute @workgroup_size(64)
fn lod(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= camera.counts.x {
        return;
    }
    let instance = instances[index];
    if (instance.flags & ALIVE) == 0u {
        return;
    }
    let form = forms[instance.form];
    let origin = origins[instance.origin];
    if form.ready == 0u {
        states[index] = 0u;
        return;
    }
    let root = origin.xyz + instance.position_yaw.xyz;
    let scale = instance.scale;
    let yaw = instance.position_yaw.w;
    // Bevy's Quat::from_rotation_y, then a uniform scale.
    let c = cos(yaw) * scale;
    let s = sin(yaw) * scale;
    transforms[index] = array<vec4<f32>, 3>(
        vec4(c, 0.0, s, root.x),
        vec4(0.0, scale, 0.0, root.y),
        vec4(-s, 0.0, c, root.z),
    );
    // Bands are at unit scale: switch distances grow with the object's height.
    let distance = length(root - camera.eye_dt.xyz) / scale;

    var state = states[index];
    var shown = state & 15u;
    var outgoing = (state >> 4u) & 15u;
    var incoming = (state >> 8u) & 15u;
    var fading = (state & FADING) != 0u;
    var progress = f32(state >> 16u) / PROGRESS_SCALE;
    let forced = instance.flags & FORCED_MASK;
    if forced != 0u {
        shown = select(forced - 1u, NONE, forced == FORCED_NOTHING);
        fading = false;
    } else if (state & INITIALISED) == 0u {
        // A newly attached instance shows what its distance asks for at once.
        shown = wanted_representation(form, distance, NONE);
    } else if fading {
        if wanted_representation(form, distance, incoming) == outgoing {
            // The camera turned back: dissolve the other way from where it got to.
            let swap = outgoing;
            outgoing = incoming;
            incoming = swap;
            progress = 1.0 - progress;
        }
        progress += camera.eye_dt.w / FADE_SECONDS;
        if progress >= 1.0 {
            shown = incoming;
            fading = false;
        }
    } else {
        let wanted = wanted_representation(form, distance, shown);
        if wanted != shown {
            if shown != NONE && wanted != NONE {
                outgoing = shown;
                incoming = wanted;
                progress = 0.0;
                fading = true;
            } else {
                shown = wanted;
            }
        }
    }
    progress = clamp(progress, 0.0, 1.0);
    state = shown | (outgoing << 4u) | (incoming << 8u) | INITIALISED
        | (u32(round(progress * PROGRESS_SCALE)) << 16u);
    if fading {
        state |= FADING;
    }
    states[index] = state;
    if camera.sizes.z != 0u || origin.w == 0.0 {
        return;
    }

    // What draws, and its dither level: 0 to 16 fading out, -16 to 0 fading in.
    var drawn = array<u32, 2>(NONE, NONE);
    var levels = array<i32, 2>(0, 0);
    if fading {
        let step = i32(round(progress * 16.0));
        drawn = array<u32, 2>(outgoing, incoming);
        levels = array<i32, 2>(step, step - 16);
    } else {
        drawn[0] = shown;
    }

    // Each primitive is culled with its own bounds, as entities were.
    for (var i = 0u; i < 2u; i++) {
        let r = drawn[i];
        if r >= form.representations || abs(levels[i]) >= 16 {
            continue;
        }
        let first = form.first_bucket[r / 4u][r % 4u];
        for (var b = first; b < first + form.buckets[r / 4u][r % 4u]; b++) {
            let bucket = buckets[b];
            if in_main_view(bucket_bounds(bucket, root, scale, yaw)) {
                append(0u, b, bucket, record_of(index, levels[i]));
            }
        }
    }

    // Shadows: each drawn mesh LOD casts from the one `bias` steps coarser, at most the last
    // mesh LOD. Two drawn LODs casting from the same one, mid-fade, cast it whole, so the
    // shadow does not change while the tree dissolves. Impostors cast nothing.
    let views = min(camera.counts.y, MAX_SHADOW_VIEWS);
    if views == 0u {
        return;
    }
    var casts = array<u32, 2>(NONE, NONE);
    var cast_levels = array<i32, 2>(0, 0);
    for (var i = 0u; i < 2u; i++) {
        if drawn[i] >= form.meshes {
            continue;
        }
        let caster = min(drawn[i] + camera.counts.z, form.meshes - 1u);
        if casts[0] == caster {
            cast_levels[0] = 0;
        } else if casts[0] == NONE {
            casts[0] = caster;
            cast_levels[0] = levels[i];
        } else {
            casts[1] = caster;
            cast_levels[1] = levels[i];
        }
    }
    // A sphere round the whole tree, as wide as it is tall, picks the cascades it may shade
    // (as entities' shadows were gathered); each primitive's bounds then must touch them too.
    let height = form.height * scale;
    let tree = sphere_bounds(root + vec3(0.0, height * 0.5, 0.0), height);
    for (var view = 0u; view < views; view++) {
        if !in_shadow_view(view, tree) {
            continue;
        }
        for (var i = 0u; i < 2u; i++) {
            let r = casts[i];
            if r == NONE || abs(cast_levels[i]) >= 16 {
                continue;
            }
            let first = form.first_bucket[r / 4u][r % 4u];
            for (var b = first; b < first + form.buckets[r / 4u][r % 4u]; b++) {
                let bucket = buckets[b];
                if in_shadow_view(view, bucket_bounds(bucket, root, scale, yaw)) {
                    append(1u + view, b, bucket, record_of(index, cast_levels[i]));
                }
            }
        }
    }
}

// One invocation per draw entry (bucket) of every view: its instance count, at most the
// bucket's capacity. The rest of each draw's arguments is written by the CPU.
@compute @workgroup_size(64)
fn finalize(@builtin(global_invocation_id) id: vec3<u32>) {
    let per_view = camera.sizes.x;
    if id.x >= per_view * (1u + MAX_SHADOW_VIEWS) {
        return;
    }
    let view = id.x / per_view;
    let index = id.x % per_view;
    let count = atomicLoad(&counters[view * per_view + index]);
    args[id.x * 5u + 1u] = min(count, buckets[index].capacity);
}
