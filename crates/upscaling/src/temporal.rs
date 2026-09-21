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
        view::{ExtractedView, ViewDepthTexture, ViewTarget, prepare_view_targets},
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
}
#[derive(Component, Clone, Copy)]
pub struct TemporalFrame {
    pub size: UVec2,
    pub jitter: Vec2,
    pub clip_from_world: Mat4,
    pub previous_clip_from_world: Mat4,
    pub raster_clip_from_world: Mat4,
    pub reset: bool,
}
/// Custom opaque renderers write this attachment together with their HDR colour and final depth.
#[derive(Component, Clone)]
pub struct TemporalMotionTarget {
    pub texture: Texture,
    pub view: TextureView,
}
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct InitializeTemporalMotion;
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResolveTemporal;

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
                .in_set(InitializeTemporalMotion)
                .after(Core3dSystems::Prepass)
                .before(Core3dSystems::MainPass),
        )
        .add_systems(
            Core3d,
            resolve
                .in_set(ResolveTemporal)
                .in_set(Core3dSystems::EarlyPostProcess),
        );
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
    native: Result<backends::Temporal, String>,
    last_pair: Option<[TextureId; 2]>,
}
#[derive(Resource)]
struct State {
    histories: HashMap<Entity, History>,
    motion_pipeline: wgpu::RenderPipeline,
    motion_layout: wgpu::BindGroupLayout,
    display_pipeline: wgpu::RenderPipeline,
    display_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}
impl FromWorld for State {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>().wgpu_device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("temporal input preparation and diagnostics"),
            source: wgpu::ShaderSource::Wgsl(include_str!("temporal.wgsl").into()),
        });
        let pipeline = |entry: &str, format| {
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
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: default(),
                depth_stencil: None,
                multisample: default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let motion_pipeline = pipeline("motion", TextureFormat::Rg16Float);
        let display_pipeline = pipeline("display", TextureFormat::Rgba16Float);
        Self {
            histories: default(),
            motion_layout: motion_pipeline.get_bind_group_layout(0),
            motion_pipeline,
            display_layout: display_pipeline.get_bind_group_layout(0),
            display_pipeline,
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
        let h = state.histories.entry(entity).or_insert_with(|| {
            make_history(
                &device,
                size,
                output_size,
                clip,
                position,
                forward,
                request.reset_epoch,
            )
        });
        if h.size != size || h.output_size != output_size {
            *h = make_history(
                &device,
                size,
                output_size,
                clip,
                position,
                forward,
                request.reset_epoch,
            );
        }
        let reset = h.index == 0
            || h.epoch != request.reset_epoch
            || h.position.distance(position) > 8.0
            || h.forward.dot(forward) < 0.5;
        let offset = if h.native.is_ok() {
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
        let mut projection = view.clip_from_view;
        TemporalJitter { offset }.jitter_projection(&mut projection, size.as_vec2());
        commands.entity(entity).insert((
            MainPassResolutionOverride(size),
            h.motion.clone(),
            TemporalFrame {
                size,
                jitter: offset,
                clip_from_world: clip,
                previous_clip_from_world: if reset { clip } else { h.clip },
                raster_clip_from_world: projection * view.world_from_view.to_matrix().inverse(),
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
fn make_history(
    device: &RenderDevice,
    size: UVec2,
    output_size: UVec2,
    clip: Mat4,
    position: Vec3,
    forward: Vec3,
    epoch: u64,
) -> History {
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
        native: backends::Temporal::new(device, size, output_size),
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
        &ViewDepthTexture,
    )>,
    mut state: ResMut<State>,
    bridge: Res<Bridge>,
    mut ctx: RenderContext,
) {
    let entity = view.entity();
    let (main, request, frame, motion, target, depth) = view.into_inner();
    let Some(history) = state.histories.get_mut(&entity) else {
        return;
    };
    let pp = target.post_process_write();
    let pair = [pp.source_texture.id(), pp.destination_texture.id()];
    let reset = frame.reset
        || history
            .last_pair
            .is_none_or(|old| !pair.iter().all(|id| old.contains(id)));
    history.last_pair = Some(pair);
    let mut failure = history.native.as_ref().err().cloned();
    if request.debug == TemporalDebug::Off
        && let Ok(native) = &mut history.native
    {
        match native.encode(
            ctx.render_device(),
            Inputs {
                color: pp.source_texture,
                depth: &depth.texture,
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
    if let Some(error) = &failure {
        history.native = Err(error.clone());
    }
    let output_size = history.output_size;
    let native_active = history.native.is_ok();
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
                        resource: wgpu::BindingResource::TextureView(depth.view()),
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
            requested: UpscaleMethod::MetalFxTemporal,
            active: (request.debug == TemporalDebug::Off).then_some(if native_active {
                UpscaleMethod::MetalFxTemporal
            } else {
                UpscaleMethod::Linear
            }),
            reason: failure.or_else(|| {
                (request.debug != TemporalDebug::Off).then(|| request.debug.label().into())
            }),
            input_size: frame.size,
            output_size,
        },
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jitter_is_bounded_and_visits_both_sides() {
        let samples: Vec<_> = (0..32).map(jitter).collect();
        assert!(samples.iter().all(|p| p.abs().max_element() <= 0.5));
        assert!(samples.iter().any(|p| p.x < -0.4) && samples.iter().any(|p| p.x > 0.4));
        assert_eq!(jitter(0), jitter(32));
    }
}
