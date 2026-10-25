//! Compute layouts and draw-pipeline specialization for view, lighting and G-buffer variants.
use super::{gpu_types::shader_defs, temporal};

use bevy::{
    core_pipeline::{
        core_3d::CORE_3D_DEPTH_FORMAT, deferred::DEFERRED_PREPASS_FORMAT,
        prepass::MOTION_VECTOR_PREPASS_FORMAT,
    },
    pbr::{MeshPipelineViewLayoutKey, MeshPipelineViewLayouts},
    prelude::*,
    render::render_resource::{
        BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId, Canonical,
        ColorTargetState, CompareFunction, ComputePipelineDescriptor, DepthStencilState,
        FragmentState, FrontFace, PipelineCache, PolygonMode, PrimitiveState, PrimitiveTopology,
        RenderPipeline, RenderPipelineDescriptor, SamplerBindingType, ShaderStages, Specializer,
        SpecializerKey, TextureFormat, TextureSampleType, Variants, VertexState,
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
        let view_layouts = world.resource::<MeshPipelineViewLayouts>().clone();
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
        draw_defs.push("SHADOW_FILTER_METHOD_HARDWARE_2X2".into());
        let draw_descriptor = RenderPipelineDescriptor {
            label: Some("vegetation draw".into()),
            // The exact mesh-view layout is selected per camera by the pipeline specializer.
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
                // Set per output by the specializer.
                targets: Vec::new(),
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
                VegetationPipelineSpecializer {
                    view_layouts,
                    draw_layout,
                },
                draw_descriptor,
            ),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, SpecializerKey)]
pub(super) struct VegetationPipelineKey {
    pub(super) output: DrawOutput,
    pub(super) environment: bool,
}

/// What a grass draw writes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum DrawOutput {
    /// Lit colour into a forward view's target, with that view's bind group layout.
    Colour {
        format: TextureFormat,
        view_layout_bits: u32,
    },
    /// The blades' surface into a deferred view's G-buffer (`deferred.rs`), and under MetalFX
    /// Temporal their motion.
    GBuffer { motion: bool },
}

pub(super) struct VegetationPipelineSpecializer {
    view_layouts: MeshPipelineViewLayouts,
    draw_layout: BindGroupLayoutDescriptor,
}

impl Specializer<RenderPipeline> for VegetationPipelineSpecializer {
    type Key = VegetationPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        descriptor: &mut RenderPipelineDescriptor,
    ) -> Result<Canonical<Self::Key>, BevyError> {
        let mut defs: Vec<ShaderDefVal> = Vec::new();
        let view_layout = match key.output {
            DrawOutput::Colour {
                view_layout_bits, ..
            } => {
                let view_key = MeshPipelineViewLayoutKey::from_bits_retain(view_layout_bits);

                if view_key.contains(MeshPipelineViewLayoutKey::ATMOSPHERE) {
                    defs.push("ATMOSPHERE".into());
                }

                self.view_layouts.get_view_layout(view_key).main_layout
            }
            DrawOutput::GBuffer { .. } => super::deferred::unused_view_layout(),
        };
        descriptor.layout = vec![view_layout, self.draw_layout.clone()];
        descriptor.layout.push(if key.environment {
            defs.extend([
                "ENVIRONMENT_SURFACE".into(),
                ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 2),
            ]);
            atmosphere::environment::surface_layout()
        } else {
            BindGroupLayoutDescriptor::new("unused environment", &[])
        });

        let fragment = descriptor.fragment.as_mut().unwrap();

        match key.output {
            DrawOutput::Colour { format, .. } => {
                fragment.targets = vec![Some(ColorTargetState::from(format))];
            }
            DrawOutput::GBuffer { motion } => {
                defs.push("GRASS_GBUFFER".into());
                fragment.targets = vec![
                    Some(ColorTargetState::from(DEFERRED_PREPASS_FORMAT)),
                    Some(ColorTargetState::from(
                        super::deferred::GRASS_TEXTURE_FORMAT,
                    )),
                ];

                if motion {
                    defs.push("TEMPORAL_GRASS".into());
                    descriptor.layout.push(temporal::layout());
                    fragment
                        .targets
                        .push(Some(ColorTargetState::from(MOTION_VECTOR_PREPASS_FORMAT)));
                }
            }
        }

        fragment.shader_defs.extend(defs.iter().cloned());
        descriptor.vertex.shader_defs.extend(defs);
        Ok(key)
    }
}
