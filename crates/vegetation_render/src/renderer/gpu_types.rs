//! CPU/WGSL layouts and shared allocation limits. Keep field order in sync with
//! `assets/shaders/vegetation/types.wesl`; the shaders take the limits from [`shader_defs`].
use super::topology::{
    HIGH_FIRST_INDEX, HIGH_INDEX_COUNT, HIGH_ROWS, LOW_FIRST_INDEX, LOW_INDEX_COUNT, LOW_ROWS,
};

use bevy::shader::ShaderDefVal;
use bytemuck::{Pod, Zeroable};
use std::mem::size_of;

// Hard device-profile budgets, not density targets. A nonzero capacity-drop counter is a rejected
// configuration: normal population LOD must fit before the emergency guard is reached.
pub(super) const HIGH_CAPACITY: u32 = 131_072;
pub(super) const LOW_CAPACITY: u32 = 786_432;
pub(super) const INSTANCE_CAPACITY: u32 = HIGH_CAPACITY + LOW_CAPACITY;
pub(super) const BIN_COUNT: u32 = 2;
pub(super) const WORKGROUP_SIZE: u32 = 64;
pub(super) const GPU_TELEMETRY_WORD_COUNT: u64 = 16;
pub(super) const GPU_TELEMETRY_SIZE: u64 = GPU_TELEMETRY_WORD_COUNT * size_of::<u32>() as u64;
pub(super) const DRAW_INDEXED_ARGS_WORD_COUNT: u64 = 5;

pub(super) const DRAW_ARGS_SIZE: u64 =
    BIN_COUNT as u64 * DRAW_INDEXED_ARGS_WORD_COUNT * size_of::<u32>() as u64;

/// Bytes of one blade's pose (`types.wesl` `BladePose`). Each instance slot holds two, for a twin
/// root's two blades.
pub(super) const BLADE_POSE_SIZE: u64 = 48;

pub(super) const POSES_SIZE: u64 = INSTANCE_CAPACITY as u64 * 2 * BLADE_POSE_SIZE;

/// The pose pass's indirect dispatch: x, y and z workgroup counts.
pub(super) const POSE_DISPATCH_SIZE: u64 = 3 * size_of::<u32>() as u64;

#[cfg(not(target_os = "ios"))]
pub(super) const TELEMETRY_READBACK_SIZE: u64 = GPU_TELEMETRY_SIZE + DRAW_ARGS_SIZE;

const _: () = assert!(
    crate::PROCEDURAL_DISTANCE_METERS as u32 as f32 == crate::PROCEDURAL_DISTANCE_METERS,
    "the shaders take the procedural distance in whole metres"
);

/// Bits 0..15 of an instance's `geometry.y`, below the LOD morph and the low-LOD bit 31: the
/// grass type's index.
pub(super) const TYPE_INDEX_MASK: u32 = 0xffff;

/// The limits every vegetation shader reads as `constants::NAME`.
pub(super) fn shader_defs() -> Vec<ShaderDefVal> {
    [
        ("HIGH_CAPACITY", HIGH_CAPACITY),
        ("LOW_CAPACITY", LOW_CAPACITY),
        ("INSTANCE_CAPACITY", INSTANCE_CAPACITY),
        ("TYPE_INDEX_MASK", TYPE_INDEX_MASK),
        ("WORKGROUP_SIZE", WORKGROUP_SIZE),
        ("HIGH_INDEX_COUNT", HIGH_INDEX_COUNT),
        ("LOW_INDEX_COUNT", LOW_INDEX_COUNT),
        ("HIGH_FIRST_INDEX", HIGH_FIRST_INDEX),
        ("LOW_FIRST_INDEX", LOW_FIRST_INDEX),
        ("HIGH_ROWS", HIGH_ROWS),
        ("LOW_ROWS", LOW_ROWS),
        (
            "PROCEDURAL_DISTANCE_METERS",
            crate::PROCEDURAL_DISTANCE_METERS as u32,
        ),
    ]
    .into_iter()
    .map(|(name, value)| ShaderDefVal::UInt(name.into(), value))
    .collect()
}

/// One resident grass field: a page of one type's candidate lattice.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct WorkItemGpu {
    // xy: page origin x/z, z: page size, w: terrain contact blocked (1) or not (0)
    pub(super) page: [f32; 4],
    // xy: minimum world-lattice cell, zw: cell count
    pub(super) domain: [i32; 4],
    // x: candidate count, y: coverage offset, z: coverage resolution, w: grass type index
    pub(super) field: [u32; 4],
    // x: surface sample offset, y: surface resolution, zw: minimum/maximum height as f32 bits
    pub(super) surface: [u32; 4],
}

/// One grass type as placement and drawing read it.
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub(super) struct GrassTypeGpu {
    // xy: height range, zw: width range
    pub(super) size: [f32; 4],
    // xy: tilt range, zw: bend range
    pub(super) shape: [f32; 4],
    // x: clump size, y: pull, z: clump height variation, w: shared facing
    pub(super) clumps: [f32; 4],
    // x: splay, y: lattice spacing, z: type seed as bits, w: maximum horizontal reach
    pub(super) placement: [f32; 4],
    // xyz: root colour, w: dry clump share
    pub(super) root: [f32; 4],
    // xyz: tip colour, w: clump brightness variation
    pub(super) tip: [f32; 4],
    // xyz: dry tip colour, w: patches (coverage thins whole clumps)
    pub(super) dry_tip: [f32; 4],
    // x: gloss, y: roughness, z: translucency, w: stiffness
    pub(super) material: [f32; 4],
    // x: twin blades (1) or one (0); yzw reserved
    pub(super) form: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct SurfaceSampleGpu {
    // x: height, y: validity
    pub(super) height_validity: [f32; 4],
    pub(super) normal: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct InstanceGpu {
    // xyz: root, w: packed clump key and nested LOD rank
    pub(super) root_clump: [f32; 4],
    // x: packed facing angle and clump distance, y: type index + LOD morph + low flag,
    // z: packed surface normal xz, w: low 24 bits seed + high 8 bits population-density target
    pub(super) geometry: [u32; 4],
}

#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub(super) struct CameraGpu {
    pub(super) clip_from_world: [f32; 16],
    // xyz: world position, w: viewport height
    pub(super) camera_position: [f32; 4],
    // x: vertical focal length in pixels, y: viewport width, z: viewport height, w: high-detail
    // radius in metres
    pub(super) projection: [f32; 4],
    // xyz: direction from the surface toward the strongest directional light, w: active
    pub(super) sun_direction: [f32; 4],
    pub(super) sun_radiance: [f32; 4],
    pub(super) ambient_radiance: [f32; 4],
    // x: diffuse, y: specular, z: transmission, w: received-shadow strength
    pub(super) lighting: [f32; 4],
    // xy: normalized world-XZ direction, z: strength, w: phase seconds
    pub(super) wind: [f32; 4],
    // x: gust spatial frequency, y: speed, z: gustiness, w: flutter
    pub(super) wind_shape: [f32; 4],
    // xy: canonical world-XZ offset of render coordinates; zw reserved.
    pub(super) render_origin: [f32; 4],
    // xy: detail centre, zw: normalized forward XZ (zero selects the camera disk).
    pub(super) lod_focus: [f32; 4],
    // xy: the trample map's canonical centre, z: its side in metres, w: 1 when it is stamped
    pub(super) trample: [f32; 4],
    // The weather's gusts over render XZ (atmosphere::wind::GustField): the wind's direction xz,
    // the pattern's offsets along and across it; x of the second, how far they swell the wind.
    pub(super) gusts: [f32; 4],
    pub(super) gust_swell: [f32; 4],
}

impl CameraGpu {
    pub(super) fn geometry_cache_key(mut self) -> Self {
        // These fields are read only by the draw shader. Cloud-driven illumination and
        // the day cycle must not regenerate placement.
        self.sun_direction = [0.0; 4];
        self.sun_radiance = [0.0; 4];
        self.ambient_radiance = [0.0; 4];
        self.lighting = [0.0; 4];
        self.wind[3] = 0.0;
        self.trample = [0.0; 4];
        self.gusts = [0.0; 4];
        self.gust_swell = [0.0; 4];
        self
    }
}

/// Last frame's camera, which the pose pass bends the blades with again for motion vectors, and
/// the clip transform the grass rasterizes with (`types.wesl` `TemporalGrass`).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct TemporalGrassGpu {
    pub(super) previous: CameraGpu,
    pub(super) raster_clip: [f32; 16],
}

#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub(super) struct ConfigGpu {
    // x: live work-item count, y: diagnostic atomics, z: candidate cache, w: reserved.
    pub(super) workload: [u32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_contracts_have_expected_alignment() {
        assert_eq!(size_of::<WorkItemGpu>(), 64);
        assert_eq!(size_of::<GrassTypeGpu>(), 144);
        assert_eq!(size_of::<SurfaceSampleGpu>(), 32);
        assert_eq!(size_of::<InstanceGpu>(), 32);
        assert_eq!(size_of::<CameraGpu>(), 272);
        assert_eq!(size_of::<TemporalGrassGpu>(), 336);
        assert_eq!(size_of::<ConfigGpu>(), 16);
        assert_eq!(GPU_TELEMETRY_SIZE, 64);
        assert_eq!(DRAW_ARGS_SIZE, 40);
        assert_eq!(TELEMETRY_READBACK_SIZE, 104);
    }
}
