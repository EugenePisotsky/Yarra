//! CPU/WGSL layouts and shared allocation limits. Keep field order and shader constants in sync.
use bytemuck::{Pod, Zeroable};
use std::mem::size_of;

// Hard device-profile budgets, not density targets. A nonzero capacity-drop counter is a rejected
// configuration: normal population LOD must fit before the emergency guard is reached.
pub(super) const SINGLE_HIGH_CAPACITY: u32 = 32_768;
pub(super) const SPLIT_HIGH_CAPACITY: u32 = 32_768;
// Full-reference density at the 72-root field's normal third-person view exceeds
// the former 278,528-record low arena. Reserve room without increasing Balanced emissions.
pub(super) const LOW_DETAIL_CAPACITY: u32 = 786_432;
pub(super) const LOW_DETAIL_MINIMUM_PARTITION: u32 = 32_768;
pub(super) const PROCEDURAL_INSTANCE_CAPACITY: u32 =
    SINGLE_HIGH_CAPACITY + SPLIT_HIGH_CAPACITY + LOW_DETAIL_CAPACITY;
pub(super) const MAX_DIAGNOSTIC_INSTANCES: u32 = 65_536;
pub(super) const TOPOLOGY_BIN_COUNT: u32 = 4;
pub(super) const WORKGROUP_SIZE: u32 = 64;
pub(super) const GPU_TELEMETRY_WORD_COUNT: u64 = 16;
pub(super) const GPU_TELEMETRY_SIZE: u64 = GPU_TELEMETRY_WORD_COUNT * size_of::<u32>() as u64;
pub(super) const DRAW_INDEXED_ARGS_WORD_COUNT: u64 = 5;
pub(super) const DRAW_ARGS_SIZE: u64 =
    TOPOLOGY_BIN_COUNT as u64 * DRAW_INDEXED_ARGS_WORD_COUNT * size_of::<u32>() as u64;
#[cfg(not(target_os = "ios"))]
pub(super) const TELEMETRY_READBACK_SIZE: u64 = GPU_TELEMETRY_SIZE + DRAW_ARGS_SIZE;

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct WorkItemGpu {
    // xy: page origin x/z, z: page size, w: minimum low-LOD population threshold
    pub(super) page: [f32; 4],
    // xy: minimum world-lattice cell, zw: cell count
    pub(super) domain: [i32; 4],
    // x: candidates/cell, y: candidate count, z: coverage offset, w: coverage resolution
    pub(super) layout: [u32; 4],
    // x: choice offset, y: choice count, z: seed, w: competition group + 1 (zero is none)
    pub(super) population: [u32; 4],
    // x: spacing, y: child radius, z: parent/uniform jitter, w: density retention
    pub(super) growth: [f32; 4],
    // x: radial, y: tangential, z: random, w: field flow direction
    pub(super) direction_weights: [f32; 4],
    // xy: world flow direction, z: requested density, w: terrain contact blocked
    pub(super) flow_density: [f32; 4],
    // x: clump spacing, y: feature jitter, z: boundary softness, w: root attraction
    pub(super) grouping: [f32; 4],
    // x: center retention, y: edge retention, z: falloff, w: group density variation
    pub(super) group_density: [f32; 4],
    // x: shared group direction weight, y: per-root angular jitter; zw: render-origin XZ
    pub(super) orientation: [f32; 4],
    // x: first work item on page, y: work-item count on page, z: placement pattern,
    // w: grouping source (0 none, 1 parent, 2 Voronoi)
    pub(super) peers: [u32; 4],
    // x: surface sample offset, y: surface resolution
    pub(super) surface: [u32; 4],
    // x: maximum height, y: maximum horizontal reach, z: minimum high-LOD threshold,
    // w: maximum low-LOD density
    pub(super) bounds: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct SpeciesChoiceGpu {
    // x: species index, y: fallback topology bin (single ribbon or split broad-leaf cluster)
    pub(super) metadata: [u32; 4],
    // x: normalized cumulative threshold
    pub(super) threshold: [f32; 4],
    // x: high density, y: low density, z: far density, w: reserved
    pub(super) density: [f32; 4],
    // xy: minimum/maximum height, z: short/tall bias, w: group height coherence
    pub(super) height: [f32; 4],
    // x: pair-below height (zero disables), yz: single/split high-topology radii
    pub(super) packing: [f32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct SpeciesGpu {
    pub(super) root_color: [f32; 4],
    // xyz: tip color, w: maximum height
    pub(super) tip_color_height: [f32; 4],
    // xy: height range, zw: half-width range
    pub(super) bounds: [f32; 4],
    // x: high sections, y: low sections, z: blades/render unit, w: longitudinal power
    pub(super) topology: [f32; 4],
    // xy: tilt range; zw: broad-leaf droop range
    pub(super) shape: [f32; 4],
    // x: lateral curve/camber, y: pair spread,
    // z: tangent of maximum ribbon view-opening angle / broad crown radius,
    // w: maximum horizontal reach
    pub(super) shape_secondary: [f32; 4],
    // xy: normalized-height root-handle forward/normal vector,
    // zw: normalized-height tip-handle forward/normal vector
    pub(super) curve_variant_a: [f32; 4],
    pub(super) curve_variant_b: [f32; 4],
    // x: clump color variation, y: roughness, z: transmission, w: normal rounding
    pub(super) material: [f32; 4],
    // x: root AO, y: tip AO, z: high-LOD threshold, w: reserved
    pub(super) shading: [f32; 4],
    // xyz: group coherence for height, complete silhouette, and lateral curve
    pub(super) group_response: [f32; 4],
    // x: short/tall height bias, y: pair-below height, zw: single/split high-topology radii
    pub(super) height_packing: [f32; 4],
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
pub(super) struct ProceduralInstanceGpu {
    // xyz: root, w: packed clump variant and nested LOD rank
    pub(super) root_clump: [f32; 4],
    // x: packed rest direction, y: species index + LOD morph + low flag,
    // z: packed surface normal xz,
    // w: low 24 bits seed + high 8 bits population-density target
    pub(super) geometry: [u32; 4],
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct DebugInstanceGpu {
    // xyz: root, w: rest direction x
    pub(super) root_direction: [f32; 4],
    // x: rest direction z, y: clump variant, z: species index as f32, w: packed surface normal xz
    pub(super) direction_species: [f32; 4],
    // xyz: parent position, w: candidate outcome code
    pub(super) parent_status: [f32; 4],
    // x: local occupancy, y: stable rank, z: group distance, w: group influence
    pub(super) diagnostics: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub(super) struct CameraGpu {
    pub(super) clip_from_world: [f32; 16],
    pub(super) camera_position: [f32; 4],
    // x: vertical focal length in pixels, y: viewport width, z: viewport height,
    // w: far-ribbon screen-space width compensation enabled
    pub(super) projection: [f32; 4],
    // xyz: direction from the surface toward the strongest directional light, w: active
    pub(super) sun_direction: [f32; 4],
    pub(super) sun_radiance: [f32; 4],
    pub(super) ambient_radiance: [f32; 4],
    // x: diffuse, y: specular, z: transmission, w: received-shadow strength
    pub(super) lighting: [f32; 4],
    // xy: normalized world-XZ direction, z: tip displacement / blade height, w: phase seconds
    pub(super) wind: [f32; 4],
    // x: spatial frequency, y: speed, z: gustiness, w: hashed blade flutter
    pub(super) wind_shape: [f32; 4],
    // xy: canonical world-XZ offset of render coordinates; zw reserved.
    pub(super) render_origin: [f32; 4],
    // xy: detail centre, zw: normalized forward XZ (zero selects the camera disk).
    pub(super) lod_focus: [f32; 4],
    pub(super) canopy: [[f32; 4]; 4],
}

impl CameraGpu {
    pub(super) fn geometry_cache_key(mut self) -> Self {
        // These fields are read only by the draw shader. Cloud-driven illumination and
        // the day cycle must not regenerate placement or invalidate prepared wind poses.
        self.sun_direction = [0.0; 4];
        self.sun_radiance = [0.0; 4];
        self.ambient_radiance = [0.0; 4];
        self.lighting = [0.0; 4];
        self.canopy = [[0.0; 4]; 4];
        self
    }
}

#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub(super) struct DebugConfigGpu {
    // x: VegetationDebugMode; y: VegetationDensityMode; z: VegetationLightingMode;
    // w: scene-adaptive single-low arena capacity.
    // Mirrors the two WGSL `vec4<u32>` fields exactly.
    pub(super) values: [u32; 4],
    // x: live work-item count, y: diagnostic atomics, z: prepared blade data available,
    // w: bit 0 early rejection, bit 1 candidate cache, bits 4..7 shape mode, bit 8 opening;
    // bits 16..23 source density for the optional blade-band material (not placement).
    pub(super) workload: [u32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_contracts_have_expected_alignment() {
        assert_eq!(size_of::<WorkItemGpu>(), 208);
        assert_eq!(size_of::<SpeciesChoiceGpu>(), 80);
        assert_eq!(size_of::<SpeciesGpu>(), 192);
        assert_eq!(size_of::<SurfaceSampleGpu>(), 32);
        assert_eq!(size_of::<ProceduralInstanceGpu>(), 32);
        assert_eq!(size_of::<DebugInstanceGpu>(), 64);
        assert_eq!(size_of::<CameraGpu>(), 288);
        assert_eq!(size_of::<DebugConfigGpu>(), 32);
        assert_eq!(GPU_TELEMETRY_SIZE, 64);
        assert_eq!(DRAW_ARGS_SIZE, 80);
        assert_eq!(TELEMETRY_READBACK_SIZE, 144);
    }
}
