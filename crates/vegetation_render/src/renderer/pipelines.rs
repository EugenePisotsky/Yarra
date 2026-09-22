//! Compute layouts and draw-pipeline specialization for view, lighting and temporal variants.
use super::temporal;
use crate::VegetationBladeBands;
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

const COMPUTE_SHADER_PATH: &str = "shaders/vegetation_debug_compute.wgsl";
const SCHEDULE_SHADER_PATH: &str = "shaders/vegetation_schedule_compute.wgsl";
const DRAW_SHADER_PATH: &str = "shaders/vegetation_debug_draw.wgsl";

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
        let compute_shader = asset_server.load(COMPUTE_SHADER_PATH);
        let schedule_shader = asset_server.load(SCHEDULE_SHADER_PATH);
        let draw_shader = asset_server.load(DRAW_SHADER_PATH);
        let pipeline_cache = world.resource::<PipelineCache>();
        let view_layouts = world.resource::<MeshPipelineViewLayouts>().clone();
        let schedule_layout = BindGroupLayoutDescriptor::new(
            "vegetation-v2 visible work scheduling",
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
            "vegetation-v2 debug generation",
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
            "vegetation-v2 debug draw",
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
            label: Some("vegetation-v2 visible work scheduling".into()),
            layout: vec![schedule_layout.clone()],
            shader: schedule_shader,
            entry_point: Some(Cow::Borrowed("schedule")),
            ..default()
        });
        let generate = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("vegetation-v2 classify once and emit".into()),
            layout: vec![compute_layout.clone()],
            shader: compute_shader.clone(),
            entry_point: Some(Cow::Borrowed("generate")),
            ..default()
        });
        let finalize = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("vegetation-v2 debug finalize".into()),
            layout: vec![compute_layout.clone()],
            shader: compute_shader.clone(),
            entry_point: Some(Cow::Borrowed("finalize")),
            ..default()
        });
        let cache_pipeline = |entry: &'static str| {
            pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some(format!("vegetation candidate {entry}").into()),
                layout: vec![compute_layout.clone()],
                shader: compute_shader.clone(),
                entry_point: Some(entry.into()),
                ..default()
            })
        };
        let cache_build = cache_pipeline("build_candidate_cache");
        let cache_finish = cache_pipeline("finish_candidate_cache");
        let draw_descriptor = RenderPipelineDescriptor {
            label: Some("vegetation-v2 placement debug".into()),
            // The exact mesh-view layout is selected per camera by the pipeline specializer.
            layout: Vec::new(),
            vertex: VertexState {
                shader: draw_shader.clone(),
                entry_point: Some(Cow::Borrowed("vertex")),
                shader_defs: vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()],
                buffers: Vec::new(),
                ..default()
            },
            fragment: Some(FragmentState {
                shader: draw_shader,
                entry_point: Some(Cow::Borrowed("fragment")),
                shader_defs: vec!["SHADOW_FILTER_METHOD_HARDWARE_2X2".into()],
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
    pub(super) blade_bands: VegetationBladeBands,
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
        if key.blade_bands != VegetationBladeBands::Off {
            let mut defs = vec![
                ShaderDefVal::Bool("BLADE_BAND_STUDY".into(), true),
                ShaderDefVal::UInt(
                    "BLADE_BAND_STRENGTH".into(),
                    if key.blade_bands == VegetationBladeBands::Subtle {
                        54
                    } else {
                        82
                    },
                ),
            ];
            if matches!(
                key.blade_bands,
                VegetationBladeBands::Mask | VegetationBladeBands::MotionMask
            ) {
                defs.push(ShaderDefVal::Bool("BLADE_BAND_MASK".into(), true));
            }
            descriptor.vertex.shader_defs.extend(defs.clone());
            descriptor
                .fragment
                .as_mut()
                .unwrap()
                .shader_defs
                .extend(defs);
        }
        descriptor.fragment.as_mut().unwrap().targets[0]
            .as_mut()
            .unwrap()
            .format = key.target_format;
        Ok(key)
    }
}
