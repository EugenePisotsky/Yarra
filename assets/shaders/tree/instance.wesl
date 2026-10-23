// A tree in the GPU trees' pool (`GpuInstance` in crates/engine/src/trees/gpu.rs), relative to its
// batch's origin: read by the LOD pass (tree/lod.wesl), the draws and the shed pass.
struct Instance {
    position_yaw: vec4<f32>,
    scale: f32,
    form: u32,
    origin: u32,
    // ALIVE, and the forced representation (FORCED_MASK: 0 the distance picks, 1 + n
    // representation n, FORCED_NOTHING none).
    flags: u32,
}

// What the LOD pass (tree/lod.wesl) finds for an instance each frame, for the draws and the shed
// pass: its world-from-local transform (rows; render space), and the wind's strength at it this
// frame and the previous one in x and y (tree/wind.wesl strength_at), which every vertex of the
// tree shares.
struct InstanceFrame {
    transform: array<vec4<f32>, 3>,
    wind: vec4<f32>,
}
