use bevy::render::{render_resource::*, renderer::RenderDevice, texture::GpuImage};

pub(crate) struct Linear {
    pipeline: RenderPipeline,
    layout: BindGroupLayout,
    sampler: Sampler,
}
impl Linear {
    pub fn new(device: &RenderDevice) -> Self {
        let shader = device
            .wgpu_device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("linear upscaler"),
                source: wgpu::ShaderSource::Wgsl(include_str!("linear.wgsl").into()),
            });
        let layout = device.create_bind_group_layout(
            "linear upscaler",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    binding_types::texture_2d(TextureSampleType::Float { filterable: true }),
                    binding_types::sampler(SamplerBindingType::Filtering),
                ),
            ),
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("linear upscaler"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("linear upscaler"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }
    pub fn bind(&self, device: &RenderDevice, input: &GpuImage) -> BindGroup {
        device.create_bind_group(
            "linear upscaler",
            &self.layout,
            &BindGroupEntries::sequential((&input.texture_view, &self.sampler)),
        )
    }
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, bind: &BindGroup, output: &GpuImage) {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("linear upscale"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &output.texture_view,
                resolve_target: None,
                depth_slice: None,
                ops: Operations {
                    load: LoadOp::Clear(Default::default()),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}
