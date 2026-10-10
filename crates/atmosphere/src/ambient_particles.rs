//! Ambient particles near the camera: dust motes, drifting seed fluff and leaves falling from
//! the crowns. Like the rain they live in camera-anchored boxes that wrap a world-fixed lattice,
//! with hashed positions, analytic motion and a wind drift accumulated on the CPU, so nothing is
//! simulated per particle. Motes and fluff are lit through the sun's shadow maps and show
//! mostly in sunbeams and against the light; leaves exist only under crowns of the forest map
//! and fall to the ground of the mist map. Drawn on the resolved HDR image with the rain.
use crate::{
    ApplyAtmosphere, AtmospherePresentation, AtmosphereState, WorldEnvironmentView,
    environment::{EnvironmentAssets, EnvironmentParams},
};
use bevy::{
    core_pipeline::{Core3dSystems, schedule::Core3d},
    pbr::{GpuLights, LightMeta, ShadowSamplers, ViewLightsUniformOffset, ViewShadowBindings},
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, ViewQuery},
        storage::GpuShaderBuffer,
        texture::GpuImage,
        view::{ViewDepthStencilTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};
use bytemuck::{Pod, Zeroable};
use world::atmosphere::evaluate;

/// Views that draw ambient particles this frame.
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct AmbientParticlesView;

struct Kind {
    /// Horizontal box edge and height in metres. Fluff fills a layer of that height over the
    /// ground; leaves take their height from the crowns.
    size: f32,
    height: f32,
    count: u32,
    /// Share of the wind drift the kind follows, and its own vertical speed (m/s).
    drift: f32,
    rise: f32,
}
const MOTES: usize = 0;
const FLUFF: usize = 1;
const LEAVES: usize = 2;
const KINDS: [Kind; 3] = [
    Kind {
        size: 6.0,
        height: 3.5,
        count: 8000,
        drift: 0.04,
        rise: 0.0,
    },
    Kind {
        size: 28.0,
        height: 5.0,
        count: 800,
        drift: 0.8,
        rise: -0.05,
    },
    Kind {
        size: 36.0,
        height: 1.0,
        count: 1600,
        drift: 0.6,
        rise: 0.0,
    },
];
/// Horizontal wind drift at unscaled weather wind, as for the rain.
const DRIFT_SPEED: f32 = 2.5;
/// Every periodic motion completes whole cycles in this many seconds, so wrapping the clock
/// never makes a particle jump.
const MOTION_PERIOD: f64 = 3600.0;

#[derive(Clone, Copy, Default, Debug, Pod, Zeroable)]
#[repr(C)]
struct AmbientUniform {
    /// x: fraction of the motion period; y: the period in seconds.
    time: [f32; 4],
    /// xyz: wind drift velocity (m/s) at full share.
    drift: [f32; 4],
    /// Per kind: horizontal box size, box height, first instance, instance count.
    kinds: [[f32; 4]; 3],
    /// Per kind: accumulated drift (m), wrapped by its box; w the kind's drift share.
    offsets: [[f32; 4]; 3],
    /// Per kind: share of the instances present, 0..1.
    amounts: [f32; 4],
}

#[derive(Resource, Clone, Copy, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
struct AmbientFrame {
    uniform: AmbientUniform,
    /// Instances to draw per kind; the last few fade with the amount.
    active: [u32; 3],
}

pub(crate) struct AmbientParticlesPlugin;
impl Plugin for AmbientParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AmbientFrame>();
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractResourcePlugin::<AmbientFrame>::default(),
            ExtractComponentPlugin::<AmbientParticlesView>::default(),
        ))
        .add_systems(PostUpdate, sync.after(ApplyAtmosphere));
        app.sub_app_mut(RenderApp)
            .add_systems(RenderStartup, init)
            .add_systems(
                Core3d,
                draw.after(Core3dSystems::EarlyPostProcess)
                    .before(Core3dSystems::PostProcess),
            );
    }
}

/// Share of each kind present: motes and fluff need a dry day, and wind stirs up fluff and
/// shakes leaves loose while it scatters the motes.
fn amounts(state: &AtmosphereState) -> [f32; 3] {
    if state.isolated() || !state.profile.outdoor {
        return [0.0; 3];
    }
    let sun = evaluate(&state.profile, state.phase).direction_to_sun[1];
    let day = smoothstep(-0.02, 0.08, sun);
    let (stir, rain) = state.weather.map_or((1.0, 0.0), |w| {
        (w.wind_strength, w.precipitation.clamp(0.0, 1.0))
    });
    // Rain washes the air; wet ground raises less.
    let dry = (1.0 - smoothstep(0.0, 0.3, rain)) * (1.0 - 0.7 * state.wetness.clamp(0.0, 1.0));
    let mut share = [0.0; 3];
    share[MOTES] = day * dry * (1.3 - 0.4 * stir).clamp(0.2, 1.0);
    share[FLUFF] = day * dry * (0.4 + 0.3 * stir).clamp(0.2, 1.0);
    share[LEAVES] = (0.2 + 0.3 * stir).clamp(0.1, 1.0);
    share
}
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Slow swells of the wind, shared by every particle.
fn gust(seconds: f64) -> f32 {
    let t = seconds as f32;
    1.0 + 0.35 * (t * 0.21).sin() * (t * 0.13 + 1.0).sin()
}

type ParticleViews<'w, 's> =
    Query<'w, 's, (Entity, Option<&'static AmbientParticlesView>), With<WorldEnvironmentView>>;

fn sync(
    mut commands: Commands,
    state: Res<AtmosphereState>,
    presentation: Option<Res<AtmospherePresentation>>,
    time: Res<Time>,
    views: ParticleViews,
    mut frame: ResMut<AmbientFrame>,
    mut offsets: Local<[[f64; 3]; 3]>,
) {
    let enabled = presentation.is_none_or(|p| p.particles);
    let amounts = if enabled { amounts(&state) } else { [0.0; 3] };
    let wind = state.weather.map_or(1.0, |w| w.wind_strength);
    let direction = state.profile.clouds.wind_degrees.to_radians();
    let elapsed = time.elapsed_secs_f64();
    let drift = Vec3::new(direction.cos(), 0.0, direction.sin()) * DRIFT_SPEED * wind;
    let step = f64::from(time.delta_secs().min(0.1));
    let velocity = drift * gust(elapsed);
    for (offset, kind) in offsets.iter_mut().zip(&KINDS) {
        let v = [velocity.x * kind.drift, kind.rise, velocity.z * kind.drift];
        let size = [kind.size, kind.height, kind.size];
        for axis in 0..3 {
            offset[axis] =
                (offset[axis] + f64::from(v[axis]) * step).rem_euclid(f64::from(size[axis]));
        }
    }
    let mut uniform = AmbientUniform {
        time: [
            (elapsed.rem_euclid(MOTION_PERIOD) / MOTION_PERIOD) as f32,
            MOTION_PERIOD as f32,
            0.0,
            0.0,
        ],
        drift: velocity.extend(0.0).to_array(),
        amounts: [amounts[0], amounts[1], amounts[2], 0.0],
        ..default()
    };
    let mut first = 0;
    let mut active = [0; 3];
    for (i, kind) in KINDS.iter().enumerate() {
        let o = offsets[i];
        uniform.kinds[i] = [kind.size, kind.height, first as f32, kind.count as f32];
        uniform.offsets[i] = [o[0] as f32, o[1] as f32, o[2] as f32, kind.drift];
        // The shader fades the last 5% of the present share.
        active[i] = if amounts[i] > 0.0 {
            ((amounts[i] + 0.05).min(1.0) * kind.count as f32).ceil() as u32
        } else {
            0
        };
        first += kind.count;
    }
    *frame = AmbientFrame { uniform, active };
    let visible = active.iter().any(|&n| n > 0);
    for (entity, view) in &views {
        if visible && view.is_none() {
            commands.entity(entity).insert(AmbientParticlesView);
        } else if !visible && view.is_some() {
            commands.entity(entity).remove::<AmbientParticlesView>();
        }
    }
}

#[derive(Resource)]
struct Pipelines {
    layout: [BindGroupLayoutDescriptor; 2],
    render: [CachedRenderPipelineId; 2],
    repeat: Sampler,
    clamp: Sampler,
}
fn init(
    mut commands: Commands,
    server: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<bevy::render::renderer::RenderDevice>,
) {
    let layout = std::array::from_fn(|i| {
        BindGroupLayoutDescriptor::new(
            "ambient particles",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer_sized(
                        false,
                        std::num::NonZeroU64::new(std::mem::size_of::<AmbientUniform>() as u64),
                    ),
                    storage_buffer_read_only_sized(
                        false,
                        std::num::NonZeroU64::new(std::mem::size_of::<EnvironmentParams>() as u64),
                    ),
                    uniform_buffer::<ViewUniform>(true),
                    if i == 0 {
                        texture_depth_2d()
                    } else {
                        texture_depth_2d_multisampled()
                    },
                    uniform_buffer::<GpuLights>(true),
                    texture_2d_array(TextureSampleType::Depth),
                    sampler(SamplerBindingType::Comparison),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                ),
            ),
        )
    });
    let shader = server.load("shaders/weather/ambient.wesl");
    let render = std::array::from_fn(|i| {
        let defs = if i == 1 {
            vec!["MULTISAMPLED".into()]
        } else {
            vec![]
        };
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("ambient particles".into()),
            layout: vec![layout[i].clone()],
            vertex: VertexState {
                shader: shader.clone(),
                shader_defs: defs.clone(),
                entry_point: Some("vertex".into()),
                buffers: vec![],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: shader.clone(),
                shader_defs: defs,
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        })
    });
    let filtered = |address_mode| {
        device.create_sampler(&SamplerDescriptor {
            label: Some("ambient particles"),
            address_mode_u: address_mode,
            address_mode_v: address_mode,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..default()
        })
    };
    commands.insert_resource(Pipelines {
        layout,
        render,
        repeat: filtered(AddressMode::Repeat),
        clamp: filtered(AddressMode::ClampToEdge),
    });
}

#[allow(clippy::too_many_arguments)] // Independent render-world resources.
fn draw(
    view: ViewQuery<(
        &AmbientParticlesView,
        &ViewTarget,
        &ViewDepthStencilTexture,
        &ViewUniformOffset,
        &ViewLightsUniformOffset,
        &ViewShadowBindings,
        &Msaa,
    )>,
    frame: Res<AmbientFrame>,
    assets: Option<Res<EnvironmentAssets>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    images: Res<RenderAssets<GpuImage>>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    uniforms: Res<ViewUniforms>,
    lights: Res<LightMeta>,
    shadow_samplers: Res<ShadowSamplers>,
    mut ctx: RenderContext,
) {
    let (_, target, depth, offset, light_offset, shadows, msaa) = view.into_inner();
    if target.main_texture_format() != TextureFormat::Rgba16Float {
        return;
    }
    let index = usize::from(msaa.samples() > 1);
    let (Some(assets), Some(pipeline), Some(view_binding), Some(light_binding)) = (
        assets,
        cache.get_render_pipeline(pipelines.render[index]),
        uniforms.uniforms.binding(),
        lights.view_gpu_lights.binding(),
    ) else {
        return;
    };
    let (Some(clouds), Some(cloud_shadow), Some(forest), Some(mist)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.shadows),
        images.get(&assets.forest_shadow),
        images.get(&assets.mist),
    ) else {
        return;
    };
    let uniform = ctx
        .render_device()
        .create_buffer_with_data(&BufferInitDescriptor {
            label: Some("ambient particle parameters"),
            contents: bytemuck::bytes_of(&frame.uniform),
            usage: BufferUsages::UNIFORM,
        });
    let group = ctx.render_device().create_bind_group(
        "ambient particles",
        &cache.get_bind_group_layout(&pipelines.layout[index]),
        &BindGroupEntries::sequential((
            uniform.as_entire_binding(),
            clouds.buffer.as_entire_binding(),
            view_binding,
            crate::sampled_depth(depth),
            light_binding,
            &shadows.directional_light_depth_texture_view,
            &shadow_samplers.directional_light_comparison_sampler,
            &cloud_shadow.texture_view,
            &pipelines.repeat,
            &forest.texture_view,
            &mist.texture_view,
            &pipelines.clamp,
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("ambient particles"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target.main_texture_view(),
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[offset.offset, light_offset.offset]);
    let mut first = 0;
    for (kind, &active) in KINDS.iter().zip(&frame.active) {
        if active > 0 {
            pass.draw(0..6, first..first + active);
        }
        first += kind.count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AtmosphereOwner;
    use world::weather::WeatherKind;

    fn state(kind: Option<WeatherKind>, phase: f32) -> AtmosphereState {
        AtmosphereState {
            weather: kind.map(WeatherKind::preset),
            phase,
            ..default()
        }
    }

    #[test]
    fn motes_and_fluff_need_a_dry_day_and_leaves_fall_in_any_weather() {
        let morning = amounts(&state(None, AtmosphereState::default().phase));
        assert!(morning.iter().all(|&a| a > 0.1 && a <= 1.0), "{morning:?}");
        let night = amounts(&state(None, 0.0));
        assert_eq!(night[MOTES], 0.0);
        assert_eq!(night[FLUFF], 0.0);
        assert_eq!(night[LEAVES], morning[LEAVES]);
        let rain = amounts(&state(Some(WeatherKind::Rain), 0.34));
        assert_eq!((rain[MOTES], rain[FLUFF]), (0.0, 0.0));
        assert!(rain[LEAVES] > morning[LEAVES], "wind shakes leaves loose");
        let isolated = AtmosphereState {
            owner: AtmosphereOwner::Isolated,
            ..state(None, 0.34)
        };
        assert_eq!(amounts(&isolated), [0.0; 3]);
    }

    fn run(presentation: AtmospherePresentation, frames: u32) -> (AmbientFrame, bool) {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        let camera = app.world_mut().spawn(WorldEnvironmentView::default()).id();
        app.insert_resource(time)
            .insert_resource(AtmosphereState::default())
            .insert_resource(presentation)
            .init_resource::<AmbientFrame>()
            .add_systems(Update, sync);
        for _ in 0..frames {
            app.update();
        }
        let visible = app.world().get::<AmbientParticlesView>(camera).is_some();
        (*app.world().resource::<AmbientFrame>(), visible)
    }

    #[test]
    fn drift_stays_within_each_box_and_the_toggle_hides_every_kind() {
        let (frame, visible) = run(AtmospherePresentation::default(), 9);
        assert!(visible);
        for (i, kind) in KINDS.iter().enumerate() {
            let offset = frame.uniform.offsets[i];
            assert!((0.0..kind.size).contains(&offset[0]));
            assert!((0.0..kind.height).contains(&offset[1]));
            assert!((0.0..kind.size).contains(&offset[2]));
            assert!(frame.active[i] > 0 && frame.active[i] <= kind.count);
        }
        assert_eq!(frame.uniform.kinds[LEAVES][2], (8000 + 800) as f32);
        let (frame, visible) = run(
            AtmospherePresentation {
                particles: false,
                ..default()
            },
            2,
        );
        assert!(!visible);
        assert_eq!(frame.active, [0; 3]);
    }
}
