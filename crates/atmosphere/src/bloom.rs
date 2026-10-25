//! Bloom on the rendered image, before the temporal upscaler, so it costs the rendered region's
//! pixels rather than the window's (`shaders/post/bloom.wesl`). Bevy's bloom reads and writes the
//! whole window-sized target after reconstruction, about 0.8 ms at the play setup. This one keeps
//! Bevy's filters and its natural preset's weights, runs the pyramid as compute over the main
//! pass's region, and blends the glow back over that region; reconstruction then carries it to
//! the window. The first level is 512 texels high whatever the resolution, so the glow keeps its
//! reach on screen.
use crate::sky::SkyCompositeDraw;

use bevy::{
    camera::{MainPassResolutionOverride, Viewport},
    core_pipeline::{Core3d, Core3dSystems, FullscreenShader},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        camera::ExtractedCamera,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        texture::{CachedTexture, TextureCache},
        view::ViewTarget,
    },
};

use bytemuck::{Pod, Zeroable};

use std::num::NonZeroU64;

/// A world camera's bloom: the finest level's share of the image, the profile's intensity.
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
#[require(bevy::camera::Hdr)]
pub struct BloomView {
    pub intensity: f32,
}

/// The bloom's draw in the `Core3d` schedule, for passes that read the bloomed image.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BloomDraw;

/// Height of the first level in texels, Bevy's.
const FIRST_HEIGHT: u32 = 512;

/// Levels from the first down to a few texels.
const LEVELS: u32 = FIRST_HEIGHT.ilog2() - 1;

/// Bevy's natural preset: the coarse levels' extra share, which widens the glow; the
/// energy-conserving blend takes it from the image.
const LOW_FREQUENCY_BOOST: f32 = 0.7;

const BOOST_CURVATURE: f32 = 0.95;

const WORKGROUP: u32 = 8;

const FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Uniform entries: the first downsample's, the other downsamples', then the upsample from each
/// level below the first.
const FIRST: u64 = 0;

const PLAIN: u64 = 1;

fn upsample_entry(level: u32) -> u64 {
    1 + level as u64
}

pub(crate) struct BloomPlugin;

impl Plugin for BloomPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<BloomView>::default());
        let Some(render) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render
            .add_systems(RenderStartup, init)
            .add_systems(
                Render,
                (
                    prepare.in_set(RenderSystems::PrepareResources),
                    bind.in_set(RenderSystems::PrepareBindGroups),
                ),
            )
            .add_systems(
                Core3d,
                draw.in_set(BloomDraw)
                    .after(SkyCompositeDraw)
                    .before(Core3dSystems::EarlyPostProcess),
            );
    }
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Level {
    origin: [f32; 2],
    extent: [f32; 2],
    low: [f32; 2],
    high: [f32; 2],
    blend: f32,
    _pad: [f32; 3],
}

/// Per view, this frame's pyramid: each level's downsampled image (`down`), and the glow built
/// back up from the coarsest level (`up`, one level fewer, as the coarsest is its own).
#[derive(Component)]
struct Pyramid {
    down: CachedTexture,
    up: CachedTexture,
    region: URect,
    viewport: Option<Viewport>,
    /// The share of each level's tent in the level above, the first's in the image.
    blend: [f32; LEVELS as usize],
}

/// Per view, the levels' parameters and the bind groups of every step but the first, which
/// reads the view's image, a different texture each frame under the temporal upscaler.
#[derive(Component)]
struct Bindings {
    textures: [TextureId; 2],
    params: Buffer,
    down: Vec<BindGroup>,
    up: Vec<BindGroup>,
    composite: BindGroup,
}

#[derive(Resource)]
struct Pipelines {
    down_layout: BindGroupLayoutDescriptor,
    up_layout: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    downsample_first: CachedComputePipelineId,
    downsample: CachedComputePipelineId,
    upsample: CachedComputePipelineId,
    composite: CachedRenderPipelineId,
    sampler: Sampler,
    stride: u64,
}

fn init(
    mut commands: Commands,
    server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
) {
    let level = uniform_buffer_sized(false, NonZeroU64::new(size_of::<Level>() as u64));
    let storage = texture_storage_2d(FORMAT, StorageTextureAccess::WriteOnly);
    let filtered = texture_2d(TextureSampleType::Float { filterable: true });
    let down_layout = BindGroupLayoutDescriptor::new(
        "bloom downsample",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                filtered,
                sampler(SamplerBindingType::Filtering),
                level,
                storage,
            ),
        ),
    );
    let up_layout = BindGroupLayoutDescriptor::new(
        "bloom upsample",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                filtered,
                sampler(SamplerBindingType::Filtering),
                level,
                storage,
                texture_2d(TextureSampleType::Float { filterable: false }),
            ),
        ),
    );
    let composite_layout = BindGroupLayoutDescriptor::new(
        "bloom composite",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (filtered, sampler(SamplerBindingType::Filtering)),
        ),
    );
    let shader = server.load("shaders/post/bloom.wesl");
    let compute = |entry: &'static str, layout: &BindGroupLayoutDescriptor| {
        cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(format!("bloom {entry}").into()),
            layout: vec![layout.clone()],
            shader: shader.clone(),
            entry_point: Some(entry.into()),
            ..default()
        })
    };
    let downsample_first = compute("downsample_first", &down_layout);
    let downsample = compute("downsample", &down_layout);
    let upsample = compute("upsample", &up_layout);

    // Energy-conserving: the glow replaces its share of the image.
    let composite = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("bloom composite".into()),
        layout: vec![composite_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader,
            entry_point: Some("composite".into()),
            targets: vec![Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::Constant,
                        dst_factor: BlendFactor::OneMinusConstant,
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
    });
    let align = device.limits().min_uniform_buffer_offset_alignment as u64;
    commands.insert_resource(Pipelines {
        down_layout,
        up_layout,
        composite_layout,
        downsample_first,
        downsample,
        upsample,
        composite,
        sampler: device.create_sampler(&SamplerDescriptor {
            label: Some("bloom"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        }),
        stride: (size_of::<Level>() as u64).next_multiple_of(align),
    });
}

/// The first level's size: [`FIRST_HEIGHT`] texels high at the region's aspect.
fn first_level(region: UVec2) -> UVec2 {
    let scale = FIRST_HEIGHT as f32 / region.y.max(1) as f32;
    (region.as_vec2() * scale)
        .round()
        .as_uvec2()
        .max(UVec2::ONE)
}

/// The share of level `level`'s tent in the level above: Bevy's `compute_blend_factor` for its
/// natural preset, energy-conserving.
fn blend(intensity: f32, level: u32) -> f32 {
    let t = level as f32 / (LEVELS - 1) as f32;
    let boost = (1.0 - (1.0 - t).powf(1.0 / (1.0 - BOOST_CURVATURE))) * LOW_FREQUENCY_BOOST;
    intensity + boost * (1.0 - intensity)
}

type PreparedView = (
    Entity,
    &'static BloomView,
    &'static ExtractedCamera,
    &'static ViewTarget,
    Option<&'static MainPassResolutionOverride>,
);

fn prepare(
    mut commands: Commands,
    views: Query<PreparedView>,
    stale: Query<Entity, (With<Pyramid>, Without<BloomView>)>,
    mut cache: ResMut<TextureCache>,
    device: Res<RenderDevice>,
) {
    for entity in &stale {
        commands.entity(entity).remove::<(Pyramid, Bindings)>();
    }

    for (entity, bloom, camera, target, resolution) in &views {
        if !camera.hdr || bloom.intensity <= 0.0 {
            commands.entity(entity).remove::<(Pyramid, Bindings)>();
            continue;
        }

        let viewport = Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution);
        let region = crate::rendered_region(viewport.as_ref(), target);
        let size = first_level(region.size());
        let mut texture = |label, levels| {
            cache.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d {
                        width: size.x,
                        height: size.y,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: levels,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: FORMAT,
                    usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let down = texture("bloom down", LEVELS);
        let up = texture("bloom up", LEVELS - 1);
        commands.entity(entity).insert(Pyramid {
            down,
            up,
            region,
            viewport,
            blend: std::array::from_fn(|level| blend(bloom.intensity, level as u32)),
        });
    }
}

fn level_view(texture: &CachedTexture, level: u32) -> TextureView {
    texture.texture.create_view(&TextureViewDescriptor {
        base_mip_level: level,
        mip_level_count: Some(1),
        ..default()
    })
}

fn bind(
    mut commands: Commands,
    views: Query<(Entity, &Pyramid, &ViewTarget, Option<&Bindings>)>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    for (entity, pyramid, target, bindings) in &views {
        let textures = [pyramid.down.texture.id(), pyramid.up.texture.id()];
        let params = match bindings {
            Some(b) if b.textures == textures => {
                write_levels(&queue, &b.params, pipelines.stride, pyramid, target);
                continue;
            }
            Some(b) => b.params.clone(),
            None => device.create_buffer(&BufferDescriptor {
                label: Some("bloom levels"),
                size: pipelines.stride * (upsample_entry(LEVELS - 1) + 1),
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        };
        write_levels(&queue, &params, pipelines.stride, pyramid, target);

        let entry = |index: u64| BufferBinding {
            buffer: &params,
            offset: index * pipelines.stride,
            size: NonZeroU64::new(size_of::<Level>() as u64),
        };
        let down_views: Vec<_> = (0..LEVELS)
            .map(|level| level_view(&pyramid.down, level))
            .collect();
        let up_views: Vec<_> = (0..LEVELS - 1)
            .map(|level| level_view(&pyramid.up, level))
            .collect();
        let down = (1..LEVELS as usize)
            .map(|level| {
                device.create_bind_group(
                    "bloom downsample",
                    &cache.get_bind_group_layout(&pipelines.down_layout),
                    &BindGroupEntries::sequential((
                        &down_views[level - 1],
                        &pipelines.sampler,
                        entry(PLAIN),
                        &down_views[level],
                    )),
                )
            })
            .collect();

        // From the coarsest level up: each writes the level above it.
        let up = (1..LEVELS as usize)
            .rev()
            .map(|level| {
                let coarser = up_views.get(level).unwrap_or(&down_views[level]);
                device.create_bind_group(
                    "bloom upsample",
                    &cache.get_bind_group_layout(&pipelines.up_layout),
                    &BindGroupEntries::sequential((
                        coarser,
                        &pipelines.sampler,
                        entry(upsample_entry(level as u32)),
                        &up_views[level - 1],
                        &down_views[level - 1],
                    )),
                )
            })
            .collect();
        let composite = device.create_bind_group(
            "bloom composite",
            &cache.get_bind_group_layout(&pipelines.composite_layout),
            &BindGroupEntries::sequential((&up_views[0], &pipelines.sampler)),
        );
        commands.entity(entity).insert(Bindings {
            textures,
            params,
            down,
            up,
            composite,
        });
    }
}

fn write_levels(
    queue: &RenderQueue,
    params: &Buffer,
    stride: u64,
    pyramid: &Pyramid,
    target: &ViewTarget,
) {
    let texture = target.main_texture();
    let size = Vec2::new(texture.width() as f32, texture.height() as f32);
    let (min, max) = (pyramid.region.min.as_vec2(), pyramid.region.max.as_vec2());
    let whole = Level {
        origin: [0.0; 2],
        extent: [1.0; 2],
        low: [0.0; 2],
        high: [1.0; 2],
        blend: 0.0,
        _pad: [0.0; 3],
    };
    let first = Level {
        origin: (min / size).to_array(),
        extent: ((max - min) / size).to_array(),
        low: ((min + 0.5) / size).to_array(),
        high: ((max - 0.5) / size).to_array(),
        ..whole
    };
    let mut bytes = vec![0; (stride * (upsample_entry(LEVELS - 1) + 1)) as usize];
    let mut put = |index: u64, level: Level| {
        let at = (index * stride) as usize;
        bytes[at..at + size_of::<Level>()].copy_from_slice(bytemuck::bytes_of(&level));
    };
    put(FIRST, first);
    put(PLAIN, whole);

    for level in 1..LEVELS {
        let blend = pyramid.blend[level as usize];
        put(upsample_entry(level), Level { blend, ..whole });
    }

    queue.write_buffer(params, 0, &bytes);
}

type DrawnView = (&'static Pyramid, &'static Bindings, &'static ViewTarget);

fn draw(
    view: ViewQuery<DrawnView>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (pyramid, bindings, target) = view.into_inner();
    let (Some(first), Some(downsample), Some(upsample), Some(composite)) = (
        cache.get_compute_pipeline(pipelines.downsample_first),
        cache.get_compute_pipeline(pipelines.downsample),
        cache.get_compute_pipeline(pipelines.upsample),
        cache.get_render_pipeline(pipelines.composite),
    ) else {
        return;
    };

    if target.main_texture_format() != TextureFormat::Rgba16Float {
        return;
    }

    let first_group = ctx.render_device().create_bind_group(
        "bloom first downsample",
        &cache.get_bind_group_layout(&pipelines.down_layout),
        &BindGroupEntries::sequential((
            target.main_texture_view(),
            &pipelines.sampler,
            BufferBinding {
                buffer: &bindings.params,
                offset: FIRST * pipelines.stride,
                size: NonZeroU64::new(size_of::<Level>() as u64),
            },
            &level_view(&pyramid.down, 0),
        )),
    );
    let size = |texture: &CachedTexture, level: u32| {
        let mip = texture
            .texture
            .size()
            .mip_level_size(level, TextureDimension::D2);
        (
            mip.width.div_ceil(WORKGROUP),
            mip.height.div_ceil(WORKGROUP),
        )
    };

    {
        let mut pass = ctx
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("bloom"),
                timestamp_writes: None,
            });
        pass.set_pipeline(first);
        pass.set_bind_group(0, &first_group, &[]);
        let (x, y) = size(&pyramid.down, 0);
        pass.dispatch_workgroups(x, y, 1);
        pass.set_pipeline(downsample);

        for (level, group) in (1..LEVELS).zip(&bindings.down) {
            pass.set_bind_group(0, group, &[]);
            let (x, y) = size(&pyramid.down, level);
            pass.dispatch_workgroups(x, y, 1);
        }

        pass.set_pipeline(upsample);

        for (level, group) in (0..LEVELS - 1).rev().zip(&bindings.up) {
            pass.set_bind_group(0, group, &[]);
            let (x, y) = size(&pyramid.up, level);
            pass.dispatch_workgroups(x, y, 1);
        }
    }

    let mut pass = ctx.begin_tracked_render_pass(RenderPassDescriptor {
        label: Some("bloom composite"),
        color_attachments: &[Some(target.get_unsampled_color_attachment())],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    if let Some(viewport) = &pyramid.viewport {
        pass.set_camera_viewport(viewport);
    }

    pass.set_render_pipeline(composite);
    pass.set_bind_group(0, &bindings.composite, &[]);
    pass.set_blend_constant(LinearRgba::gray(pyramid.blend[0]));
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_halve_from_512_high_to_a_few_texels() {
        assert_eq!(LEVELS, 8);
        assert_eq!(first_level(UVec2::new(1728, 1084)), UVec2::new(816, 512));
        assert_eq!(first_level(UVec2::new(3456, 2168)), UVec2::new(816, 512));
        assert_eq!(FIRST_HEIGHT >> (LEVELS - 1), 4);
    }

    #[test]
    fn coarse_levels_take_more_of_the_glow() {
        let intensity = 0.15;
        assert_eq!(blend(intensity, 0), intensity);
        let coarsest = blend(intensity, LEVELS - 1);
        assert!((coarsest - (intensity + LOW_FREQUENCY_BOOST * (1.0 - intensity))).abs() < 1e-6);
        assert!((1..LEVELS).all(|level| blend(intensity, level) >= blend(intensity, level - 1)));
    }
}
