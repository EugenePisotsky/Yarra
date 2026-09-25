//! Game rain: camera-anchored streak volumes drawn on the resolved HDR image after temporal
//! reconstruction and before bloom/tone mapping. Drop positions come from hashes plus a fall
//! offset accumulated on the CPU, so there is no per-drop simulation state and changing wind
//! never makes drops jump. Occlusion is a soft test against the scene depth; there is no
//! shelter under trees yet.
use crate::{
    ApplyAtmosphere, AtmosphereOwner, AtmosphereState, WorldEnvironmentView, clouds::CloudAssets,
};
use bevy::{
    core_pipeline::{Core3dSystems, schedule::Core3d},
    prelude::*,
    render::{
        RenderApp, RenderStartup,
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, ViewQuery},
        storage::GpuShaderBuffer,
        view::{ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms},
    },
};
use bytemuck::{Pod, Zeroable};

/// Temporary presentation switch for measurements. Weather still computes precipitation.
#[derive(Resource, Clone, Copy, Debug, ExtractResource)]
pub struct PrecipitationPresentation {
    pub enabled: bool,
}
impl Default for PrecipitationPresentation {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Views that draw rain this frame.
#[derive(Component, Clone, ExtractComponent)]
pub struct PrecipitationView;

struct Layer {
    /// Horizontal box edge and height in metres, centred slightly ahead of the camera.
    size: f32,
    height: f32,
    /// Drops at full intensity.
    count: u32,
    alpha: f32,
}
/// Near drops carry detail; wider layers fill the distance at lower screen cost.
const LAYERS: [Layer; 3] = [
    Layer {
        size: 10.0,
        height: 12.0,
        count: 3000,
        alpha: 0.35,
    },
    Layer {
        size: 28.0,
        height: 20.0,
        count: 6000,
        alpha: 0.3,
    },
    Layer {
        size: 70.0,
        height: 40.0,
        count: 8000,
        alpha: 0.3,
    },
];
const FALL_SPEED: f32 = 8.5;
/// Horizontal drift at unscaled weather wind.
const DRIFT_SPEED: f32 = 2.5;
/// Motion-blur length of a streak, as seconds of fall.
const STREAK_SECONDS: f32 = 0.02;
const DROP_WIDTH: f32 = 0.004;
const SOFT_DEPTH: f32 = 0.35;
/// Share of each box placed ahead of the camera, where drops are visible.
const FORWARD_SHIFT: f32 = 0.3;

#[derive(Clone, Copy, Default, Debug, Pod, Zeroable)]
#[repr(C)]
struct RainUniform {
    /// xyz: drop velocity (m/s); w: streak seconds.
    velocity: [f32; 4],
    /// x: drop width (m); y: intensity alpha scale; z: soft depth (m); w: forward shift.
    shape: [f32; 4],
    /// Per layer: xyz accumulated fall offset (m), w horizontal box size.
    offsets: [[f32; 4]; 3],
    /// Per layer: x box height, y alpha, z first instance.
    layers: [[f32; 4]; 3],
}

#[derive(Resource, Clone, Copy, Default, ExtractResource)]
struct RainFrame {
    uniform: RainUniform,
    active: [u32; 3],
}

pub(crate) struct PrecipitationPlugin;
impl Plugin for PrecipitationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PrecipitationPresentation>()
            .init_resource::<RainFrame>();
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        app.add_plugins((
            ExtractResourcePlugin::<RainFrame>::default(),
            ExtractComponentPlugin::<PrecipitationView>::default(),
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

fn sync(
    mut commands: Commands,
    state: Res<AtmosphereState>,
    presentation: Res<PrecipitationPresentation>,
    time: Res<Time>,
    views: Query<(Entity, Option<&PrecipitationView>), With<WorldEnvironmentView>>,
    mut frame: ResMut<RainFrame>,
    mut offsets: Local<[[f64; 3]; 3]>,
) {
    let precipitation = state
        .weather
        .filter(|_| state.owner == AtmosphereOwner::Game && state.profile.outdoor)
        .map_or(0.0, |w| w.precipitation.clamp(0.0, 1.0));
    let wind = state.weather.map_or(1.0, |w| w.wind_strength);
    let direction = state.profile.clouds.wind_degrees.to_radians();
    let velocity = Vec3::new(
        direction.cos() * DRIFT_SPEED * wind,
        -FALL_SPEED,
        direction.sin() * DRIFT_SPEED * wind,
    );
    // Integrate rather than multiply elapsed time by velocity: wind changes stay continuous.
    let step = f64::from(time.delta_secs().min(0.1));
    for (offset, layer) in offsets.iter_mut().zip(&LAYERS) {
        let size = [layer.size, layer.height, layer.size].map(f64::from);
        for axis in 0..3 {
            offset[axis] = (offset[axis] + f64::from(velocity[axis]) * step).rem_euclid(size[axis]);
        }
    }
    let mut first = 0;
    let mut uniform = RainUniform {
        velocity: velocity.extend(STREAK_SECONDS).to_array(),
        shape: [
            DROP_WIDTH,
            0.6 + 0.4 * precipitation,
            SOFT_DEPTH,
            FORWARD_SHIFT,
        ],
        ..default()
    };
    let mut active = [0; 3];
    for (i, layer) in LAYERS.iter().enumerate() {
        let o = offsets[i];
        uniform.offsets[i] = [o[0] as f32, o[1] as f32, o[2] as f32, layer.size];
        uniform.layers[i] = [layer.height, layer.alpha, first as f32, 0.0];
        active[i] = (layer.count as f32 * precipitation).round() as u32;
        first += layer.count;
    }
    *frame = RainFrame { uniform, active };
    let visible = presentation.enabled && active.iter().any(|&n| n > 0);
    for (entity, view) in &views {
        if visible && view.is_none() {
            commands.entity(entity).insert(PrecipitationView);
        } else if !visible && view.is_some() {
            commands.entity(entity).remove::<PrecipitationView>();
        }
    }
}

#[derive(Resource)]
struct Pipelines {
    layout: [BindGroupLayoutDescriptor; 2],
    render: [CachedRenderPipelineId; 2],
}
fn init(mut commands: Commands, server: Res<AssetServer>, cache: Res<PipelineCache>) {
    let layout = std::array::from_fn(|i| {
        BindGroupLayoutDescriptor::new(
            "rain",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::VERTEX_FRAGMENT,
                (
                    uniform_buffer_sized(
                        false,
                        std::num::NonZeroU64::new(std::mem::size_of::<RainUniform>() as u64),
                    ),
                    storage_buffer_read_only_sized(
                        false,
                        std::num::NonZeroU64::new(
                            std::mem::size_of::<crate::clouds::CloudParams>() as u64,
                        ),
                    ),
                    uniform_buffer::<ViewUniform>(true),
                    if i == 0 {
                        texture_depth_2d()
                    } else {
                        texture_depth_2d_multisampled()
                    },
                ),
            ),
        )
    });
    let shader = server.load("shaders/weather/rain.wgsl");
    let render = std::array::from_fn(|i| {
        let defs = if i == 1 {
            vec!["MULTISAMPLED".into()]
        } else {
            vec![]
        };
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("rain".into()),
            layout: vec![layout[i].clone()],
            vertex: VertexState {
                shader: shader.clone(),
                shader_defs: defs.clone(),
                entry_point: Some("vertex".into()),
                buffers: vec![],
            },
            fragment: Some(FragmentState {
                shader: shader.clone(),
                shader_defs: defs,
                entry_point: Some("fragment".into()),
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    // Premultiplied: streak radiance over the scene.
                    blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            ..default()
        })
    });
    commands.insert_resource(Pipelines { layout, render });
}

#[allow(clippy::too_many_arguments)] // Independent render-world resources.
fn draw(
    view: ViewQuery<(
        &PrecipitationView,
        &ViewTarget,
        &ViewDepthTexture,
        &ViewUniformOffset,
        &Msaa,
    )>,
    rain: Res<RainFrame>,
    assets: Option<Res<CloudAssets>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    pipelines: Res<Pipelines>,
    cache: Res<PipelineCache>,
    uniforms: Res<ViewUniforms>,
    mut ctx: RenderContext,
) {
    let (_, target, depth, offset, msaa) = view.into_inner();
    if target.main_texture_format() != TextureFormat::Rgba16Float {
        return;
    }
    let index = usize::from(msaa.samples() > 1);
    let (Some(assets), Some(pipeline), Some(view_binding)) = (
        assets,
        cache.get_render_pipeline(pipelines.render[index]),
        uniforms.uniforms.binding(),
    ) else {
        return;
    };
    let Some(clouds) = buffers.get(&assets.parameters) else {
        return;
    };
    let uniform = ctx
        .render_device()
        .create_buffer_with_data(&BufferInitDescriptor {
            label: Some("rain parameters"),
            contents: bytemuck::bytes_of(&rain.uniform),
            usage: BufferUsages::UNIFORM,
        });
    let group = ctx.render_device().create_bind_group(
        "rain",
        &cache.get_bind_group_layout(&pipelines.layout[index]),
        &BindGroupEntries::sequential((
            uniform.as_entire_binding(),
            clouds.buffer.as_entire_binding(),
            view_binding,
            depth.view(),
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("rain"),
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
    pass.set_bind_group(0, &group, &[offset.offset]);
    let mut first = 0;
    for (layer, &active) in LAYERS.iter().zip(&rain.active) {
        if active > 0 {
            pass.draw(0..6, first..first + active);
        }
        first += layer.count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::weather::WeatherKind;

    fn run(
        kind: Option<WeatherKind>,
        owner: AtmosphereOwner,
        step: f32,
        frames: u32,
    ) -> (RainFrame, bool) {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs_f32(step));
        let camera = app.world_mut().spawn(WorldEnvironmentView::default()).id();
        app.insert_resource(time)
            .insert_resource(AtmosphereState {
                owner,
                weather: kind.map(WeatherKind::preset),
                ..default()
            })
            .init_resource::<PrecipitationPresentation>()
            .init_resource::<RainFrame>()
            .add_systems(Update, sync);
        for _ in 0..frames {
            app.update();
        }
        let visible = app.world().get::<PrecipitationView>(camera).is_some();
        (*app.world().resource::<RainFrame>(), visible)
    }

    #[test]
    fn only_game_precipitation_draws_and_intensity_scales_drop_counts() {
        let game = AtmosphereOwner::Game;
        assert!(
            !run(None, game, 0.016, 1).1,
            "authored weather has no precipitation"
        );
        assert!(!run(Some(WeatherKind::Overcast), game, 0.016, 1).1);
        let (storm, visible) = run(Some(WeatherKind::Storm), game, 0.016, 1);
        assert!(visible);
        assert_eq!(storm.active, LAYERS.map(|l| l.count));
        let (rain, _) = run(Some(WeatherKind::Rain), game, 0.016, 1);
        assert!(rain.active[0] > 0 && rain.active[0] < LAYERS[0].count);
        let editor = AtmosphereOwner::Editor;
        assert!(
            !run(Some(WeatherKind::Storm), editor, 0.016, 1).1,
            "editor workspaces own the authored atmosphere"
        );
    }

    #[test]
    fn fall_offsets_stay_within_each_box_and_advance_with_time() {
        let (frame, _) = run(Some(WeatherKind::Rain), AtmosphereOwner::Game, 0.05, 7);
        for (offset, layer) in frame.uniform.offsets.iter().zip(&LAYERS) {
            assert!((0.0..layer.size).contains(&offset[0]));
            assert!((0.0..layer.height).contains(&offset[1]));
            assert!((0.0..layer.size).contains(&offset[2]));
            assert_eq!(offset[3], layer.size);
        }
        let expected = (-(FALL_SPEED * 0.35)).rem_euclid(LAYERS[0].height);
        assert!((frame.uniform.offsets[0][1] - expected).abs() < 1e-3);
    }
}
