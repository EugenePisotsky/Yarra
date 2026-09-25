use super::*;
use bevy::{
    core_pipeline::{Core3dSystems, FullscreenShader, schedule::Core3d},
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        render_asset::RenderAssets,
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        storage::GpuShaderBuffer,
        texture::GpuImage,
        view::{
            ExtractedView, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset,
            ViewUniforms,
        },
    },
};
use binding_types::*;

#[derive(Resource)]
pub struct CloudShadowLayout(pub BindGroupLayoutDescriptor);
#[derive(Resource)]
pub struct CloudShadowGpu(pub BindGroup);
#[derive(Resource)]
struct Pipelines {
    compute_layout: BindGroupLayoutDescriptor,
    render_layout: BindGroupLayoutDescriptor,
    composite_layout: [BindGroupLayoutDescriptor; 2],
    compute: CachedComputePipelineId,
    render: [CachedRenderPipelineId; 2],
    composite: [CachedRenderPipelineId; 4],
    sampler: Sampler,
    clamp_sampler: Sampler,
    /// Cached sky: wraps across the azimuth seam, clamps at the horizon and zenith rows.
    panorama_sampler: Sampler,
}
pub(super) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .add_systems(RenderStartup, init)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(Core3d, update_shadows.before(Core3dSystems::MainPass))
        .add_systems(
            Core3d,
            draw.after(Core3dSystems::MainPass)
                .before(Core3dSystems::EarlyPostProcess),
        );
}
fn init(
    mut commands: Commands,
    device: Res<RenderDevice>,
    server: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    cache: Res<PipelineCache>,
) {
    let params = || {
        storage_buffer_read_only_sized(
            false,
            Some(std::num::NonZeroU64::new(std::mem::size_of::<CloudParams>() as u64).unwrap()),
        )
    };
    let shadow_layout = surface_layout();
    let compute_layout = BindGroupLayoutDescriptor::new(
        "cloud shadow compute",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                params(),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let render_layout = BindGroupLayoutDescriptor::new(
        "cloud volume",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                params(),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<ViewUniform>(true),
                texture_2d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    );
    let composite_layout = std::array::from_fn(|i| {
        BindGroupLayoutDescriptor::new(
            "cloud composite",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    if i == 0 {
                        texture_depth_2d()
                    } else {
                        texture_depth_2d_multisampled()
                    },
                    uniform_buffer::<ViewUniform>(true),
                    params(),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    uniform_buffer_sized(false, std::num::NonZeroU64::new(16)),
                ),
            ),
        )
    });
    let compute = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("cloud sun/moon transmittance".into()),
        layout: vec![compute_layout.clone()],
        shader: server.load("shaders/clouds/shadows.wgsl"),
        ..default()
    });
    let render = std::array::from_fn(|i| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("cloud volume".into()),
            layout: vec![render_layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: server.load("shaders/clouds/render.wgsl"),
                shader_defs: if i == 1 {
                    vec!["CACHED_SKY".into()]
                } else {
                    vec![]
                },
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        })
    });
    let composite = std::array::from_fn(|i| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("cloud HDR composite".into()),
            layout: vec![composite_layout[i % 2].clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: server.load("shaders/clouds/composite.wgsl"),
                shader_defs: {
                    let mut defs = Vec::new();
                    if i % 2 == 1 {
                        defs.push("MULTISAMPLED".into());
                    }
                    if i >= 2 {
                        defs.push("CACHED_SKY".into());
                    }
                    defs
                },
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::One,
                            dst_factor: BlendFactor::SrcAlpha,
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::Zero,
                            dst_factor: BlendFactor::One,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        })
    });
    commands.insert_resource(CloudShadowLayout(shadow_layout));
    commands.insert_resource(Pipelines {
        compute_layout,
        render_layout,
        composite_layout,
        compute,
        render,
        composite,
        clamp_sampler: device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        panorama_sampler: device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        sampler: device.create_sampler(&SamplerDescriptor {
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            address_mode_w: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
    });
}
fn prepare(
    mut commands: Commands,
    assets: Option<Res<CloudAssets>>,
    params: Res<CloudParams>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    layout: Res<CloudShadowLayout>,
    mut previous: Local<Option<(BufferId, TextureViewId)>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let (Some(buffer), Some(image)) =
        (buffers.get(&assets.parameters), images.get(&assets.shadows))
    else {
        return;
    };
    queue.write_buffer(&buffer.buffer, 0, bytemuck::bytes_of(&*params));
    let key = (buffer.buffer.id(), image.texture_view.id());
    if *previous != Some(key) {
        commands.insert_resource(CloudShadowGpu(device.create_bind_group(
            "cloud surface bind group",
            &cache.get_bind_group_layout(&layout.0),
            &BindGroupEntries::with_indices((
                (120, buffer.buffer.as_entire_binding()),
                (121, &image.texture_view),
                (122, &image.sampler),
            )),
        )));
        *previous = Some(key);
    }
}
fn update_shadows(
    view: ViewQuery<&CloudView>,
    assets: Option<Res<CloudAssets>>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    params: Res<CloudParams>,
    mut previous: Local<
        Option<(
            ShadowInputs,
            TextureViewId,
            TextureViewId,
            ComputePipelineId,
        )>,
    >,
    mut ctx: RenderContext,
) {
    let _ = view.into_inner();
    let Some(assets) = assets else {
        return;
    };
    let (Some(buffer), Some(noise), Some(shadows), Some(pipeline)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.noise),
        images.get(&assets.shadows),
        cache.get_compute_pipeline(pipelines.compute),
    ) else {
        return;
    };
    let key = (
        ShadowInputs::from(&*params),
        noise.texture_view.id(),
        shadows.texture_view.id(),
        pipeline.id(),
    );
    if previous.as_ref() == Some(&key) {
        return;
    }
    let group = ctx.render_device().create_bind_group(
        "cloud shadow generation",
        &cache.get_bind_group_layout(&pipelines.compute_layout),
        &BindGroupEntries::sequential((
            buffer.buffer.as_entire_binding(),
            &noise.texture_view,
            &pipelines.sampler,
            &shadows.texture_view,
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("cloud shadow generation"),
            timestamp_writes: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.dispatch_workgroups(32, 32, 1);
    drop(pass);
    *previous = Some(key);
}

/// Shadows are stored in the moving cloud field's coordinates. Wind and origin
/// rebases change lookup coordinates, not this texture. Light color/exposure
/// also cannot change optical depth. Actual light directions and density do.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ShadowInputs {
    layer: [f32; 4],
    shape: [f32; 4],
    transition: [f32; 4],
    lights: [[f32; 4]; 2],
}
impl From<&CloudParams> for ShadowInputs {
    fn from(p: &CloudParams) -> Self {
        Self {
            layer: p.layer,
            shape: p.shape,
            transition: p.transition,
            lights: [p.sun, p.moon].map(|mut light| {
                if light[3] > 0. && light[1] > 0. {
                    light[3] = 1.;
                    light
                } else {
                    [0.; 4]
                }
            }),
        }
    }
}

/// Inputs that make the cached sky a different cloud field. Coverage, density, erosion and
/// thickness change continuously with weather; progressive refresh follows those without a
/// full retrace every frame. Altitude, field size, seed and enablement require a full refresh.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SkyCacheKey {
    base: f32,
    period: f32,
    enabled: f32,
    seed: f32,
}
impl From<&CloudParams> for SkyCacheKey {
    fn from(p: &CloudParams) -> Self {
        Self {
            base: p.layer[0],
            period: p.layer[2],
            enabled: p.layer[3],
            seed: p.shape[3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finished_sweeps_rotate_without_ever_refreshing_a_displayed_image() {
        let mut target = CloudTarget {
            older: 0,
            newer: 1,
            building: 2,
            ..default()
        };
        for _ in 0..6 {
            let (older, newer, building) = (target.older, target.newer, target.building);
            target.rotate();
            // The finished image is shown next; the image being replaced was not on screen.
            assert_eq!(
                (target.older, target.newer, target.building),
                (newer, building, older)
            );
            assert_ne!(target.building, target.older);
            assert_ne!(target.building, target.newer);
        }
    }
    #[test]
    fn weather_evolution_refreshes_the_sky_progressively_but_field_changes_do_not() {
        let mut p = CloudParams {
            layer: [1200., 650., 7200., 1.],
            shape: [0.48, 0.02, 0.3, 7.],
            ..default()
        };
        let key = SkyCacheKey::from(&p);
        p.layer[1] = 450.;
        p.shape[0] = 0.92;
        p.shape[1] = 0.03;
        p.shape[2] = 0.1;
        p.offset = [1000., 2000., 500., 750.];
        assert_eq!(SkyCacheKey::from(&p), key);
        for change in [
            |p: &mut CloudParams| p.layer[0] = 1500.,
            |p: &mut CloudParams| p.layer[2] = 8000.,
            |p: &mut CloudParams| p.layer[3] = 0.,
            |p: &mut CloudParams| p.shape[3] = 3.,
        ] {
            let mut changed = p;
            change(&mut changed);
            assert_ne!(SkyCacheKey::from(&changed), key);
        }
    }
    #[test]
    fn wind_origin_and_exposure_reuse_shadows_but_density_and_light_changes_do_not() {
        let mut p = CloudParams::default();
        p.layer = [1200., 650., 7200., 1.];
        p.shape = [0.48, 0.02, 0.3, 7.];
        p.sun = [0.5, 0.7, 0.5, 100000.];
        let key = ShadowInputs::from(&p);
        p.offset = [1000., 2000., 500., 750.];
        p.sun[3] *= 0.5;
        p.sun_color = [1., 0.5, 0.2, 0.];
        p.ambient = [0.1, 0.2, 0.5, 700.];
        assert_eq!(ShadowInputs::from(&p), key);
        p.shape[0] += 0.1;
        assert_ne!(ShadowInputs::from(&p), key);
        p.shape = key.shape;
        p.sun[1] += 0.01;
        assert_ne!(ShadowInputs::from(&p), key);
        p.sun = [0.5, 0.7, 0.5, 100000.];
        p.moon = [0., 1., 0., 100.];
        assert_ne!(ShadowInputs::from(&p), key);
    }
}
/// Balanced keeps three cache images: the previous and newest complete refreshes, which the
/// composite cross-fades by sweep progress, and one being refreshed row by row. Displaying only
/// complete images turns 4 Hz texel updates into continuous change: no stepping, and no seam
/// where the refresh cursor passes. High keeps a single per-frame image.
#[derive(Default)]
struct CloudTarget {
    size: UVec2,
    slots: Vec<(Texture, TextureView)>,
    /// Indices into `slots`: previous complete, newest complete, being refreshed.
    older: usize,
    newer: usize,
    building: usize,
    sky_refresh: sky_cache::Refresh,
    sky_key: Option<(SkyCacheKey, TextureViewId, RenderPipelineId)>,
    epoch: Option<std::time::Instant>,
    last_report: std::time::Duration,
    traced_pixels: u64,
    composites: u64,
}
impl CloudTarget {
    /// A finished sweep becomes the newest image; the oldest slot is refreshed next.
    fn rotate(&mut self) {
        (self.older, self.newer, self.building) = (self.newer, self.building, self.older);
    }
}
#[allow(clippy::too_many_arguments)] // One scissored pass: target, rows and pipeline state.
fn trace(
    encoder: &mut CommandEncoder,
    view: &TextureView,
    rows: (u32, u32),
    width: u32,
    clear: bool,
    pipeline: &RenderPipeline,
    group: &BindGroup,
    view_offset: u32,
) {
    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
        label: Some("cloud volume"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: if clear {
                    LoadOp::Clear(Color::WHITE.to_linear().into())
                } else {
                    LoadOp::Load
                },
                store: StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[view_offset]);
    pass.set_scissor_rect(0, rows.0, width, rows.1);
    pass.draw(0..3, 0..1);
}
#[allow(clippy::too_many_arguments)]
fn draw(
    view: ViewQuery<(
        &CloudView,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthTexture,
        &ViewUniformOffset,
        &Msaa,
        Option<&bevy::camera::MainPassResolutionOverride>,
    )>,
    assets: Option<Res<CloudAssets>>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    uniforms: Res<ViewUniforms>,
    mut target: Local<CloudTarget>,
    quality: Res<CloudQuality>,
    params: Res<CloudParams>,
    mut ctx: RenderContext,
) {
    let (_, extracted, view, depth, offset, msaa, resolution) = view.into_inner();
    if view.main_texture_format() != TextureFormat::Rgba16Float {
        return;
    }
    let Some(assets) = assets else {
        return;
    };
    let index = usize::from(msaa.samples() > 1);
    let cached = *quality == CloudQuality::Balanced;
    let (Some(buffer), Some(noise), Some(render), Some(composite), Some(view_binding)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.noise),
        cache.get_render_pipeline(pipelines.render[usize::from(cached)]),
        cache.get_render_pipeline(pipelines.composite[index + usize::from(cached) * 2]),
        uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let main_size = resolution.map_or(
        UVec2::new(view.main_texture().width(), view.main_texture().height()),
        |r| r.0,
    );
    let size = quality.target_size(main_size);
    let slot_count = if cached { 3 } else { 1 };
    let resized = target.size != size || target.slots.len() != slot_count;
    if resized {
        target.slots = (0..slot_count)
            .map(|_| {
                let texture = ctx.render_device().create_texture(&TextureDescriptor {
                    label: Some("cloud volume target"),
                    size: Extent3d {
                        width: size.x,
                        height: size.y,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba16Float,
                    usage: TextureUsages::TEXTURE_BINDING
                        | TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::COPY_SRC
                        | TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&default());
                (texture, view)
            })
            .collect();
        (target.older, target.newer, target.building) = if cached { (0, 1, 2) } else { (0, 0, 0) };
        target.size = size;
    }
    let now = target
        .epoch
        .get_or_insert_with(std::time::Instant::now)
        .elapsed();
    let key = (
        SkyCacheKey::from(&*params),
        noise.texture_view.id(),
        render.id(),
    );
    let position =
        extracted.world_from_view.translation() + Vec3::new(params.offset[0], 0., params.offset[1]);
    let region = if cached {
        let invalid = resized || target.sky_key.as_ref() != Some(&key);
        target
            .sky_refresh
            .next(now, position, params.layer[2] * 4., invalid)
    } else {
        Some(sky_cache::Region::Full)
    };
    if let Some(region) = region {
        let group = ctx.render_device().create_bind_group(
            "cloud volume",
            &cache.get_bind_group_layout(&pipelines.render_layout),
            &BindGroupEntries::sequential((
                buffer.buffer.as_entire_binding(),
                &noise.texture_view,
                &pipelines.sampler,
                view_binding.clone(),
                view.main_texture_view(),
            )),
        );
        let full = region == sky_cache::Region::Full;
        for rows in region.ranges(size.y) {
            let building = target.slots[target.building].1.clone();
            trace(
                ctx.command_encoder(),
                &building,
                rows,
                size.x,
                full,
                render,
                &group,
                offset.offset,
            );
            target.traced_pixels += u64::from(size.x) * u64::from(rows.1);
            if cached && !full && rows.0 + rows.1 == size.y {
                target.rotate();
            }
        }
        if cached && full {
            // Restart: both displayed images show the new field until the next sweep.
            let source = target.slots[target.building].0.clone();
            for destination in [target.older, target.newer] {
                ctx.command_encoder().copy_texture_to_texture(
                    source.as_image_copy(),
                    target.slots[destination].0.as_image_copy(),
                    source.size(),
                );
            }
        }
        target.sky_key = Some(key);
    }
    target.composites += 1;
    if now.saturating_sub(target.last_report).as_secs_f32() >= 5. {
        debug!(
            "CLOUD_WORK quality={:?} seconds={:.3} composites={} traced_pixels={}",
            *quality,
            (now - target.last_report).as_secs_f32(),
            target.composites,
            target.traced_pixels
        );
        target.last_report = now;
        target.composites = 0;
        target.traced_pixels = 0;
    }
    let blend = if cached {
        target.sky_refresh.progress()
    } else {
        1.0
    };
    let blend = ctx
        .render_device()
        .create_buffer_with_data(&BufferInitDescriptor {
            label: Some("cloud cache blend"),
            contents: bytemuck::bytes_of(&[blend, 0.0, 0.0, 0.0]),
            usage: BufferUsages::UNIFORM,
        });
    let group = ctx.render_device().create_bind_group(
        "cloud composite",
        &cache.get_bind_group_layout(&pipelines.composite_layout[index]),
        &BindGroupEntries::sequential((
            &target.slots[target.newer].1,
            if cached {
                &pipelines.panorama_sampler
            } else {
                &pipelines.clamp_sampler
            },
            depth.view(),
            view_binding,
            buffer.buffer.as_entire_binding(),
            &target.slots[target.older].1,
            blend.as_entire_binding(),
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("cloud composite"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: view.main_texture_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_viewport(0., 0., main_size.x as f32, main_size.y as f32, 0., 1.);
    pass.set_pipeline(composite);
    pass.set_bind_group(0, &group, &[offset.offset]);
    pass.draw(0..3, 0..1);
    drop(pass);
}

pub fn surface_layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "cloud shared surface input",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (
                    120,
                    storage_buffer_read_only_sized(
                        false,
                        Some(
                            std::num::NonZeroU64::new(std::mem::size_of::<CloudParams>() as u64)
                                .unwrap(),
                        ),
                    ),
                ),
                (
                    121,
                    texture_2d(TextureSampleType::Float { filterable: true }),
                ),
                (122, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    )
}
