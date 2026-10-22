use super::*;

use bevy::{
    core_pipeline::{Core3dSystems, FullscreenShader, schedule::Core3d},
    render::{
        RenderApp, RenderStartup,
        render_asset::RenderAssets,
        renderer::{RenderContext, RenderDevice, ViewQuery},
        storage::GpuShaderBuffer,
        texture::GpuImage,
        view::{ExtractedView, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};

use binding_types::*;

/// Below this transmittance of the weather's fog up to the cloud base the clouds show nowhere:
/// the cloud tracer's own opacity cut (`shaders/clouds/render.wesl`).
const HIDDEN_BEHIND_FOG: f32 = 0.003;

/// Whether the weather's fog, `extinction` per metre, hides a cloud base `above` metres
/// overhead: the shortest way to it, so then every cloud.
fn hidden_behind_fog(extinction: f32, above: f32) -> bool {
    (-extinction * above.max(0.)).exp() < HIDDEN_BEHIND_FOG
}

#[derive(Resource)]
pub(crate) struct Pipelines {
    compute_layout: BindGroupLayoutDescriptor,
    render_layout: BindGroupLayoutDescriptor,
    noise_layout: BindGroupLayoutDescriptor,
    compute: CachedComputePipelineId,
    render: [CachedRenderPipelineId; 2],
    /// Filling the base shape and the detail noise volumes.
    noise: [CachedComputePipelineId; 2],
    sampler: Sampler,
    clamp_sampler: Sampler,
    /// Cached sky: wraps across the azimuth seam, clamps at the horizon and zenith rows.
    panorama_sampler: Sampler,
}

impl Pipelines {
    /// Sampler for displaying the cloud image: the panorama cache or a per-frame screen image.
    pub(crate) fn display_sampler(&self, cached: bool) -> &Sampler {
        if cached {
            &self.panorama_sampler
        } else {
            &self.clamp_sampler
        }
    }
}

pub(super) fn install(app: &mut App) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_resource::<CloudTarget>()
        .add_systems(RenderStartup, init)
        .add_systems(Core3d, update_shadows.before(Core3dSystems::MainPass))
        .add_systems(
            Core3d,
            refresh
                .after(Core3dSystems::MainPass)
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
            Some(
                std::num::NonZeroU64::new(std::mem::size_of::<EnvironmentParams>() as u64).unwrap(),
            ),
        )
    };
    let compute_layout = BindGroupLayoutDescriptor::new(
        "cloud shadow compute",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                params(),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_storage_2d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly),
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
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
                texture_3d(TextureSampleType::Float { filterable: true }),
                texture_3d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    );
    let noise_layout = BindGroupLayoutDescriptor::new(
        "cloud noise",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_storage_3d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly),
                texture_storage_3d(TextureFormat::Rgba8Unorm, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let noise = ["generate_shape", "generate_detail"].map(|entry| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("cloud noise".into()),
            layout: vec![noise_layout.clone()],
            shader: server.load("shaders/clouds/noise.wesl"),
            entry_point: Some(entry.into()),
            ..default()
        })
    });
    let compute = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("cloud sun/moon transmittance".into()),
        layout: vec![compute_layout.clone()],
        shader: server.load("shaders/clouds/shadows.wesl"),
        ..default()
    });
    let render = std::array::from_fn(|i| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("cloud volume".into()),
            layout: vec![render_layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: server.load("shaders/clouds/render.wesl"),
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
    commands.insert_resource(Pipelines {
        compute_layout,
        render_layout,
        noise_layout,
        compute,
        render,
        noise,
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

#[allow(clippy::too_many_arguments)] // Independent render-world inputs of one pass.
fn update_shadows(
    view: ViewQuery<&CloudView>,
    assets: Option<Res<EnvironmentAssets>>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    params: Res<EnvironmentParams>,
    mut previous: Local<Option<(ShadowInputs, [TextureViewId; 4], ComputePipelineId)>>,
    mut ctx: RenderContext,
) {
    let _ = view.into_inner();
    let Some(assets) = assets else {
        return;
    };
    let (Some(buffer), Some(noise), Some(shadows), Some(shape), Some(detail), Some(pipeline)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.noise),
        images.get(&assets.shadows),
        images.get(&assets.cloud_shape),
        images.get(&assets.cloud_detail),
        cache.get_compute_pipeline(pipelines.compute),
    ) else {
        return;
    };
    let views = [noise, shadows, shape, detail].map(|image| image.texture_view.id());

    // The noise volumes are filled before anything reads them, and again when made anew; the
    // shadows follow them.
    if previous.as_ref().is_none_or(|p| p.1[2..] != views[2..])
        && !generate_noise(&mut ctx, &pipelines, &cache, shape, detail)
    {
        return;
    }

    let key = (ShadowInputs::from(&*params), views, pipeline.id());

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
            &shape.texture_view,
            &detail.texture_view,
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

/// Fills the base shape and detail noise volumes; false until their pipelines are ready.
fn generate_noise(
    ctx: &mut RenderContext,
    pipelines: &Pipelines,
    cache: &PipelineCache,
    shape: &GpuImage,
    detail: &GpuImage,
) -> bool {
    let (Some(shape_pipeline), Some(detail_pipeline)) = (
        cache.get_compute_pipeline(pipelines.noise[0]),
        cache.get_compute_pipeline(pipelines.noise[1]),
    ) else {
        return false;
    };
    let group = ctx.render_device().create_bind_group(
        "cloud noise",
        &cache.get_bind_group_layout(&pipelines.noise_layout),
        &BindGroupEntries::sequential((&shape.texture_view, &detail.texture_view)),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("cloud noise"),
            timestamp_writes: None,
        });
    pass.set_bind_group(0, &group, &[]);

    for (pipeline, size) in [
        (shape_pipeline, super::SHAPE_SIZE),
        (detail_pipeline, super::DETAIL_SIZE),
    ] {
        let groups = size.div_ceil(4);
        pass.set_pipeline(pipeline);
        pass.dispatch_workgroups(groups, groups, groups);
    }

    true
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

impl From<&EnvironmentParams> for ShadowInputs {
    fn from(p: &EnvironmentParams) -> Self {
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

impl From<&EnvironmentParams> for SkyCacheKey {
    fn from(p: &EnvironmentParams) -> Self {
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
    fn a_blizzards_fog_hides_the_clouds_and_light_snows_does_not() {
        let fog = |visibility: f32| world::weather::VISIBILITY_EXTINCTION / visibility;
        assert!(hidden_behind_fog(fog(120.), 800.));
        assert!(!hidden_behind_fog(fog(1500.), 800.));
        assert!(
            !hidden_behind_fog(fog(120.), -50.),
            "nothing hides clouds around the camera"
        );
    }

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
    fn traced_clouds_move_with_the_wind_and_the_camera_across_the_field_seam() {
        let period = 28800.;
        let at = |camera: Vec3, wind: f32| {
            field_eye(
                camera,
                &EnvironmentParams {
                    offset: [0., 0., wind, 0.],
                    ..default()
                },
            )
        };
        let traced = at(Vec3::new(10., 300., 20.), period - 6.);
        let mut target = CloudTarget {
            older: 0,
            newer: 1,
            building: 2,
            eyes: vec![[traced; 2]; 3],
            eye: at(Vec3::new(10., 300., 23.), 6.),
            period,
            ..default()
        };
        target.eyes[0] = [at(Vec3::new(10., 300., 20.), 6.); 2];

        // The wind carried the clouds 12 m along X past the seam while the camera stepped 3 m
        // along Z: relative to the clouds the camera moved 12 m back along X.
        let shifts = target.shifts();
        assert!(shifts[0].abs_diff_eq(Vec4::new(-12., 0., 3., 0.), 1e-3));
        assert_eq!(shifts[0], shifts[1]);
        assert!(shifts[2].abs_diff_eq(Vec4::new(0., 0., 3., 0.), 1e-3));
    }

    #[test]
    fn weather_evolution_refreshes_the_sky_progressively_but_field_changes_do_not() {
        let mut p = EnvironmentParams {
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
            |p: &mut EnvironmentParams| p.layer[0] = 1500.,
            |p: &mut EnvironmentParams| p.layer[2] = 8000.,
            |p: &mut EnvironmentParams| p.layer[3] = 0.,
            |p: &mut EnvironmentParams| p.shape[3] = 3.,
        ] {
            let mut changed = p;
            change(&mut changed);
            assert_ne!(SkyCacheKey::from(&changed), key);
        }
    }

    #[test]
    fn wind_origin_and_exposure_reuse_shadows_but_density_and_light_changes_do_not() {
        let mut p = EnvironmentParams {
            layer: [1200., 650., 7200., 1.],
            shape: [0.48, 0.02, 0.3, 7.],
            sun: [0.5, 0.7, 0.5, 100000.],
            ..default()
        };
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
/// sky composite cross-fades by sweep progress, and one being refreshed row by row. Displaying
/// only complete images turns once-a-second texel updates into continuous change: no stepping,
/// and no seam where the refresh cursor passes. Between refreshes the composite carries each
/// image's clouds along with the wind and the camera's own moves. High keeps a single per-frame
/// image.
#[derive(Resource, Default)]
pub(crate) struct CloudTarget {
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
    /// Cross-fade from the previous to the newest complete image.
    blend: f32,
    /// Where the camera stood in the drifting cloud field (`field_eye`) as each slot's first and
    /// last rows were traced, and now; and the field's wrapping period.
    eyes: Vec<[Vec3; 2]>,
    eye: Vec3,
    period: f32,
}

impl CloudTarget {
    /// Newest and previous complete images and the cross-fade between them, once traced.
    pub(crate) fn display(&self) -> Option<(&TextureView, &TextureView, f32)> {
        let newer = self.slots.get(self.newer)?;
        let older = self.slots.get(self.older)?;
        Some((&newer.1, &older.1, self.blend))
    }

    /// How far the camera has moved through the drifting field since the newest image traced its
    /// first and its last row, then the previous image: the composite re-aims its lookups by
    /// them, so the clouds keep moving between refreshes.
    pub(crate) fn shifts(&self) -> [Vec4; 4] {
        let shift = |slot: usize, row: usize| {
            self.eyes.get(slot).map_or(Vec4::ZERO, |eyes| {
                sky_cache::field_delta(self.eye, eyes[row], self.period).extend(0.)
            })
        };

        [
            shift(self.newer, 0),
            shift(self.newer, 1),
            shift(self.older, 0),
            shift(self.older, 1),
        ]
    }

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

/// The camera's position in the cloud field, which the wind carries along: a traced cloud stays
/// where the image saw it relative to this.
fn field_eye(position: Vec3, params: &EnvironmentParams) -> Vec3 {
    position - Vec3::new(params.offset[2], 0., params.offset[3])
}

/// Trace this frame's share of the cloud images. The sky composite displays them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn refresh(
    view: ViewQuery<(
        &CloudView,
        &ExtractedView,
        &ViewTarget,
        &ViewUniformOffset,
        Option<&bevy::camera::MainPassResolutionOverride>,
    )>,
    assets: Option<Res<EnvironmentAssets>>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    uniforms: Res<ViewUniforms>,
    mut target: ResMut<CloudTarget>,
    quality: Res<CloudQuality>,
    params: Res<EnvironmentParams>,
    mut ctx: RenderContext,
) {
    let (_, extracted, view, offset, resolution) = view.into_inner();

    if view.main_texture_format() != TextureFormat::Rgba16Float {
        return;
    }

    let Some(assets) = assets else {
        return;
    };
    let cached = *quality == CloudQuality::Balanced;
    let (Some(buffer), Some(noise), Some(shape), Some(detail), Some(render), Some(view_binding)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.noise),
        images.get(&assets.cloud_shape),
        images.get(&assets.cloud_detail),
        cache.get_render_pipeline(pipelines.render[usize::from(cached)]),
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
        target.eyes = vec![[Vec3::ZERO; 2]; slot_count];
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
    target.eye = field_eye(position, &params);
    target.period = params.layer[2] * 4.;

    // Clouds the weather's fog hides even straight up show nowhere: the sweep keeps its schedule
    // and key without tracing, and resumes long before the fog thins enough to show them. New
    // images are traced whole, hidden or not.
    if !resized && hidden_behind_fog(params.fog[3], params.layer[0] - position.y) {
        if cached {
            target
                .sky_refresh
                .next(now, position, params.layer[2] * 4., false);
            target.sky_key = Some(key);
        }

        target.composites += 1;
        return;
    }

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
                &shape.texture_view,
                &detail.texture_view,
            )),
        );
        let full = region == sky_cache::Region::Full;

        for rows in region.ranges(size.y) {
            let slot = target.building;

            if rows.0 == 0 {
                target.eyes[slot][0] = target.eye;
            }

            if rows.0 + rows.1 == size.y {
                target.eyes[slot][1] = target.eye;
            }

            let building = target.slots[slot].1.clone();
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
            let eyes = target.eyes[target.building];

            for destination in [target.older, target.newer] {
                target.eyes[destination] = eyes;
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

    target.blend = if cached {
        target.sky_refresh.progress()
    } else {
        1.0
    };
}
