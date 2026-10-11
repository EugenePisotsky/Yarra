//! One grass draw writes colour, final scene depth and deformation-aware motion. The vertex
//! shader poses every blade twice, with this frame's camera and wind and with last frame's.
use super::{buffers::VegetationBuffers, gpu_types::CameraGpu};
use crate::{VegetationSceneState, VegetationView};
use bevy::{
    core_pipeline::{Core3dSystems, schedule::Core3d},
    prelude::*,
    render::{
        GpuResourceAppExt, Render, RenderSystems,
        render_resource::{
            BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, Buffer,
            BufferDescriptor, BufferUsages, CachedRenderPipelineId, IndexFormat, LoadOp,
            Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
            ShaderStages, StoreOp, binding_types::uniform_buffer_sized,
        },
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        view::{ViewDepthStencilTexture, ViewTarget},
    },
};
use bytemuck::{Pod, Zeroable};
use std::mem::size_of;
use upscaling::temporal::{TemporalFrame, TemporalMotionTarget};

#[derive(Component)]
pub(super) struct Pipeline(pub CachedRenderPipelineId);
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    previous: CameraGpu,
    raster_clip: [f32; 16],
}
#[derive(Resource)]
struct State {
    uniform: Buffer,
    group: Option<BindGroup>,
    previous: Option<CameraGpu>,
    catalog_revision: u64,
    was_active: bool,
}
impl FromWorld for State {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("grass previous pose"),
            size: size_of::<Uniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            uniform,
            group: None,
            previous: None,
            catalog_revision: 0,
            was_active: false,
        }
    }
}
pub(super) fn layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "grass temporal data",
        &BindGroupLayoutEntries::single(ShaderStages::VERTEX, uniform_buffer_sized(false, None)),
    )
}
pub(super) fn install(app: &mut SubApp) {
    app.init_gpu_resource::<State>()
        .add_systems(
            Render,
            prepare
                .after(super::prepare::prepare)
                .in_set(RenderSystems::PrepareBindGroups),
        )
        .add_systems(
            Core3d,
            draw.in_set(Core3dSystems::MainPass)
                .after(bevy::core_pipeline::core_3d::main_opaque_pass_3d)
                .after(upscaling::temporal::CompleteTemporalMotion)
                .before(bevy::pbr::main_transmissive_pass_3d)
                .before(bevy::core_pipeline::core_3d::main_transparent_pass_3d),
        );
}
#[allow(clippy::too_many_arguments)] // Independent render-world resources.
fn prepare(
    mut state: ResMut<State>,
    buffers: Res<VegetationBuffers>,
    queue: Res<RenderQueue>,
    mut frames: Query<&mut TemporalFrame, With<VegetationView>>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    scene: Option<Res<VegetationSceneState>>,
    diagnostics: Option<Res<upscaling::UpscalingDiagnostics>>,
) {
    let (Some(camera), Ok(mut frame)) = (buffers.camera_values, frames.single_mut()) else {
        state.previous = None;
        state.was_active = false;
        state.group = None;
        return;
    };
    if state.group.is_none() {
        state.group = Some(device.create_bind_group(
            "grass temporal data",
            &cache.get_bind_group_layout(&layout()),
            &BindGroupEntries::single(state.uniform.as_entire_binding()),
        ));
    }
    let catalog_revision = scene.as_ref().map_or(0, |s| s.catalog_revision());
    let catalog_changed = buffers.active && state.catalog_revision != catalog_revision;
    let active_changed = state.was_active != buffers.active;
    let pose_discontinuity = state.was_active
        && buffers.active
        && state.previous.is_some_and(|p| {
            p.render_origin != camera.render_origin || (p.wind[3] - camera.wind[3]).abs() > 0.25
        });
    let reset = frame.reset || catalog_changed || pose_discontinuity;
    // Residency repacks instance indices, not blade identity: both poses come from the same
    // root and seed, so page loads need no reset.
    if diagnostics.as_ref().is_some_and(|d| d.temporal_timing)
        && (active_changed || catalog_changed || pose_discontinuity)
    {
        info!(
            "GRASS_TEMPORAL_HISTORY source_revision={} catalog_changed={catalog_changed} active_changed={active_changed} pose_discontinuity={pose_discontinuity} reset={reset}",
            buffers.uploaded_revision
        );
    }
    frame.reset |= reset;
    let mut previous = if reset || active_changed {
        camera
    } else {
        state.previous.unwrap_or(camera)
    };
    if active_changed && !reset {
        // Newly enabled grass has no old deformation, but still has camera motion.
        previous.clip_from_world = frame.previous_clip_from_world.to_cols_array();
    }
    queue.write_buffer(
        &state.uniform,
        0,
        bytemuck::bytes_of(&Uniform {
            previous,
            raster_clip: frame.raster_clip_from_world.to_cols_array(),
        }),
    );
    state.previous = Some(camera);
    // An edit while disabled must still invalidate when the new catalog is first drawn.
    if buffers.active {
        state.catalog_revision = catalog_revision;
    }
    state.was_active = buffers.active;
}
fn draw(
    view: ViewQuery<(
        &Pipeline,
        &TemporalFrame,
        &TemporalMotionTarget,
        &ViewTarget,
        &ViewDepthStencilTexture,
        &bevy::pbr::MeshViewBindGroup,
    )>,
    state: Res<State>,
    buffers: Res<VegetationBuffers>,
    cache: Res<PipelineCache>,
    environment: Option<Res<atmosphere::environment::EnvironmentSurfaceGpu>>,
    mut ctx: RenderContext,
) {
    let (id, frame, motion, target, depth, mesh) = view.into_inner();
    let Some(pipeline) = cache.get_render_pipeline(id.0) else {
        return;
    };
    let Some(group) = &state.group else {
        return;
    };
    if !buffers.active {
        return;
    }
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("grass colour + motion"),
            color_attachments: &[
                Some(RenderPassColorAttachment {
                    view: target.main_texture_view(),
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Load,
                        store: StoreOp::Store,
                    },
                }),
                Some(RenderPassColorAttachment {
                    view: &motion.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Load,
                        store: StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            ..default()
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &*mesh.main, &mesh.main_offsets);
    pass.set_bind_group(1, &*buffers.draw_bind_group, &[]);
    if let Some(environment) = &environment {
        pass.set_bind_group(2, &*environment.0, &[]);
    }
    pass.set_bind_group(3, &**group, &[]);
    pass.set_viewport(0., 0., frame.size.x as f32, frame.size.y as f32, 0., 1.);
    pass.set_index_buffer(*buffers.topology_indices.slice(..), IndexFormat::Uint16);
    for offset in [0, 20] {
        pass.draw_indexed_indirect(&buffers.args, offset);
    }
}
