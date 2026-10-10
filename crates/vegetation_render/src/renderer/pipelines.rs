//! Compute layouts and draw-pipeline specialization for view, lighting and temporal variants.
use super::{gpu_types::shader_defs, temporal};
use bevy::{
    core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT,
    pbr::{MeshPipelineViewLayoutKey, MeshPipelineViewLayouts},
    prelude::*,
    render::render_resource::{
        BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId, Canonical,
        ColorTargetState, ColorWrites, CompareFunction, ComputePipelineDescriptor,
        DepthStencilState, FragmentState, FrontFace, PipelineCache, PolygonMode, PrimitiveState,
        PrimitiveTopology, RenderPipeline, RenderPipelineDescriptor, ShaderStages, Specializer,
        SpecializerKey, TextureFormat, Variants, VertexState,
        binding_types::{
            storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
        },
    },
    shader::ShaderDefVal,
};
use std::borrow::Cow;

const SCHEDULE_SHADER_PATH: &str = "shaders/vegetation/schedule.wesl";
const PLACEMENT_SHADER_PATH: &str = "shaders/vegetation/placement.wesl";
const CANDIDATE_CACHE_SHADER_PATH: &str = "shaders/vegetation/candidate_cache.wesl";
const DRAW_SHADER_PATH: &str = "shaders/vegetation/draw.wesl";

#[derive(Resource)]
pub(super) struct VegetationPipelines {
    pub(super) schedule_layout: BindGroupLayoutDescriptor,
    pub(super) compute_layout: BindGroupLayoutDescriptor,
    pub(super) draw_layout: BindGroupLayoutDescriptor,
    pub(super) schedule: CachedComputePipelineId,
    pub(super) generate: CachedComputePipelineId,
    pub(super) finalize: CachedComputePipelineId,
    pub(super) cache_build: CachedComputePipelineId,
    pub(super) cache_finish: CachedComputePipelineId,
    pub(super) draw_variants: Variants<RenderPipeline, VegetationPipelineSpecializer>,
}

impl FromWorld for VegetationPipelines {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        let schedule_shader = asset_server.load(SCHEDULE_SHADER_PATH);
        let placement_shader = asset_server.load(PLACEMENT_SHADER_PATH);
        let candidate_cache_shader = asset_server.load(CANDIDATE_CACHE_SHADER_PATH);
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
                    storage_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
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
                    storage_buffer_read_only_sized(false, None),
                    uniform_buffer_sized(false, None),
                    uniform_buffer_sized(false, None),
                    storage_buffer_read_only_sized(false, None),
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
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
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
            draw_layout: draw_layout.clone(),
            schedule,
            generate,
            finalize,
            cache_build,
            cache_finish,
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
    pub(super) msaa: Msaa,
    pub(super) target_format: TextureFormat,
    pub(super) view_layout_bits: u32,
    pub(super) clouds: bool,
    pub(super) temporal: bool,
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
        let view_layout =
            self.view_layouts
                .get_view_layout(MeshPipelineViewLayoutKey::from_bits_retain(
                    key.view_layout_bits,
                ));
        descriptor.layout = vec![view_layout.main_layout, self.draw_layout.clone()];
        if key.clouds {
            descriptor.layout.push(atmosphere::clouds::surface_layout());
            descriptor.vertex.shader_defs.extend([
                "YARRA_CLOUDS".into(),
                ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 2),
            ]);
            descriptor.fragment.as_mut().unwrap().shader_defs.extend([
                "YARRA_CLOUDS".into(),
                ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 2),
            ]);
        }
        if key.temporal {
            if !key.clouds {
                descriptor
                    .layout
                    .push(BindGroupLayoutDescriptor::new("unused clouds", &[]));
            }
            descriptor.layout.push(temporal::layout());
            descriptor.vertex.shader_defs.push("TEMPORAL_GRASS".into());
            descriptor
                .fragment
                .as_mut()
                .unwrap()
                .shader_defs
                .push("TEMPORAL_GRASS".into());
            descriptor
                .fragment
                .as_mut()
                .unwrap()
                .targets
                .push(Some(ColorTargetState {
                    format: TextureFormat::Rg16Float,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                }));
        }
        descriptor.multisample.count = key.msaa.samples();
        if MeshPipelineViewLayoutKey::from_bits_retain(key.view_layout_bits)
            .contains(MeshPipelineViewLayoutKey::ATMOSPHERE)
        {
            descriptor.vertex.shader_defs.push("ATMOSPHERE".into());
            descriptor
                .fragment
                .as_mut()
                .unwrap()
                .shader_defs
                .push("ATMOSPHERE".into());
        }
        descriptor.fragment.as_mut().unwrap().targets[0]
            .as_mut()
            .unwrap()
            .format = key.target_format;
        Ok(key)
    }
}
