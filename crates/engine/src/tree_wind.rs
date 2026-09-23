//! Authored foliage weights deform on the GPU using the shared vegetation wind field.
//! This composes with cloud-shaded PBR and uses identical colour/depth/shadow geometry.
#[cfg(test)]
mod gpu_tests;
mod material;
#[cfg(test)]
mod scene_tests;
#[cfg(test)]
mod tests;

use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        renderer::RenderQueue,
        storage::{GpuShaderBuffer, ShaderBuffer},
    },
};
use bytemuck::{Pod, Zeroable};
use vegetation_render::{VegetationRenderOrigin, VegetationWind};

/// Requires the atmosphere/cloud material pipeline and VegetationRenderPlugin's clock.
/// Installed explicitly by game/editor composition, independently of world streaming.
pub struct TreeWindPlugin;
/// Wind producers that run in PostUpdate must finish before this snapshot is taken.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TreeWindSystems;
impl Plugin for TreeWindPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TreeWindResponse>()
            .init_resource::<WindPose>()
            .add_systems(PostUpdate, sample_wind.in_set(TreeWindSystems));
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractResourcePlugin::<WindPose>::default(),
            ExtractResourcePlugin::<WindBuffer>::default(),
            material::TreeWindMaterialPlugin,
        ))
        .add_systems(Startup, setup_buffer);
        app.sub_app_mut(RenderApp)
            .add_systems(Render, upload_wind.in_set(RenderSystems::PrepareResources));
    }
}

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

// Limits below bound every deformation to < 1.5 m, including the vertical flutter.
const MAX_DISPLACEMENT: f32 = 1.5;
const FLUTTER_FREQUENCY: [f64; 2] = [1.91, -1.37];
const FLUTTER_SPEED: f64 = 6.5;

#[derive(Resource, ExtractResource, Clone, Copy, Default, Pod, Zeroable, Debug, PartialEq)]
#[repr(C)]
struct WindPose {
    // direction XZ, broad frequency, enabled strength
    field: [f32; 4],
    // Broad, cross, gust and flutter phases at the floating origin.
    phases: [f32; 4],
    // gustiness, branch amplitude, flutter amplitude, padding
    response: [f32; 4],
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
        }
    }
}

fn sample_wind(
    wind: Option<Res<VegetationWind>>,
    origin: Option<Res<VegetationRenderOrigin>>,
    response: Res<TreeWindResponse>,
    mut pose: ResMut<WindPose>,
) {
    *pose = wind.map_or_else(WindPose::default, |w| {
        WindPose::sample(&w, *response, origin.map_or([0.; 2], |o| o.world_xz))
    });
}

#[derive(Resource, ExtractResource, Clone)]
struct WindBuffer(Handle<ShaderBuffer>);
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct WindFrames {
    current: WindPose,
    previous: WindPose,
}
fn setup_buffer(mut commands: Commands, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    commands.insert_resource(WindBuffer(buffers.add(ShaderBuffer::new(
        bytemuck::bytes_of(&WindFrames::zeroed()),
        RenderAssetUsages::RENDER_WORLD,
    ))));
}
fn upload_wind(
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
    // cannot handle paused study transport, toggles, scrubbing or an origin change correctly.
    *previous = Some(*pose);
}
