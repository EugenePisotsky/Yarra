// A burst of snow shaken off a bough, falling as powder and clumps (weather/ambient.wesl). The GPU
// trees write them into the ambient particles' ring (tree/shed_shake.wesl,
// crates/atmosphere/src/ambient_particles.rs).
struct SnowFall {
    // xyz: render-space start of the snow along its bough; w: when it fell, on the game clock.
    start: vec4<f32>,
    // xyz: from there to where the snow ends; w: how far across it lay.
    along: vec4<f32>,
    // xyz: the bough's velocity as it was shaken, m/s; w: the ground the snow lands on.
    motion: vec4<f32>,
    // x: the share of a whole burst, 0..1; y: a seed, a whole number below 2^24.
    shape: vec4<f32>,
}
