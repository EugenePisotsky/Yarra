// Eye adaptation (crates/atmosphere/src/adaptation.rs). A histogram of the rendered image's log
// luminance, from every other pixel each way, the sky band at the top weighing less; then one
// thread averages the histogram's 10–90% band, takes the correction towards the reference and
// eases the exposure there. It lands where the tone map reads it, the view's colour grading
// exposure. Bins, band and easing are Bevy's auto exposure's, which measured the calibration.
import bevy_render::view::View;
import bevy_render::globals::Globals;

struct Adaptation {
    // The view's rendered region, in pixels.
    origin: vec2<u32>,
    size: vec2<u32>,
    // The correction is follow * (reference - average) stops, within -darken..brighten.
    follow: f32,
    reference: f32,
    darken: f32,
    brighten: f32,
    // Stops a second towards a brighter scene and towards a darker one.
    to_bright: f32,
    to_dark: f32,
    // Metering weight of the frame's top row, rising to 1 at `land_from` of its height.
    sky_weight: f32,
    land_from: f32,
}

struct Meter {
    bins: array<atomic<u32>, 64>,
    // Stops added to the view's exposure, eased from frame to frame.
    exposure: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<uniform> adaptation: Adaptation;
@group(0) @binding(2) var image: texture_2d<f32>;
@group(0) @binding(3) var<storage, read_write> meter: Meter;
@group(0) @binding(4) var<storage, read_write> view: View;

const BINS: u32 = 64u;
// Metered log2 luminance, -8..8; bin 0 holds what is darker.
const LOWEST: f32 = -8.0;
const RANGE: f32 = 16.0;
const BAND: vec2<f32> = vec2(0.1, 0.9);
// Stops from the target within which the easing slows down.
const SETTLE: f32 = 1.5;

var<workgroup> bins: array<atomic<u32>, BINS>;

fn bin(colour: vec3<f32>) -> u32 {
    let luminance = dot(colour, vec3(0.2125, 0.7154, 0.0721));
    if luminance < exp2(LOWEST) {
        return 0u;
    }
    return u32(saturate((log2(luminance) - LOWEST) / RANGE) * 62.0 + 1.0);
}

@compute @workgroup_size(16, 16, 1)
fn measure(
    @builtin(global_invocation_id) id: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    if index < BINS {
        atomicStore(&bins[index], 0u);
    }
    workgroupBarrier();

    let pixel = id.xy * 2u;
    if all(pixel < adaptation.size) {
        let colour = textureLoad(image, adaptation.origin + pixel, 0).rgb;
        let height = (f32(pixel.y) + 0.5) / f32(adaptation.size.y);
        let weight = mix(adaptation.sky_weight, 1.0, saturate(height / adaptation.land_from));
        atomicAdd(&bins[bin(colour)], u32(weight * 16.0));
    }
    workgroupBarrier();

    if index < BINS {
        atomicAdd(&meter.bins[index], atomicLoad(&bins[index]));
    }
}

@compute @workgroup_size(1, 1, 1)
fn adapt() {
    var below: array<u32, BINS>;
    var total = 0u;
    for (var i = 0u; i < BINS; i += 1u) {
        total += atomicLoad(&meter.bins[i]);
        below[i] = total;
        atomicStore(&meter.bins[i], 0u);
    }

    let first = u32(f32(total) * BAND.x);
    let last = u32(f32(total) * BAND.y);
    var count = 0u;
    var sum = 0.0;
    for (var i = 1u; i < BINS; i += 1u) {
        let n = clamp(below[i], first, last) - clamp(below[i - 1u], first, last);
        sum += f32(n) * f32(i);
        count += n;
    }
    var average = LOWEST;
    if count > 0u {
        average = sum / (f32(count) * 63.0) * RANGE + LOWEST;
    }

    let wanted = clamp(
        adaptation.follow * (adaptation.reference - average),
        -adaptation.darken,
        adaptation.brighten,
    );
    let exposure = meter.exposure;
    let delta = wanted - exposure;
    if delta > 0.0 {
        // Brightening the image: the scene went darker.
        let step = adaptation.to_dark * globals.delta_time;
        meter.exposure = exposure + min(step, delta * step / SETTLE);
    } else {
        let step = adaptation.to_bright * globals.delta_time;
        meter.exposure = exposure + max(-step, delta * step / SETTLE);
    }
    view.color_grading.exposure += meter.exposure;
}
