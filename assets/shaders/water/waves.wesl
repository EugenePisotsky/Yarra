// Waves of the open sea for the sky composite: two dozen deep-water waves from 60 m to 15 cm,
// each in its own direction within a wide fan around the wind, summed as slopes of the surface;
// with fewer or narrower waves the sea read as parallel stripes. Each has the same steepness, so
// together their slope variance follows Cox and Munk's measurements of the sea for the wind.
// Waves too fine for the pixel are left out and their slope variance becomes roughness: far
// water spreads the sun into a broad glitter path instead of shimmering.

const WAVES: u32 = 24u;
// Per wave, generated once (60 m down to 15 cm in equal ratios; deep water, omega = sqrt(g k)
// rounded to whole cycles per `atmosphere::clouds::WAVE_PERIOD`, an hour): direction in the
// wind's frame (along, across), its directions filling a fan around the wind on alternating
// sides and wider for short waves; wavenumber k; angular frequency.
const WAVE_TABLE: array<vec4<f32>, 24> = array<vec4<f32>, 24>(
    vec4(0.963073, 0.269240, 0.104720, 1.014036),
    vec4(0.795977, -0.605327, 0.135882, 1.155408),
    vec4(0.832893, 0.553434, 0.176317, 1.315978),
    vec4(0.761807, -0.647804, 0.228785, 1.497492),
    vec4(0.756497, 0.653997, 0.296866, 1.706932),
    vec4(0.729135, -0.684370, 0.385207, 1.944297),
    vec4(0.482171, 0.876077, 0.499835, 2.214823),
    vec4(0.963654, -0.267154, 0.648575, 2.522001),
    vec4(0.661344, 0.750082, 0.841575, 2.872812),
    vec4(0.978089, -0.208185, 1.092008, 3.272492),
    vec4(0.895559, 0.444942, 1.416965, 3.728023),
    vec4(0.478043, -0.878337, 1.838620, 4.246386),
    vec4(0.664241, 0.747519, 2.385751, 4.838053),
    vec4(0.588850, -0.808242, 3.095695, 5.510004),
    vec4(0.874047, 0.485841, 4.016902, 6.277949),
    vec4(0.680304, -0.732930, 5.212238, 7.150614),
    vec4(0.777331, 0.629092, 6.763279, 8.145452),
    vec4(0.580362, -0.814359, 8.775873, 9.278170),
    vec4(0.931199, 0.364511, 11.387369, 10.569714),
    vec4(0.845941, -0.533277, 14.775985, 12.039281),
    vec4(0.436388, 0.899759, 19.172974, 13.714797),
    vec4(0.953642, -0.300944, 24.878404, 15.622442),
    vec4(0.833927, 0.551875, 32.281638, 17.795377),
    vec4(0.877622, -0.479354, 41.887902, 20.271999),
);
// Per wave: length in metres and phase offset.
const WAVE_SHAPE: array<vec2<f32>, 24> = array<vec2<f32>, 24>(
    vec2(60.000000, 3.027748),
    vec2(46.240041, 1.977291),
    vec2(35.635689, 0.435999),
    vec2(27.463262, 3.967644),
    vec2(21.165039, 0.350017),
    vec2(16.311205, 3.393898),
    vec2(12.570513, 0.375368),
    vec2(9.687684, 0.677181),
    vec2(7.465981, 1.069490),
    vec2(5.753788, 0.298125),
    vec2(4.434257, 5.340432),
    vec2(3.417337, 0.374906),
    vec2(2.633630, 0.796852),
    vec2(2.029653, 4.041154),
    vec2(1.564187, 3.538396),
    vec2(1.205468, 3.953757),
    vec2(0.929015, 3.923766),
    vec2(0.715961, 3.395523),
    vec2(0.551768, 1.234872),
    vec2(0.425230, 4.972685),
    vec2(0.327711, 0.192806),
    vec2(0.252556, 3.771714),
    vec2(0.194637, 4.763771),
    vec2(0.150000, 2.917986),
);

struct WaveSurface {
    // Height gradient of the resolved waves.
    slope: vec2<f32>,
    // Slope variance of the waves too fine to resolve, both axes together.
    variance: f32,
}

// `xz` in metres, `time` in seconds, `wind` a unit direction, `strength` the wind's scale and
// `footprint` the size of the pixel on the water in metres.
fn sea_waves(xz: vec2<f32>, time: f32, wind: vec2<f32>, strength: f32, footprint: f32) -> WaveSurface {
    // Cox and Munk: slope variance 0.003 + 0.00512 U for wind speed U, here 5 m/s times the
    // weather's wind strength; each wave carries an equal share (a cosine averages to 1/2).
    let variance = 0.003 + 0.0256 * strength;
    let steepness = sqrt(2.0 * variance / f32(WAVES));
    let across = vec2(-wind.y, wind.x);
    var surface = WaveSurface(vec2(0.0), 0.0);
    let share = 0.5 * steepness * steepness;
    for (var i = 0u; i < WAVES; i += 1u) {
        let shape = WAVE_SHAPE[i];
        // Resolved while at least about three pixels span the wave. Waves run from longest to
        // shortest, so past the first one left out every later one is too, and only their
        // slope variance remains.
        let resolved = 1.0 - smoothstep(0.2, 0.4, footprint / shape.x);
        if resolved <= 0.0 {
            surface.variance += share * f32(WAVES - i);
            break;
        }
        let wave = WAVE_TABLE[i];
        let direction = wind * wave.x + across * wave.y;
        let phase = wave.z * dot(direction, xz) - wave.w * time + shape.y;
        surface.slope += direction * (steepness * cos(phase) * resolved);
        surface.variance += share * (1.0 - resolved * resolved);
    }
    return surface;
}
