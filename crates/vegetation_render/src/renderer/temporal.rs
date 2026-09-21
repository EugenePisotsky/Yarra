//! One grass draw writes colour, final scene depth and deformation-aware motion.
use super::*;
use bevy::{
    core_pipeline::{Core3dSystems, schedule::Core3d},
    render::{
        GpuResourceAppExt,
        render_resource::*,
        renderer::ViewQuery,
        view::{ViewDepthTexture, ViewTarget},
    },
};
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
    arena: Option<Buffer>,
    previous: Option<CameraGpu>,
    previous_pose: Option<CameraGpu>,
    reused_previous: bool,
    prepare_group: Option<([BufferId; 2], BindGroup)>,
    draw_groups: Vec<([BufferId; 7], BindGroup)>,
    previous_groups: Vec<(BufferId, BindGroup)>,
    source: u64,
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
            arena: None,
            previous: None,
            previous_pose: None,
            reused_previous: false,
            prepare_group: None,
            draw_groups: Vec::new(),
            previous_groups: Vec::new(),
            source: 0,
            was_active: false,
        }
    }
}
pub(super) fn layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "grass temporal data",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX,
            (
                uniform_buffer_sized(false, None),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    )
}
pub(super) fn install(app: &mut SubApp) {
    app.init_gpu_resource::<State>()
        .add_systems(
            RenderGraph,
            reuse_previous
                .after(super::generate)
                .before(blade_preparation::run),
        )
        .add_systems(
            RenderGraph,
            prepare_previous
                .after(blade_preparation::run)
                .before(camera_driver),
        )
        .add_systems(
            Render,
            prepare
                .after(super::prepare)
                .in_set(RenderSystems::PrepareBindGroups),
        )
        .add_systems(
            Core3d,
            draw.in_set(Core3dSystems::MainPass)
                .after(bevy::core_pipeline::core_3d::main_opaque_pass_3d)
                .before(bevy::pbr::main_transmissive_pass_3d)
                .before(bevy::core_pipeline::core_3d::main_transparent_pass_3d),
        );
}
fn prepare(
    mut state: ResMut<State>,
    buffers: Res<VegetationBuffers>,
    queue: Res<RenderQueue>,
    mut frames: Query<&mut TemporalFrame, With<VegetationView>>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    mut preparation: ResMut<blade_preparation::BladePreparation>,
) {
    let (Some(camera), Ok(mut frame)) = (buffers.preparation_camera, frames.single_mut()) else {
        state.previous = None;
        state.previous_pose = None;
        state.prepare_group = None;
        state.draw_groups.clear();
        state.previous_groups.clear();
        preparation.release_history_bindings();
        state.was_active = false;
        state.group = None;
        state.arena = None;
        return;
    };
    if state.arena.is_none() {
        let arena = device.create_buffer(&BufferDescriptor {
            label: Some("grass previous prepared poses"),
            size: preparation.arena.size(),
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        state.group = Some(device.create_bind_group(
            "grass temporal data",
            &cache.get_bind_group_layout(&layout()),
            &BindGroupEntries::sequential((
                state.uniform.as_entire_binding(),
                arena.as_entire_binding(),
            )),
        ));
        state.arena = Some(arena);
    }
    let reset = frame.reset
        || state.source != buffers.uploaded_revision
        || state.was_active != buffers.active
        || state.previous.is_some_and(|p| {
            p.render_origin != camera.render_origin || (p.wind[3] - camera.wind[3]).abs() > 0.25
        });
    // Streaming/world swaps and origin rebases invalidate correspondence rather than creating trails.
    frame.reset |= reset;
    let previous = if reset {
        camera
    } else {
        state.previous.unwrap_or(camera)
    };
    state.previous_pose = Some(previous);
    queue.write_buffer(
        &state.uniform,
        0,
        bytemuck::bytes_of(&Uniform {
            previous,
            raster_clip: frame.raster_clip_from_world.to_cols_array(),
        }),
    );
    state.previous = Some(camera);
    state.source = buffers.uploaded_revision;
    state.was_active = buffers.active;
}
fn draw(
    view: ViewQuery<(
        &Pipeline,
        &TemporalFrame,
        &TemporalMotionTarget,
        &ViewTarget,
        &ViewDepthTexture,
        &bevy::pbr::MeshViewBindGroup,
    )>,
    state: Res<State>,
    buffers: Res<VegetationBuffers>,
    cache: Res<PipelineCache>,
    clouds: Option<Res<atmosphere::clouds::CloudShadowGpu>>,
    mut ctx: RenderContext,
) {
    let (id, frame, motion, target, depth, mesh) = view.into_inner();
    let Some(pipeline) = cache.get_render_pipeline(id.0) else {
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
    if let Some(clouds) = &clouds {
        pass.set_bind_group(2, &*clouds.0, &[]);
    }
    let Some(group) = &state.group else {
        return;
    };
    pass.set_bind_group(3, &**group, &[]);
    pass.set_viewport(0., 0., frame.size.x as f32, frame.size.y as f32, 0., 1.);
    pass.set_index_buffer(*buffers.topology_indices.slice(..), IndexFormat::Uint16);
    for offset in [0, 20, 40, 60] {
        pass.draw_indexed_indirect(&buffers.args, offset);
    }
}

fn reuse_previous(
    mut state: ResMut<State>,
    mut buffers: ResMut<VegetationBuffers>,
    mut preparation: ResMut<blade_preparation::BladePreparation>,
    pipelines: Res<VegetationPipelines>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
) {
    state.reused_previous = false;
    let Some(pose) = state.previous_pose else {
        return;
    };
    if !buffers.active
        || !buffers.preparation_enabled
        || state.arena.is_none()
        || !preparation.contains_pose(&buffers, pose, &cache)
    {
        return;
    }
    // Keep last frame's prepared arena as history, and prepare the new pose into
    // the other arena. No geometry copy, deformation dispatch or GPU readback.
    preparation.swap_arena(state.arena.as_mut().unwrap());
    let arena = state.arena.as_ref().unwrap();
    let previous_id = arena.id();
    let previous_group = state
        .previous_groups
        .iter()
        .find(|(id, _)| *id == previous_id)
        .map(|(_, group)| group.clone())
        .unwrap_or_else(|| {
            device.create_bind_group(
                "grass temporal data",
                &cache.get_bind_group_layout(&layout()),
                &BindGroupEntries::sequential((
                    state.uniform.as_entire_binding(),
                    arena.as_entire_binding(),
                )),
            )
        });
    if !state
        .previous_groups
        .iter()
        .any(|(id, _)| *id == previous_id)
    {
        if state.previous_groups.len() == 2 {
            state.previous_groups.remove(0);
        }
        state
            .previous_groups
            .push((previous_id, previous_group.clone()));
    }
    state.group = Some(previous_group);
    let draw_key = [
        buffers.procedural_instances.id(),
        buffers.diagnostic_instances.id(),
        buffers.species.id(),
        buffers.camera.id(),
        buffers.debug_config.id(),
        preparation.arena.id(),
        buffers.canopy_boundary.buffer.id(),
    ];
    let draw = state
        .draw_groups
        .iter()
        .find(|(key, _)| *key == draw_key)
        .map(|(_, group)| group.clone())
        .unwrap_or_else(|| {
            create_draw_bind_group(
                &device,
                &cache.get_bind_group_layout(&pipelines.draw_layout),
                &buffers.procedural_instances,
                &buffers.diagnostic_instances,
                &buffers.species,
                &buffers.camera,
                &buffers.debug_config,
                &preparation.arena,
                &buffers.canopy_boundary.buffer,
            )
        });
    if !state.draw_groups.iter().any(|(key, _)| *key == draw_key) {
        if state.draw_groups.len() == 2 {
            state.draw_groups.remove(0);
        }
        state.draw_groups.push((draw_key, draw.clone()));
    }
    buffers.draw_bind_group = draw;
    state.reused_previous = true;
}

#[cfg(test)]
pub(super) fn reused_previous(world: &World) -> bool {
    world.resource::<State>().reused_previous
}

fn prepare_previous(
    mut state: ResMut<State>,
    buffers: Res<VegetationBuffers>,
    preparation: Res<blade_preparation::BladePreparation>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    if state.reused_previous {
        return;
    }
    let (Some(arena), Some(pipeline)) = (
        &state.arena,
        cache.get_compute_pipeline(preparation.prepare_pipeline),
    ) else {
        return;
    };
    if !buffers.active || !buffers.preparation_enabled {
        return;
    }
    if state
        .prepare_group
        .as_ref()
        .is_none_or(|(key, _)| *key != [buffers.species.id(), arena.id()])
    {
        let group = device.create_bind_group(
            "grass previous blade preparation",
            &cache.get_bind_group_layout(&preparation.layout),
            &BindGroupEntries::sequential((
                buffers.procedural_instances.as_entire_binding(),
                buffers.species.as_entire_binding(),
                state.uniform.as_entire_binding(),
                buffers.debug_config.as_entire_binding(),
                buffers.args.as_entire_binding(),
                arena.as_entire_binding(),
            )),
        );
        state.prepare_group = Some(([buffers.species.id(), arena.id()], group));
    }
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("grass previous blade preparation"),
            ..default()
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &state.prepare_group.as_ref().unwrap().1, &[]);
    pass.dispatch_workgroups_indirect(&preparation.dispatch, 0);
}
