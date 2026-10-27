//! Shared temporal inputs for custom renderers. Motion is unjittered current-minus-previous UV.
//! Backend-specific pixel units and history live behind the upscaling backend boundary.
use crate::*;

use bevy::{
    camera::MainPassResolutionOverride,
    core_pipeline::{
        Core3dSystems,
        prepass::{DepthPrepass, MotionVectorPrepass, ViewPrepassTextures},
        schedule::Core3d,
    },
    render::{
        GpuResourceAppExt, Render, RenderApp, RenderSystems,
        camera::{MipBias, TemporalJitter},
        render_resource::*,
        renderer::{RenderContext, RenderDevice, ViewQuery},
        sync_world::MainEntity,
        view::{ExtractedView, ViewDepthStencilTexture, ViewTarget, prepare_view_targets},
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemporalDebug {
    #[default]
    Off,
    Motion,
    Depth,
    /// Profile the surrounding temporal pipeline without running reconstruction.
    Bypass,
}

impl TemporalDebug {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Motion,
            Self::Motion => Self::Depth,
            Self::Depth => Self::Bypass,
            Self::Bypass => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Image",
            Self::Motion => "Motion vectors",
            Self::Depth => "Depth",
            Self::Bypass => "Reconstruction bypass (diagnostic)",
        }
    }
}

#[derive(Component, Clone, ExtractComponent, PartialEq)]
#[extract_app(bevy::render::RenderApp)]
#[require(
    TemporalJitter,
    MipBias,
    DepthPrepass,
    MotionVectorPrepass,
    bevy::camera::Hdr
)]
pub struct TemporalView {
    pub input_size: UVec2,
    /// Change on explicit camera cuts/world changes. Resize and large camera jumps also reset.
    pub reset_epoch: u64,
    pub debug: TemporalDebug,
    pub resolve: TemporalResolve,
}

/// What turns a view's jittered frames into its image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TemporalResolve {
    /// MetalFX Temporal reconstructs the output, enlarging the input.
    #[default]
    MetalFx,
    /// Temporal anti-aliasing (`taa` in `temporal.wgsl`) at the input's size: the output must be
    /// as large as the input.
    Taa,
}

#[derive(Component, Clone, Copy)]
pub struct TemporalFrame {
    pub size: UVec2,
    pub jitter: Vec2,
    pub clip_from_world: Mat4,
    pub previous_clip_from_world: Mat4,
    /// Where renderers with their own clip transform rasterise: the jittered clip position is
    /// the unjittered one plus `raster_shift * w` in x and y.
    pub raster_shift: Vec2,
    pub reset: bool,
}

/// How far `TemporalJitter::jitter_projection` moves a clip position in x and y, per unit of its
/// w, on the perspective projection `clip_from_view`. Its jitter is a shift of the projection's z
/// column, so it moves a point by its view depth, which w is a multiple of.
fn raster_shift(clip_from_view: Mat4, jitter: Vec2, size: Vec2) -> Vec2 {
    let mut jittered = clip_from_view;
    TemporalJitter { offset: jitter }.jitter_projection(&mut jittered, size);
    (jittered.z_axis.xy() - clip_from_view.z_axis.xy()) / clip_from_view.z_axis.w
}

/// Custom opaque renderers write this attachment together with their HDR colour and final depth.
#[derive(Component, Clone)]
pub struct TemporalMotionTarget {
    pub texture: Texture,
    pub view: TextureView,
}

#[cfg_attr(
    not(all(feature = "metalfx", any(target_os = "macos", target_os = "ios"))),
    allow(dead_code)
)]
pub(crate) struct Inputs<'a> {
    pub color: &'a Texture,
    pub depth: &'a Texture,
    pub motion: &'a Texture,
    pub output: &'a Texture,
    pub output_view: &'a TextureView,
    pub size: UVec2,
    pub jitter: Vec2,
    pub reset: bool,
}

pub(crate) fn install(app: &mut App) {
    app.add_plugins(ExtractComponentPlugin::<TemporalView>::default());
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .init_gpu_resource::<State>()
        .add_systems(
            Render,
            prepare
                .in_set(RenderSystems::PrepareViews)
                .before(prepare_view_targets),
        )
        .add_systems(
            Core3d,
            initialize_motion
                .after(Core3dSystems::Prepass)
                .before(Core3dSystems::MainPass),
        )
        .add_systems(Core3d, resolve.in_set(Core3dSystems::EarlyPostProcess));
}

struct History {
    size: UVec2,
    output_size: UVec2,
    clip: Mat4,
    position: Vec3,
    forward: Vec3,
    epoch: u64,
    index: u32,
    motion: TemporalMotionTarget,
    resolve: TemporalResolve,
    resolver: Resolver,
    last_pair: Option<[TextureId; 2]>,
}

enum Resolver {
    MetalFx(Result<backends::Temporal, String>),
    Taa(TaaHistory),
}

impl Resolver {
    fn new(device: &RenderDevice, resolve: TemporalResolve, size: UVec2, output: UVec2) -> Self {
        match resolve {
            TemporalResolve::MetalFx => {
                Self::MetalFx(backends::Temporal::new(device, size, output))
            }
            TemporalResolve::Taa => Self::Taa(TaaHistory::new(device, size)),
        }
    }

    /// Whether frames are jittered for it: not for a MetalFX scaler that failed to start.
    fn running(&self) -> bool {
        !matches!(self, Self::MetalFx(Err(_)))
    }
}

/// The image TAA blends each frame into: one texture holds last frame's while the other takes
/// this frame's, in turn.
struct TaaHistory {
    views: [TextureView; 2],
    read: usize,
}

impl TaaHistory {
    fn new(device: &RenderDevice, size: UVec2) -> Self {
        let view = || {
            device
                .create_texture(&TextureDescriptor {
                    label: Some("temporal anti-aliasing history"),
                    size: Extent3d {
                        width: size.x,
                        height: size.y,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba16Float,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&default())
        };
        Self {
            views: [view(), view()],
            read: 0,
        }
    }
}

#[derive(Resource)]
struct State {
    histories: HashMap<Entity, History>,
    motion_pipeline: wgpu::RenderPipeline,
    motion_layout: wgpu::BindGroupLayout,
    display_pipeline: wgpu::RenderPipeline,
    display_layout: wgpu::BindGroupLayout,
    taa: TaaPass,
    sampler: wgpu::Sampler,
}

struct TaaPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl FromWorld for State {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>().wgpu_device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("temporal input preparation and diagnostics"),
            source: wgpu::ShaderSource::Wgsl(include_str!("temporal.wgsl").into()),
        });
        let pipeline = |entry: &str, formats: &[TextureFormat]| {
            let targets: Vec<_> = formats
                .iter()
                .map(|&format| {
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })
                })
                .collect();
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    buffers: &[],
                    compilation_options: default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: default(),
                    targets: &targets,
                }),
                primitive: default(),
                depth_stencil: None,
                multisample: default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let motion_pipeline = pipeline("motion", &[TextureFormat::Rg16Float]);
        let display_pipeline = pipeline("display", &[TextureFormat::Rgba16Float]);

        // The view's image and the history it leaves for the next frame.
        let taa_pipeline = pipeline("taa", &[TextureFormat::Rgba16Float; 2]);
        Self {
            histories: default(),
            motion_layout: motion_pipeline.get_bind_group_layout(0),
            motion_pipeline,
            display_layout: display_pipeline.get_bind_group_layout(0),
            display_pipeline,
            taa: TaaPass {
                layout: taa_pipeline.get_bind_group_layout(0),
                pipeline: taa_pipeline,
            },
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..default()
            }),
        }
    }
}

fn halton(mut n: u32, base: u32) -> f32 {
    let (mut result, mut f) = (0., 1.);

    while n > 0 {
        f /= base as f32;
        result += f * (n % base) as f32;
        n /= base;
    }

    result
}

pub fn jitter(index: u32) -> Vec2 {
    Vec2::new(halton(index % 32 + 1, 2), halton(index % 32 + 1, 3)) - Vec2::splat(0.5)
}

#[allow(clippy::type_complexity)] // Render-view inputs are one Bevy query.
fn prepare(
    diagnostics: Option<Res<crate::UpscalingDiagnostics>>,
    mut commands: Commands,
    mut state: ResMut<State>,
    device: Res<RenderDevice>,
    mut views: Query<(
        Entity,
        &ExtractedView,
        Option<&TemporalView>,
        Option<&TemporalFrame>,
        &mut Camera3d,
        &mut bevy::camera::CameraMainTextureUsages,
        Option<&mut TemporalJitter>,
        Option<&mut MipBias>,
    )>,
) {
    state.histories.retain(|e, _| {
        views
            .get(*e)
            .is_ok_and(|(_, _, t, _, _, _, _, _)| t.is_some())
    });
    for (entity, view, request, owned_frame, mut camera, mut usage, temporal_jitter, bias) in
        &mut views
    {
        let Some(request) = request else {
            if owned_frame.is_some() {
                commands.entity(entity).remove::<(
                    MainPassResolutionOverride,
                    TemporalFrame,
                    TemporalMotionTarget,
                )>();
            }
            continue;
        };
        let size = request.input_size.max(UVec2::ONE).min(view.viewport.zw());
        let output_size = view.viewport.zw();
        let position = view.world_from_view.translation();
        let forward = *view.world_from_view.forward();
        let clip = view.clip_from_view * view.world_from_view.to_matrix().inverse();
        let start = Start {
            size,
            output_size,
            clip,
            position,
            forward,
            epoch: request.reset_epoch,
            resolve: request.resolve,
        };
        let h = state
            .histories
            .entry(entity)
            .or_insert_with(|| make_history(&device, start));

        if h.size != size || h.output_size != output_size || h.resolve != request.resolve {
            *h = make_history(&device, start);
        }

        if let Resolver::MetalFx(Ok(native)) = &mut h.resolver {
            native.set_timing(diagnostics.as_ref().is_some_and(|d| d.temporal_timing));
        }

        let camera_delta = h.position.distance(position);
        let direction_dot = h.forward.dot(forward);
        let reset = h.index == 0
            || h.epoch != request.reset_epoch
            || camera_delta > 8.0
            || direction_dot < 0.5;

        if reset && diagnostics.as_ref().is_some_and(|d| d.temporal_timing) {
            // Log every reset, not just the backend's sampled timing frames. This
            // distinguishes coordinate/camera cuts from local mesh/LOD draw gaps.
            warn!(
                "TEMPORAL_HISTORY_RESET view={entity:?} history_frame={} initial={} epoch_changed={} camera_delta_m={camera_delta:.3} direction_dot={direction_dot:.4} input={size:?} output={output_size:?}",
                h.index,
                h.index == 0,
                h.epoch != request.reset_epoch,
            );
        }

        let offset = if h.resolver.running() {
            jitter(h.index)
        } else {
            Vec2::ZERO
        };

        if let Some(mut j) = temporal_jitter {
            j.offset = offset;
        }

        if let Some(mut b) = bias {
            b.0 = (size.x as f32 / output_size.x as f32).log2();
        }

        usage.0 |= TextureUsages::STORAGE_BINDING;
        camera.depth_texture_usages = (TextureUsages::from(camera.depth_texture_usages)
            | TextureUsages::TEXTURE_BINDING)
            .into();
        commands.entity(entity).insert((
            MainPassResolutionOverride(size),
            h.motion.clone(),
            TemporalFrame {
                size,
                jitter: offset,
                clip_from_world: clip,
                previous_clip_from_world: if reset { clip } else { h.clip },
                raster_shift: raster_shift(view.clip_from_view, offset, size.as_vec2()),
                reset,
            },
        ));
        h.clip = clip;
        h.position = position;
        h.forward = forward;
        h.epoch = request.reset_epoch;
        h.index = h.index.wrapping_add(1).max(1);
    }
}

/// A view's camera and sizes when its history starts.
#[derive(Clone, Copy)]
struct Start {
    size: UVec2,
    output_size: UVec2,
    clip: Mat4,
    position: Vec3,
    forward: Vec3,
    epoch: u64,
    resolve: TemporalResolve,
}

fn make_history(device: &RenderDevice, start: Start) -> History {
    let Start {
        size,
        output_size,
        clip,
        position,
        forward,
        epoch,
        resolve,
    } = start;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("complete scene temporal motion"),
        size: Extent3d {
            width: output_size.x,
            height: output_size.y,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::Rg16Float,
        usage: TextureUsages::TEXTURE_BINDING
            | TextureUsages::RENDER_ATTACHMENT
            | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&default());
    History {
        size,
        output_size,
        clip,
        position,
        forward,
        epoch,
        index: 0,
        motion: TemporalMotionTarget { texture, view },
        resolve,
        resolver: Resolver::new(device, resolve, size, output_size),
        last_pair: None,
    }
}

#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
struct Params {
    previous_from_current: [f32; 16],
    size: [f32; 4],
    jitter_debug: [f32; 4],
}

fn params(frame: &TemporalFrame, output: UVec2, debug: TemporalDebug) -> Params {
    Params {
        previous_from_current: (frame.previous_clip_from_world * frame.clip_from_world.inverse())
            .to_cols_array(),
        size: [
            frame.size.x as f32,
            frame.size.y as f32,
            output.x as f32,
            output.y as f32,
        ],
        jitter_debug: [
            frame.jitter.x,
            frame.jitter.y,
            match debug {
                TemporalDebug::Off => 0.,
                TemporalDebug::Motion => 1.,
                TemporalDebug::Depth => 2.,
                TemporalDebug::Bypass => 0.,
            },
            0.,
        ],
    }
}

fn initialize_motion(
    view: ViewQuery<(&TemporalFrame, &TemporalMotionTarget, &ViewPrepassTextures)>,
    state: Res<State>,
    mut ctx: RenderContext,
) {
    let (frame, target, prepass) = view.into_inner();
    let (Some(motion), Some(depth)) = (&prepass.motion_vectors, &prepass.depth) else {
        return;
    };
    let device = ctx.render_device();
    let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("temporal motion parameters"),
        contents: bytemuck::bytes_of(&params(frame, frame.size, TemporalDebug::Off)),
        usage: BufferUsages::UNIFORM,
    });
    let group = device
        .wgpu_device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene motion inputs"),
            layout: &state.motion_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&motion.texture.default_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth.texture.default_view),
                },
            ],
        });
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("initialize complete scene motion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..default()
        });
    pass.set_pipeline(&state.motion_pipeline);
    pass.set_bind_group(0, &group, &[]);
    pass.set_viewport(0., 0., frame.size.x as f32, frame.size.y as f32, 0., 1.);
    pass.draw(0..3, 0..1);
}

fn resolve(
    view: ViewQuery<(
        &MainEntity,
        &TemporalView,
        &TemporalFrame,
        &TemporalMotionTarget,
        &ViewTarget,
        &ViewDepthStencilTexture,
    )>,
    mut state: ResMut<State>,
    bridge: Res<Bridge>,
    diagnostics: Option<Res<crate::UpscalingDiagnostics>>,
    mut ctx: RenderContext,
) {
    let entity = view.entity();
    let (main, request, frame, motion, target, depth) = view.into_inner();
    let state = &mut *state;
    let Some(history) = state.histories.get_mut(&entity) else {
        return;
    };
    let pp = target.post_process_write();
    let pair = [pp.source_texture.id(), pp.destination_texture.id()];
    let targets_changed = history
        .last_pair
        .is_none_or(|old| !pair.iter().all(|id| old.contains(id)));
    let reset = frame.reset || targets_changed;

    if targets_changed && diagnostics.as_ref().is_some_and(|d| d.temporal_timing) {
        warn!(
            "TEMPORAL_HISTORY_RESET view={entity:?} history_frame={} targets_changed=true initial_targets={}",
            history.index.saturating_sub(1),
            history.last_pair.is_none(),
        );
    }

    history.last_pair = Some(pair);
    let mut failure = match &history.resolver {
        Resolver::MetalFx(Err(error)) => Some(error.clone()),
        _ => None,
    };

    if request.debug == TemporalDebug::Off {
        match &mut history.resolver {
            Resolver::MetalFx(Ok(native)) => {
                match native.encode(
                    ctx.render_device(),
                    Inputs {
                        color: pp.source_texture,
                        depth: depth.texture(),
                        motion: &motion.texture,
                        output: pp.destination_texture,
                        output_view: pp.destination,
                        size: frame.size,
                        jitter: frame.jitter,
                        reset,
                    },
                ) {
                    Ok(buffers) => {
                        for b in buffers {
                            ctx.add_command_buffer(b);
                        }
                    }
                    Err(error) => {
                        failure = Some(error);
                    }
                }
            }
            Resolver::Taa(taa) => {
                let inputs = TaaInputs {
                    frame,
                    reset,
                    motion: &motion.view,
                    depth: sampled_depth(depth),
                    color: pp.source,
                    output: pp.destination,
                };
                anti_alias(&state.taa, &state.sampler, taa, inputs, &mut ctx);
            }
            Resolver::MetalFx(Err(_)) => {}
        }
    }

    if let Some(error) = &failure {
        history.resolver = Resolver::MetalFx(Err(error.clone()));
    }

    let output_size = history.output_size;
    let active = match history.resolver {
        Resolver::MetalFx(Ok(_)) => UpscaleMethod::MetalFxTemporal,
        Resolver::Taa(_) => UpscaleMethod::Taa,
        Resolver::MetalFx(Err(_)) => UpscaleMethod::Linear,
    };

    // Debugging consumes no history: the next normal frame must reset the scaler.
    if request.debug != TemporalDebug::Off {
        history.last_pair = None;
    }

    if failure.is_some() || request.debug != TemporalDebug::Off {
        let device = ctx.render_device();
        let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("temporal display parameters"),
            contents: bytemuck::bytes_of(&params(frame, output_size, request.debug)),
            usage: BufferUsages::UNIFORM,
        });
        let group = device
            .wgpu_device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("temporal diagnostic/fallback"),
                layout: &state.display_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&motion.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(sampled_depth(depth)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(pp.source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&state.sampler),
                    },
                ],
            });
        let mut pass = ctx
            .command_encoder()
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("temporal diagnostic/fallback"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: pp.destination,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..default()
            });
        pass.set_pipeline(&state.display_pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }

    bridge.0.lock().unwrap().statuses.insert(
        main.id(),
        UpscaleStatus {
            requested: match request.resolve {
                TemporalResolve::MetalFx => UpscaleMethod::MetalFxTemporal,
                TemporalResolve::Taa => UpscaleMethod::Taa,
            },
            active: (request.debug == TemporalDebug::Off).then_some(active),
            reason: failure.or_else(|| {
                (request.debug != TemporalDebug::Off).then(|| request.debug.label().into())
            }),
            input_size: frame.size,
            output_size,
        },
    );
}

/// What one TAA resolve reads and writes besides its history.
struct TaaInputs<'a> {
    frame: &'a TemporalFrame,
    reset: bool,
    motion: &'a wgpu::TextureView,
    depth: &'a wgpu::TextureView,
    color: &'a wgpu::TextureView,
    output: &'a wgpu::TextureView,
}

/// Blends this frame into the view's history (`taa` in `temporal.wgsl`), writing the view's
/// image and the history the next frame reads.
fn anti_alias(
    pass: &TaaPass,
    sampler: &wgpu::Sampler,
    taa: &mut TaaHistory,
    inputs: TaaInputs,
    ctx: &mut RenderContext,
) {
    let device = ctx.render_device();
    let mut parameters = params(inputs.frame, inputs.frame.size, TemporalDebug::Off);
    parameters.jitter_debug[3] = f32::from(u8::from(inputs.reset));
    let uniform = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("temporal anti-aliasing parameters"),
        contents: bytemuck::bytes_of(&parameters),
        usage: BufferUsages::UNIFORM,
    });
    let group = device
        .wgpu_device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("temporal anti-aliasing inputs"),
            layout: &pass.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(inputs.motion),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(inputs.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(inputs.color),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&taa.views[taa.read]),
                },
            ],
        });
    let attachment = |view| {
        Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })
    };
    let written = 1 - taa.read;
    let mut render = ctx
        .command_encoder()
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("temporal anti-aliasing"),
            color_attachments: &[attachment(inputs.output), attachment(&taa.views[written])],
            ..default()
        });
    render.set_pipeline(&pass.pipeline);
    render.set_bind_group(0, &group, &[]);
    render.draw(0..3, 0..1);
    taa.read = written;
}

/// The view of the main pass depth that passes sample; since Bevy 0.20 a depth texture may carry
/// a stencil aspect, which a binding cannot include.
fn sampled_depth(depth: &ViewDepthStencilTexture) -> &wgpu::TextureView {
    depth
        .attachment
        .depth_stencil_views()
        .depth_only_view()
        .expect("view depth textures have a depth aspect")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::CameraProjection;

    #[test]
    fn jitter_is_bounded_and_visits_both_sides() {
        let samples: Vec<_> = (0..32).map(jitter).collect();
        assert!(samples.iter().all(|p| p.abs().max_element() <= 0.5));
        assert!(samples.iter().any(|p| p.x < -0.4) && samples.iter().any(|p| p.x > 0.4));
        assert_eq!(jitter(0), jitter(32));
    }

    #[test]
    fn raster_shift_matches_the_jittered_projection() {
        let size = Vec2::new(1728.0, 1084.0);
        let clip_from_view = PerspectiveProjection {
            aspect_ratio: size.x / size.y,
            ..default()
        }
        .get_clip_from_view();
        let view_from_world = Transform::from_xyz(3.0, 2.0, -5.0)
            .looking_at(Vec3::new(10.0, 0.0, 20.0), Vec3::Y)
            .to_matrix()
            .inverse();
        let offset = Vec2::new(0.31, -0.42);
        let mut jittered = clip_from_view;
        TemporalJitter { offset }.jitter_projection(&mut jittered, size);
        let shift = raster_shift(clip_from_view, offset, size);

        for point in [Vec3::new(4.0, 0.5, 12.0), Vec3::new(-30.0, 3.0, 80.0)] {
            let clip = clip_from_view * view_from_world * point.extend(1.0);
            let raster = jittered * view_from_world * point.extend(1.0);
            let shifted = clip + (shift * clip.w).extend(0.0).extend(0.0);
            assert!(
                (raster - shifted).abs().max_element() < 1e-5,
                "{raster} vs {shifted}"
            );
        }
    }
}
