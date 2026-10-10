//! The wind pose every tree material and impostor reads: the shared vegetation wind field
//! sampled once a frame at the floating origin, with this frame's and the previous frame's
//! pose in one GPU buffer.
#[cfg(test)]
mod gpu_tests;
#[cfg(test)]
mod tests;

use super::tuning::{TreeWindTuning, WindTuningState};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::{
        extract_resource::ExtractResource,
        render_asset::RenderAssets,
        renderer::RenderQueue,
        storage::{GpuShaderBuffer, ShaderBuffer},
    },
};
use bytemuck::{Pod, Zeroable};
use vegetation_render::{VegetationRenderOrigin, VegetationWind};

/// Response in world metres, separate from wind direction, clock and strength.
#[derive(Resource, Clone, Copy, Debug)]
pub struct TreeWindResponse {
    pub branch_amplitude: f32,
    pub flutter_amplitude: f32,
}
impl Default for TreeWindResponse {
    fn default() -> Self {
        Self {
            branch_amplitude: 0.32,
            flutter_amplitude: 0.055,
        }
    }
}

// Legacy UV-weight deformation is bounded to < 1.5 m. Hierarchical assets use
// their own swept bounds because their whole trunks can bend.
pub(super) const MAX_DISPLACEMENT: f32 = 1.5;
const FLUTTER_FREQUENCY: [f64; 2] = [1.91, -1.37];
const FLUTTER_SPEED: f64 = 6.5;

#[derive(Resource, ExtractResource, Clone, Copy, Default, Pod, Zeroable, Debug, PartialEq)]
#[extract_app(bevy::render::RenderApp)]
#[repr(C)]
pub(super) struct WindPose {
    // direction XZ, broad frequency, enabled strength
    field: [f32; 4],
    // Broad, cross, gust and flutter phases at the floating origin.
    phases: [f32; 4],
    // gustiness, branch amplitude, flutter amplitude, padding
    response: [f32; 4],
    // Main camera in the same floating-origin coordinates as this frame's meshes.
    // W indicates a valid sample; the GPU history retains the previous camera too.
    camera: [f32; 4],
    // hierarchy: trunk gain, branch gain, flutter gain, rhythm multiplier
    hierarchy: [f32; 4],
    sway_phases: [f32; 4],
}

fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

impl WindPose {
    fn sample(wind: &VegetationWind, response: TreeWindResponse, origin: [f64; 2]) -> Self {
        let direction = if wind.direction.is_finite() {
            wind.direction.normalize_or(Vec2::X)
        } else {
            Vec2::X
        };
        let frequency = finite(wind.spatial_frequency, 0.12).clamp(0.001, 10.);
        let speed = f64::from(finite(wind.speed, 0.).clamp(0., 20.));
        let time = f64::from(wind.phase_seconds());
        let origin = origin.map(|v| if v.is_finite() { v } else { 0. });
        let broad = (origin[0] * f64::from(direction.x) + origin[1] * f64::from(direction.y))
            * f64::from(frequency)
            - time * speed;
        let cross = (-origin[0] * f64::from(direction.y) + origin[1] * f64::from(direction.x))
            * f64::from(frequency)
            * 0.71
            + time * speed * 0.37;
        // Reduce each independent phase only after evaluating in f64. Reducing broad/cross
        // before the fractional gust combination would introduce jumps at rebase boundaries.
        let phases = [
            broad,
            cross,
            broad * 0.43 - cross * 0.61,
            origin[0] * FLUTTER_FREQUENCY[0]
                + origin[1] * FLUTTER_FREQUENCY[1]
                + time * FLUTTER_SPEED,
        ]
        .map(|v| v.rem_euclid(std::f64::consts::TAU) as f32);
        Self {
            field: [
                direction.x,
                direction.y,
                frequency,
                if wind.enabled {
                    finite(wind.strength, 0.).clamp(0., 2.)
                } else {
                    0.
                },
            ],
            phases,
            response: [
                finite(wind.gustiness, 0.).clamp(0., 1.),
                finite(response.branch_amplitude, 0.).clamp(0., 0.5),
                finite(response.flutter_amplitude, 0.).clamp(0., 0.1),
                0.,
            ],
            camera: [0.; 4],
            hierarchy: [1.; 4],
            sway_phases: [(0.035, 0.55), (0.07, 1.3), (0.14, 2.7), (0.28, 4.3)].map(
                |(k, omega)| {
                    ((origin[0] * f64::from(direction.x) + origin[1] * f64::from(direction.y)) * k
                        - time * omega)
                        .rem_euclid(std::f64::consts::TAU) as f32
                },
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)] // Wind, its tuning and response, the origin and the camera.
pub(super) fn sample_wind(
    wind: Option<ResMut<VegetationWind>>,
    time: Res<Time>,
    tuning: Res<TreeWindTuning>,
    mut state: ResMut<WindTuningState>,
    origin: Option<Res<VegetationRenderOrigin>>,
    response: Res<TreeWindResponse>,
    mut pose: ResMut<WindPose>,
    view: crate::ActiveWorldView,
) {
    *pose = wind.map_or_else(WindPose::default, |mut w| {
        state.apply(&mut w, &tuning, time.delta_secs());
        let mut result = WindPose::sample(&w, *response, origin.map_or([0.; 2], |o| o.world_xz));
        result.hierarchy = [
            tuning.sway.clamp(0., 3.),
            tuning.branches.clamp(0., 3.),
            tuning.flutter.clamp(0., 3.),
            tuning.rhythm.clamp(0.4, 2.),
        ];
        result
    });
    if let Some(view) = view.current() {
        pose.camera = view.transform.translation().extend(1.).to_array();
    }
}

/// This frame's and the previous frame's [`WindPose`], shared by every tree material and
/// the impostors (`super::impostor`).
#[derive(Resource, ExtractResource, Clone)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct WindBuffer(pub(crate) Handle<ShaderBuffer>);
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct WindFrames {
    current: WindPose,
    previous: WindPose,
}
pub(super) fn setup_buffer(mut commands: Commands, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    commands.insert_resource(WindBuffer(buffers.add(ShaderBuffer::new(
        vec![WindFrames::zeroed()],
        RenderAssetUsages::RENDER_WORLD,
    ))));
}
pub(super) fn upload_wind(
    pose: Res<WindPose>,
    handle: Option<Res<WindBuffer>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    queue: Res<RenderQueue>,
    mut previous: Local<Option<WindPose>>,
) {
    let Some(buffer) = handle.and_then(|h| buffers.get(&h.0)) else {
        return;
    };
    let frames = WindFrames {
        current: *pose,
        previous: previous.unwrap_or(*pose),
    };
    // One shared GPU buffer per frame; never dirty/rebind every material to advance time.
    queue.write_buffer(&buffer.buffer, 0, bytemuck::bytes_of(&frames));
    // Keep the last submitted pose, including settings and origin. Time-delta reconstruction
    // cannot handle a paused external transport, toggles, scrubbing or an origin change correctly.
    *previous = Some(*pose);
}
