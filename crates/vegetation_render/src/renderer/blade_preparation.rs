//! Per-blade curve preparation. Overflow uses the original vertex calculation, not fewer blades.
use super::*;

#[cfg(test)]
pub(super) const BLADE_CAPACITY: u64 = 131_072;
pub(super) const BLADE_BYTES: u64 = 128;
#[cfg(test)]
const ARENA_BYTES: u64 = PROCEDURAL_INSTANCE_CAPACITY as u64 * 4 + BLADE_CAPACITY * BLADE_BYTES;
const SHADER: &str = "shaders/vegetation_prepare_blades.wgsl";

#[derive(Resource)]
pub(super) struct BladePreparation {
    pub arena: Buffer,
    pub(super) dispatch: Buffer,
    pub(super) layout: BindGroupLayoutDescriptor,
    setup_layout: BindGroupLayoutDescriptor,
    setup_pipeline: CachedComputePipelineId,
    pub(super) prepare_pipeline: CachedComputePipelineId,
    bind_groups: Vec<(
        [bevy::render::render_resource::BufferId; 2],
        BindGroup,
        BindGroup,
    )>,
    last_key: Option<PreparationKey>,
}

#[derive(Clone, Copy, PartialEq)]
struct PreparationKey {
    generation: u64,
    source: u64,
    camera: CameraGpu,
    config: DebugConfigGpu,
    pipelines: [ComputePipelineId; 2],
}

impl PreparationKey {
    fn new(
        generation: u64,
        source: u64,
        mut camera: CameraGpu,
        config: DebugConfigGpu,
        pipelines: [ComputePipelineId; 2],
    ) -> Self {
        camera = camera.geometry_cache_key();
        if camera.wind[2] <= 1e-5 || config.workload[3] & (1 << 9) != 0 {
            camera.wind[3] = 0.0;
        }
        Self {
            generation,
            source,
            camera,
            config,
            pipelines,
        }
    }
}

impl BladePreparation {
    pub(super) fn swap_arena(&mut self, previous: &mut Buffer) {
        std::mem::swap(&mut self.arena, previous);
        self.last_key = None;
    }
    pub(super) fn release_history_bindings(&mut self) {
        let arena = self.arena.id();
        self.bind_groups.retain(|(key, _, _)| key[1] == arena);
    }

    /// The arena still holds this exact pose before this frame's preparation overwrites it.
    /// Identity includes placement generation: compacted instance indices alone are not stable.
    pub(super) fn contains_pose(
        &self,
        buffers: &VegetationBuffers,
        camera: CameraGpu,
        cache: &PipelineCache,
    ) -> bool {
        let (Some(setup), Some(prepare), Some(inputs)) = (
            cache.get_compute_pipeline(self.setup_pipeline),
            cache.get_compute_pipeline(self.prepare_pipeline),
            buffers.generation_inputs,
        ) else {
            return false;
        };
        self.last_key
            == Some(PreparationKey::new(
                buffers.generation_serial,
                buffers.uploaded_revision,
                camera,
                inputs.config,
                [setup.id(), prepare.id()],
            ))
    }

    pub fn available(&self, cache: &PipelineCache) -> bool {
        cache.get_compute_pipeline(self.setup_pipeline).is_some()
            && cache.get_compute_pipeline(self.prepare_pipeline).is_some()
    }
}

impl FromWorld for BladePreparation {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let capacity = world
            .get_resource::<crate::VegetationPreparationCapacity>()
            .copied()
            .unwrap_or_default()
            .0;
        assert!(
            (32_768..=524_288).contains(&capacity),
            "prepared blades must be in 32768..524288"
        );
        let arena_bytes = PROCEDURAL_INSTANCE_CAPACITY as u64 * 4 + capacity * BLADE_BYTES;
        assert!(
            arena_bytes <= u64::from(device.limits().max_storage_buffer_binding_size),
            "prepared-blade experiment exceeds device storage-buffer limit"
        );
        let arena = device.create_buffer(&BufferDescriptor {
            label: Some("vegetation bounded prepared blades"),
            size: arena_bytes,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let dispatch = device.create_buffer(&BufferDescriptor {
            label: Some("vegetation blade preparation dispatch"),
            size: 12,
            usage: BufferUsages::STORAGE | BufferUsages::INDIRECT,
            mapped_at_creation: false,
        });
        let setup_layout = BindGroupLayoutDescriptor::new(
            "vegetation blade preparation",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        // The indirect argument buffer must not remain bound as writable storage in the
        // preparation dispatch, even though that entry point does not access it.
        let layout = BindGroupLayoutDescriptor::new(
            "vegetation prepared blades",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let shader = world.resource::<AssetServer>().load(SHADER);
        let cache = world.resource::<PipelineCache>();
        let pipeline = |entry: &'static str| {
            cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some(format!("vegetation blade {entry}").into()),
                layout: vec![if entry == "prepare_dispatch" {
                    setup_layout.clone()
                } else {
                    layout.clone()
                }],
                shader: shader.clone(),
                entry_point: Some(entry.into()),
                ..default()
            })
        };
        Self {
            arena,
            dispatch,
            setup_pipeline: pipeline("prepare_dispatch"),
            prepare_pipeline: pipeline("prepare"),
            layout,
            setup_layout,
            bind_groups: Vec::new(),
            last_key: None,
        }
    }
}

pub(super) fn run(
    mut context: RenderContext,
    mut preparation: ResMut<BladePreparation>,
    buffers: Res<VegetationBuffers>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    diagnostics: Res<VegetationDiagnostics>,
) {
    let enabled = buffers.active && buffers.preparation_enabled;
    diagnostics.update(|s| {
        s.blade_preparation_enabled = enabled;
        s.blade_preparation_bytes = preparation.arena.size() + 12;
    });
    if !enabled {
        if preparation.last_key.take().is_some() {
            context
                .command_encoder()
                .clear_buffer(&buffers.gpu_telemetry, 11 * 4, Some(2 * 4));
        }
        return;
    }
    let (Some(setup), Some(prepare)) = (
        cache.get_compute_pipeline(preparation.setup_pipeline),
        cache.get_compute_pipeline(preparation.prepare_pipeline),
    ) else {
        preparation.last_key = None;
        return;
    };
    let Some(camera) = buffers.preparation_camera else {
        return;
    };
    let key = PreparationKey::new(
        buffers.generation_serial,
        buffers.uploaded_revision,
        camera,
        buffers
            .generation_inputs
            .expect("active preparation has generation inputs")
            .config,
        [setup.id(), prepare.id()],
    );
    if preparation.last_key == Some(key) {
        diagnostics.update(|s| s.blade_preparation_reuses += 1);
        return;
    }
    let binding_key = [buffers.species.id(), preparation.arena.id()];
    let binding_index = preparation
        .bind_groups
        .iter()
        .position(|(key, _, _)| *key == binding_key);
    let binding_index = if let Some(index) = binding_index {
        index
    } else {
        let group = device.create_bind_group(
            "vegetation blade preparation",
            &cache.get_bind_group_layout(&preparation.setup_layout),
            &BindGroupEntries::sequential((
                buffers.procedural_instances.as_entire_binding(),
                buffers.species.as_entire_binding(),
                buffers.camera.as_entire_binding(),
                buffers.debug_config.as_entire_binding(),
                buffers.args.as_entire_binding(),
                preparation.arena.as_entire_binding(),
                preparation.dispatch.as_entire_binding(),
                buffers.gpu_telemetry.as_entire_binding(),
            )),
        );
        let draw_preparation = device.create_bind_group(
            "vegetation prepared blades",
            &cache.get_bind_group_layout(&preparation.layout),
            &BindGroupEntries::sequential((
                buffers.procedural_instances.as_entire_binding(),
                buffers.species.as_entire_binding(),
                buffers.camera.as_entire_binding(),
                buffers.debug_config.as_entire_binding(),
                buffers.args.as_entire_binding(),
                preparation.arena.as_entire_binding(),
            )),
        );
        if preparation.bind_groups.len() == 2 {
            preparation.bind_groups.remove(0);
        }
        preparation
            .bind_groups
            .push((binding_key, group, draw_preparation));
        preparation.bind_groups.len() - 1
    };
    let recorder = context.diagnostic_recorder();
    let mut pass = context
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("vegetation shared blade curves and wind"),
            ..default()
        });
    let recorder_ref = recorder.as_deref();
    let span = recorder_ref.pass_span(&mut pass, "vegetation_v2_prepare_blades");
    pass.set_bind_group(0, &preparation.bind_groups[binding_index].1, &[]);
    pass.set_pipeline(setup);
    pass.dispatch_workgroups(1, 1, 1);
    pass.set_pipeline(prepare);
    pass.set_bind_group(0, &preparation.bind_groups[binding_index].2, &[]);
    pass.dispatch_workgroups_indirect(&preparation.dispatch, 0);
    span.end(&mut pass);
    drop(pass);
    preparation.last_key = Some(key);
    diagnostics.update(|s| s.blade_preparation_dispatches += 1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_pose_ignores_lighting_but_tracks_wind_and_placement_identity() {
        let mut camera = CameraGpu::zeroed();
        camera.wind[2] = 1.0;
        let config = DebugConfigGpu::zeroed();
        let pipelines = [ComputePipelineId::new(), ComputePipelineId::new()];
        let key = PreparationKey::new(5, 2, camera, config, pipelines);
        camera.sun_radiance = [100.0; 4];
        camera.ambient_radiance = [0.2; 4];
        camera.sun_direction = [0.5; 4];
        camera.lighting = [1.0; 4];
        assert!(key == PreparationKey::new(5, 2, camera, config, pipelines));
        assert!(key != PreparationKey::new(6, 2, camera, config, pipelines));
        camera.wind[3] = 0.05;
        assert!(key != PreparationKey::new(5, 2, camera, config, pipelines));
    }

    #[test]
    fn motion_mask_reuses_fixed_geometry_but_switching_back_restores_live_wind() {
        let mut camera = CameraGpu::zeroed();
        camera.wind[2] = 0.82;
        let live = DebugConfigGpu::zeroed();
        let mut fixed = live;
        fixed.workload[3] |= 1 << 9;
        let pipelines = [ComputePipelineId::new(), ComputePipelineId::new()];
        let fixed_key = PreparationKey::new(1, 1, camera, fixed, pipelines);
        let live_key = PreparationKey::new(1, 1, camera, live, pipelines);
        camera.wind[3] = 0.5;
        assert!(fixed_key == PreparationKey::new(1, 1, camera, fixed, pipelines));
        assert!(live_key != PreparationKey::new(1, 1, camera, live, pipelines));
        assert!(fixed_key != PreparationKey::new(1, 1, camera, live, pipelines));
    }

    #[test]
    fn idle_wind_reuses_but_animation_movement_generation_and_reloads_invalidate() {
        let camera = CameraGpu::zeroed();
        let config = DebugConfigGpu::zeroed();
        let pipelines = [ComputePipelineId::new(), ComputePipelineId::new()];
        let key = PreparationKey::new(1, 1, camera, config, pipelines);
        let mut shaded = camera;
        shaded.canopy = vegetation::CanopyShading::experiment().packed([32.0, -64.0]);
        assert!(key == PreparationKey::new(1, 1, shaded, config, pipelines));
        let mut animated = camera;
        animated.wind[3] = 2.0;
        assert!(key == PreparationKey::new(1, 1, animated, config, pipelines));
        animated.wind[2] = 0.3;
        assert!(key != PreparationKey::new(1, 1, animated, config, pipelines));
        animated = camera;
        animated.camera_position[0] = 1.0;
        assert!(key != PreparationKey::new(1, 1, animated, config, pipelines));
        assert!(key != PreparationKey::new(2, 1, camera, config, pipelines));
        assert!(key != PreparationKey::new(1, 2, camera, config, pipelines));
        assert!(
            key != PreparationKey::new(
                1,
                1,
                camera,
                config,
                [ComputePipelineId::new(), pipelines[1]]
            )
        );
        let mut density = config;
        density.values[1] = 1;
        assert!(key != PreparationKey::new(1, 1, camera, density, pipelines));
        const {
            // Includes the expanded full-reference instance-to-blade index table.
            assert!(ARENA_BYTES + 12 <= 20 * 1024 * 1024);
        }
    }
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod lod_gpu_tests;
