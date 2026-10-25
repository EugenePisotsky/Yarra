//! Compute layouts and the G-buffer draw's specialization with and without motion.
use super::{gpu_types::shader_defs, temporal};

use bevy::{
    core_pipeline::{
        core_3d::CORE_3D_DEPTH_FORMAT, deferred::DEFERRED_PREPASS_FORMAT,
        prepass::MOTION_VECTOR_PREPASS_FORMAT,
    },
    prelude::*,
    render::render_resource::{
        BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId, Canonical,
        ColorTargetState, CompareFunction, ComputePipelineDescriptor, DepthStencilState,
        FragmentState, FrontFace, PipelineCache, PolygonMode, PrimitiveState, PrimitiveTopology,
        RenderPipeline, RenderPipelineDescriptor, SamplerBindingType, ShaderStages, Specializer,
        SpecializerKey, TextureSampleType, Variants, VertexState,
        binding_types::{
            sampler, storage_buffer_read_only_sized, storage_buffer_sized, texture_2d,
            uniform_buffer_sized,
        },
    },
    shader::ShaderDefVal,
};

use std::borrow::Cow;

const SCHEDULE_SHADER_PATH: &str = "shaders/vegetation/schedule.wesl";
const PLACEMENT_SHADER_PATH: &str = "shaders/vegetation/placement.wesl";
const CANDIDATE_CACHE_SHADER_PATH: &str = "shaders/vegetation/candidate_cache.wesl";
const POSE_SHADER_PATH: &str = "shaders/vegetation/pose.wesl";
const DRAW_SHADER_PATH: &str = "shaders/vegetation/draw.wesl";

#[derive(Resource)]
pub(super) struct VegetationPipelines {
    pub(super) schedule_layout: BindGroupLayoutDescriptor,
    pub(super) compute_layout: BindGroupLayoutDescriptor,
    pub(super) pose_layout: BindGroupLayoutDescriptor,
    pub(super) draw_layout: BindGroupLayoutDescriptor,
    pub(super) schedule: CachedComputePipelineId,
    pub(super) generate: CachedComputePipelineId,
    pub(super) finalize: CachedComputePipelineId,
    pub(super) cache_build: CachedComputePipelineId,
    pub(super) cache_finish: CachedComputePipelineId,
    pub(super) pose: CachedComputePipelineId,
    pub(super) draw_variants: Variants<RenderPipeline, VegetationPipelineSpecializer>,
}

impl FromWorld for VegetationPipelines {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        let schedule_shader = asset_server.load(SCHEDULE_SHADER_PATH);
        let placement_shader = asset_server.load(PLACEMENT_SHADER_PATH);
        let candidate_cache_shader = asset_server.load(CANDIDATE_CACHE_SHADER_PATH);
        let pose_shader = asset_server.load(POSE_SHADER_PATH);
        let draw_shader = asset_server.load(DRAW_SHADER_PATH);
        let pipeline_cache = world.resource::<PipelineCache>();
        let schedule_layout = BindGroupLayoutDescriptor::new(
            "vegetation visible work scheduling",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                ),
            ),
        );
        let compute_layout = BindGroupLayoutDescriptor::new(
            "vegetation placement",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let pose_layout = BindGroupLayoutDescriptor::new(
            "vegetation blade poses",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    // The trample map now and last frame, and its wrapping sampler.
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let draw_layout = BindGroupLayoutDescriptor::new(
            "vegetation draw",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    storage_buffer_read_only_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                ),
            ),
        );
        let schedule = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("vegetation visible work scheduling".into()),
            layout: vec![schedule_layout.clone()],
            shader: schedule_shader,
            shader_defs: shader_defs(),
            entry_point: Some(Cow::Borrowed("schedule")),
            ..default()
        });
        let placement_pipeline =
            |label: &'static str, shader: &Handle<Shader>, entry: &'static str| {
                pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                    label: Some(label.into()),
                    layout: vec![compute_layout.clone()],
                    shader: shader.clone(),
                    shader_defs: shader_defs(),
                    entry_point: Some(entry.into()),
                    ..default()
                })
            };
        let generate = placement_pipeline(
            "vegetation classify once and emit",
            &placement_shader,
            "generate",
        );
        let finalize = placement_pipeline(
            "vegetation placement finalize",
            &placement_shader,
            "finalize",
        );
        let cache_build = placement_pipeline(
            "vegetation candidate build_candidate_cache",
            &candidate_cache_shader,
            "build_candidate_cache",
        );
        let cache_finish = placement_pipeline(
            "vegetation candidate finish_candidate_cache",
            &candidate_cache_shader,
            "finish_candidate_cache",
        );
        let pose = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("vegetation blade poses".into()),
            layout: vec![pose_layout.clone()],
            shader: pose_shader,
            shader_defs: shader_defs(),
            entry_point: Some(Cow::Borrowed("pose")),
            ..default()
        });
        let mut draw_defs = shader_defs();
        draw_defs.extend([
            "ENVIRONMENT_SURFACE".into(),
            ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 2),
        ]);
        let draw_descriptor = RenderPipelineDescriptor {
            label: Some("vegetation draw".into()),
            // Set by the specializer.
            layout: Vec::new(),
            vertex: VertexState {
                shader: draw_shader.clone(),
                entry_point: Some(Cow::Borrowed("vertex")),
                shader_defs: draw_defs.clone(),
                buffers: Vec::new(),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: draw_shader,
                entry_point: Some(Cow::Borrowed("fragment")),
                shader_defs: draw_defs,
                // The G-buffer word and the grass's own texel; the specializer adds motion.
                targets: vec![
                    Some(ColorTargetState::from(DEFERRED_PREPASS_FORMAT)),
                    Some(ColorTargetState::from(
                        super::deferred::GRASS_TEXTURE_FORMAT,
                    )),
                ],
                ..default()
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: PolygonMode::Fill,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: default(),
                bias: default(),
            }),
            ..default()
        };
        Self {
            schedule_layout,
            compute_layout,
            pose_layout,
            draw_layout: draw_layout.clone(),
            schedule,
            generate,
            finalize,
            cache_build,
            cache_finish,
            pose,
            draw_variants: Variants::new(
                VegetationPipelineSpecializer { draw_layout },
                draw_descriptor,
            ),
        }
    }
}

/// The blades' G-buffer pass (`deferred.rs`); under MetalFX Temporal it writes their motion too.
#[derive(Clone, Copy, PartialEq, Eq, Hash, SpecializerKey)]
pub(super) struct VegetationPipelineKey {
    pub(super) motion: bool,
}

pub(super) struct VegetationPipelineSpecializer {
    draw_layout: BindGroupLayoutDescriptor,
}

impl Specializer<RenderPipeline> for VegetationPipelineSpecializer {
    type Key = VegetationPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Self::Key>, BevyError> {
        descriptor.layout = vec![
            super::deferred::unused_view_layout(),
            self.draw_layout.clone(),
            atmosphere::environment::surface_layout(),
        ];

        if key.motion {
            descriptor.layout.push(temporal::layout());
            descriptor.vertex.shader_defs.push("TEMPORAL_GRASS".into());
            let fragment = descriptor.fragment.as_mut().unwrap();
            fragment.shader_defs.push("TEMPORAL_GRASS".into());
            fragment
                .targets
                .push(Some(ColorTargetState::from(MOTION_VECTOR_PREPASS_FORMAT)));
        }

        Ok(key)
    }
}
