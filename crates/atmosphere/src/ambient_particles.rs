//! Ambient particles near the camera: dust motes, drifting seed fluff, leaves falling from the
//! crowns and, where it snows, falling snowflakes. Like the rain they live in camera-anchored boxes that wrap a world-fixed lattice,
//! with hashed positions, analytic motion and a wind drift accumulated on the CPU, so nothing is
//! simulated per particle. Motes and fluff are lit through the sun's shadow maps and show
//! mostly in sunbeams and against the light; leaves exist only under crowns of the forest map
//! and fall to the ground of the mist map. Snow shaken off boughs falls the same way from where
//! it lay, as powder and clumps, from bursts the trees write into a ring on the GPU
//! ([`snow_falls_handle`], [`SnowFalls`]). Drawn on the resolved HDR image with the rain.
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
        storage::{GpuShaderBuffer, ShaderBuffer},
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
    /// ground; leaves take their height from the crowns; snowflakes fall through the box.
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
const NEAR_FLAKES: usize = 3;
const FAR_FLAKES: usize = 4;

/// Near flakes carry the detail; a wider, sparser box fills the middle distance, past which the
/// weather's fog takes over.
const KINDS: [Kind; 5] = [
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
    Kind {
        size: 10.0,
        height: 8.0,
        count: 20000,
        drift: 0.9,
        rise: -1.0,
    },
    Kind {
        size: 40.0,
        height: 24.0,
        count: 24000,
        drift: 0.9,
        rise: -1.2,
    },
];

/// Every periodic motion completes whole cycles in this many seconds, so wrapping the clock
/// never makes a particle jump.
const MOTION_PERIOD: f64 = 3600.0;

/// Bursts of shaken snow the ring holds (`SnowFall` in `shaders/weather/snow_fall.wesl`, four
/// vec4s); the oldest gives way.
pub const SNOW_FALL_CAPACITY: usize = 256;

const SNOW_FALL_BYTES: usize = 64;

/// Particles a whole burst holds (`SNOW_PARTICLES` in `shaders/weather/ambient.wesl`).
const SNOW_FALL_PARTICLES: u32 = 64;

/// A burst's last particle has settled by then, seconds: the latest born (`CLUMP_BIRTH`, `POWDER_BIRTH`)
/// lasts `SNOW_SECONDS`.
const SNOW_FALL_SECONDS: f32 = 3.6;

/// The ring of shaken snow bursts, which what shakes the snow writes on the GPU and the ambient
/// particles draw.
pub fn snow_falls_handle() -> Handle<ShaderBuffer> {
    bevy::asset::uuid_handle!("7c3e9a14-58d2-4b6f-a1e0-2f9b6c8d4e53")
}

/// Whether bursts may be falling: whatever writes them says so each frame it might have.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct SnowFalls {
    until: f32,
}

impl SnowFalls {
    /// Snow may have been shaken loose at `now`: the bursts draw for as long as one lasts.
    pub fn shaking(&mut self, now: f32) {
        self.until = now + SNOW_FALL_SECONDS;
    }

    fn falling(&self, now: f32) -> bool {
        now < self.until
    }
}

/// The ring, every burst long gone.
fn setup_falls(mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let mut gone = [0.0_f32; SNOW_FALL_BYTES / 4];
    gone[3] = -1.0e30;
    buffers
        .insert(
            snow_falls_handle().id(),
            ShaderBuffer::new(
                vec![gone; SNOW_FALL_CAPACITY],
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            ),
        )
        .expect("the snow falls' handle is free");
}

#[derive(Clone, Copy, Default, Debug, Pod, Zeroable)]
#[repr(C)]
struct AmbientUniform {
    /// x: fraction of the motion period; y: the period in seconds; z: the game clock, seconds.
    time: [f32; 4],
    /// xyz: wind drift velocity (m/s) at full share.
    drift: [f32; 4],
    /// Per kind: horizontal box size, box height, first instance, instance count.
    kinds: [[f32; 4]; 5],
    /// Per kind: accumulated drift (m), wrapped by its box.
    offsets: [[f32; 4]; 5],
    /// Per kind: its velocity (m/s), the wind's share it follows and its own fall.
    velocities: [[f32; 4]; 5],
    /// Per kind: share of the instances present, 0..1.
    amounts: [[f32; 4]; 2],
}

#[derive(Resource, Clone, Copy, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
struct AmbientFrame {
    uniform: AmbientUniform,
    /// Instances to draw per kind; the last few fade with the amount.
    active: [u32; 5],
    /// Whether any burst of shaken snow is falling.
    snow: bool,
}

pub(crate) struct AmbientParticlesPlugin;

impl Plugin for AmbientParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AmbientFrame>()
            .init_resource::<SnowFalls>();

        if app.get_sub_app(RenderApp).is_none() {
            return;
        }

        app.add_systems(Startup, setup_falls);

        app.add_plugins((
            ExtractResourcePlugin::<AmbientFrame>::default(),
            ExtractComponentPlugin::<AmbientParticlesView>::default(),
        ))
        .add_systems(
            PostUpdate,
            sync.after(ApplyAtmosphere).after(crate::wind::advance),
        );
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
/// shakes leaves loose while it scatters the motes. Snowflakes fall as the weather's snow does,
/// by day and night, as the square of its snowfall: light snow keeps a few, a blizzard fills the
/// air. Where it snows no leaves fall.
fn amounts(state: &AtmosphereState) -> [f32; 5] {
    if state.isolated() || !state.profile.outdoor {
        return [0.0; 5];
    }

    let sun = evaluate(&state.profile, state.phase).direction_to_sun[1];
    let day = smoothstep(-0.02, 0.08, sun);
    let (stir, rain) = state.weather.map_or((1.0, 0.0), |w| {
        (w.wind_strength, w.precipitation.clamp(0.0, 1.0))
    });

    // Rain washes the air; wet ground raises less.
    let dry = (1.0 - smoothstep(0.0, 0.3, rain)) * (1.0 - 0.7 * state.wetness.clamp(0.0, 1.0));
    let snow = state.snow.clamp(0.0, 1.0);
    let mut share = [0.0; 5];
    share[MOTES] = day * dry * (1.3 - 0.4 * stir).clamp(0.2, 1.0);
    share[FLUFF] = day * dry * (0.4 + 0.3 * stir).clamp(0.2, 1.0);
    share[LEAVES] = (0.2 + 0.3 * stir).clamp(0.1, 1.0) * (1.0 - snow);
    share[NEAR_FLAKES] = rain * rain * snow;
    share[FAR_FLAKES] = rain * rain * snow;
    share
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

type ParticleViews<'w, 's> =
    Query<'w, 's, (Entity, Option<&'static AmbientParticlesView>), With<WorldEnvironmentView>>;

/// What the particles follow: the weather and time of day, the gusts, the clock, and snow shaken
/// off.
#[derive(bevy::ecs::system::SystemParam)]
struct ParticleInputs<'w> {
    state: Res<'w, AtmosphereState>,
    gusts: Res<'w, crate::wind::Gusts>,
    presentation: Option<Res<'w, AtmospherePresentation>>,
    time: Res<'w, Time>,
    falls: Res<'w, SnowFalls>,
}

fn sync(
    mut commands: Commands,
    inputs: ParticleInputs,
    views: ParticleViews,
    mut frame: ResMut<AmbientFrame>,
    mut offsets: Local<[[f64; 3]; 5]>,
) {
    let ParticleInputs {
        state,
        gusts,
        presentation,
        time,
        falls,
    } = inputs;
    let enabled = presentation.is_none_or(|p| p.particles);
    let amounts = if enabled { amounts(&state) } else { [0.0; 5] };
    let elapsed = time.elapsed_secs_f64();
    let drift = Vec3::new(gusts.direction.x, 0.0, gusts.direction.y) * gusts.speed;
    let step = f64::from(time.delta_secs().min(0.1));

    // Every particle moves with the gust at the camera.
    let velocity = drift * gusts.here;

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
            time.elapsed_secs(),
            0.0,
        ],
        drift: velocity.extend(0.0).to_array(),
        amounts: [
            [amounts[0], amounts[1], amounts[2], amounts[3]],
            [amounts[4], 0.0, 0.0, 0.0],
        ],
        ..default()
    };
    let mut first = 0;
    let mut active = [0; 5];

    for (i, kind) in KINDS.iter().enumerate() {
        let o = offsets[i];
        uniform.kinds[i] = [kind.size, kind.height, first as f32, kind.count as f32];
        uniform.offsets[i] = [o[0] as f32, o[1] as f32, o[2] as f32, 0.0];
        uniform.velocities[i] = [
            velocity.x * kind.drift,
            kind.rise,
            velocity.z * kind.drift,
            0.0,
        ];

        // The shader fades the last 5% of the present share.
        active[i] = if amounts[i] > 0.0 {
            ((amounts[i] + 0.05).min(1.0) * kind.count as f32).ceil() as u32
        } else {
            0
        };
        first += kind.count;
    }

    let snow = enabled && falls.falling(time.elapsed_secs());
    *frame = AmbientFrame {
        uniform,
        active,
        snow,
    };
    let visible = snow || active.iter().any(|&n| n > 0);

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
    /// The kinds' and shaken snow's, by MSAA.
    render: [CachedRenderPipelineId; 2],
    snow: [CachedRenderPipelineId; 2],
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
                    storage_buffer_read_only_sized(
                        false,
                        std::num::NonZeroU64::new((SNOW_FALL_CAPACITY * SNOW_FALL_BYTES) as u64),
                    ),
                ),
            ),
        )
    });
    let shader = server.load("shaders/weather/ambient.wesl");
    let pipeline = |i: usize, vertex: &'static str| {
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
                entry_point: Some(vertex.into()),
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
    };
    let render = std::array::from_fn(|i| pipeline(i, "vertex"));
    let snow = std::array::from_fn(|i| pipeline(i, "vertex_snow"));
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
        snow,
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
    let (Some(clouds), Some(cloud_shadow), Some(forest), Some(mist), Some(falls)) = (
        buffers.get(&assets.parameters),
        images.get(&assets.shadows),
        images.get(&assets.forest_shadow),
        images.get(&assets.mist),
        buffers.get(&snow_falls_handle()),
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
            falls.buffer.as_entire_binding(),
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

    if frame.snow
        && let Some(snow) = cache.get_render_pipeline(pipelines.snow[index])
    {
        pass.set_pipeline(snow);
        pass.draw(0..6, 0..SNOW_FALL_CAPACITY as u32 * SNOW_FALL_PARTICLES);
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
        assert!(
            morning[..NEAR_FLAKES].iter().all(|&a| a > 0.1 && a <= 1.0),
            "{morning:?}"
        );
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
        assert_eq!(amounts(&isolated), [0.0; 5]);
    }

    #[test]
    fn shaken_snow_draws_for_as_long_as_a_burst_lasts() {
        let mut falls = SnowFalls::default();
        assert!(!falls.falling(0.0));
        falls.shaking(10.0);
        assert!(falls.falling(10.0 + SNOW_FALL_SECONDS - 0.1));
        assert!(!falls.falling(10.0 + SNOW_FALL_SECONDS + 0.1));
    }

    fn run(presentation: AtmospherePresentation, frames: u32) -> (AmbientFrame, bool) {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs_f32(0.05));
        let camera = app.world_mut().spawn(WorldEnvironmentView).id();
        app.insert_resource(time)
            .insert_resource(AtmosphereState::default())
            .insert_resource(presentation)
            .init_resource::<AmbientFrame>()
            .init_resource::<crate::wind::Gusts>()
            .init_resource::<SnowFalls>()
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
            assert!(frame.active[i] <= kind.count);
            assert_eq!(
                frame.active[i] > 0,
                i < NEAR_FLAKES,
                "no snow on a clear day"
            );
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
        assert_eq!(frame.active, [0; 5]);
    }

    #[test]
    fn snowflakes_fall_where_the_precipitation_is_snow_and_leaves_stop() {
        let snowing = |snow| {
            amounts(&AtmosphereState {
                snow,
                ..state(Some(WeatherKind::Rain), 0.34)
            })
        };
        let rain = snowing(0.0);
        assert_eq!((rain[NEAR_FLAKES], rain[FAR_FLAKES]), (0.0, 0.0));
        let snow = snowing(1.0);
        assert!(
            snow[NEAR_FLAKES] > 0.2 && snow[FAR_FLAKES] > 0.2,
            "{snow:?}"
        );
        assert_eq!(snow[LEAVES], 0.0);
        let blizzard = amounts(&AtmosphereState {
            snow: 1.0,
            ..state(Some(WeatherKind::Storm), 0.34)
        });
        assert_eq!(blizzard[NEAR_FLAKES], 1.0, "a blizzard fills the air");
        assert!(blizzard[NEAR_FLAKES] > 2.0 * snow[NEAR_FLAKES] - 0.01);
        let night = amounts(&AtmosphereState {
            snow: 1.0,
            ..state(Some(WeatherKind::Rain), 0.0)
        });
        assert_eq!(
            night[NEAR_FLAKES], snow[NEAR_FLAKES],
            "snow falls at night too"
        );
    }
}
