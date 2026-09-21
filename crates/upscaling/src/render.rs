use crate::*;
use backends::{Spatial, linear::Linear};
use bevy::{
    core_pipeline::{schedule::Core3d, upscaling::upscaling as finish_camera},
    render::{
        GpuResourceAppExt, Render, RenderApp, RenderSystems,
        render_asset::RenderAssets,
        render_resource::{
            BindGroup, TextureDimension, TextureFormat, TextureId, TextureUsages, TextureViewId,
        },
        renderer::{RenderContext, RenderDevice, ViewQuery},
        sync_world::MainEntity,
        texture::GpuImage,
    },
};

pub(crate) fn install(app: &mut App, bridge: Bridge) {
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render
        .insert_resource(bridge)
        .init_gpu_resource::<State>()
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(Core3d, upscale.after(finish_camera));
}
#[derive(PartialEq, Eq)]
struct Key(
    TextureId,
    TextureViewId,
    TextureId,
    TextureViewId,
    UpscaleMethod,
);
struct Prepared {
    key: Key,
    input: GpuImage,
    output: GpuImage,
    bind: BindGroup,
    spatial: Option<Spatial>,
    status: UpscaleStatus,
}
#[derive(Resource)]
struct State {
    linear: Linear,
    capabilities: UpscalingCapabilities,
    views: HashMap<Entity, Prepared>,
}
impl FromWorld for State {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let capabilities = UpscalingCapabilities::detect(device);
        world.resource::<Bridge>().0.lock().unwrap().capabilities = capabilities.clone();
        Self {
            linear: Linear::new(device),
            capabilities,
            views: HashMap::new(),
        }
    }
}
fn prepare(
    mut state: ResMut<State>,
    views: Query<(Entity, &MainEntity, &UpscaleView), Without<temporal::TemporalView>>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    bridge: Res<Bridge>,
) {
    state
        .views
        .retain(|entity, _| views.get(*entity).is_ok_and(|(_, _, v)| v.enabled));
    for (entity, main, view) in &views {
        if !view.enabled {
            bridge.0.lock().unwrap().statuses.insert(
                main.id(),
                UpscaleStatus {
                    requested: view.method,
                    active: None,
                    reason: Some("Direct rendering; upscaler bypassed".into()),
                    ..default()
                },
            );
            continue;
        }
        let (Some(input), Some(output)) = (images.get(&view.input), images.get(&view.output))
        else {
            state.views.remove(&entity);
            bridge.0.lock().unwrap().statuses.insert(
                main.id(),
                UpscaleStatus {
                    requested: view.method,
                    ..default()
                },
            );
            continue;
        };
        let key = Key(
            input.texture.id(),
            input.texture_view.id(),
            output.texture.id(),
            output.texture_view.id(),
            view.method,
        );
        if state.views.get(&entity).is_some_and(|p| p.key == key) {
            continue;
        }
        // The public output helper establishes this format contract, shared by all backends.
        if !valid_images(input, output) {
            state.views.remove(&entity);
            bridge.0.lock().unwrap().statuses.insert(
                main.id(),
                UpscaleStatus {
                    requested: view.method,
                    reason: Some("Invalid upscaling input/output images".into()),
                    ..default()
                },
            );
            continue;
        }
        let input_size = UVec2::new(input.texture.width(), input.texture.height());
        let output_size = UVec2::new(output.texture.width(), output.texture.height());
        let (mut active, mut reason) = state.capabilities.select(view.method);
        if active == UpscaleMethod::MetalFxTemporal {
            active = UpscaleMethod::Linear;
            reason =
                Some("Temporal requires a TemporalView with HDR, depth and motion inputs".into());
        }
        if input_size.cmpge(output_size).any() {
            active = UpscaleMethod::Linear;
            reason = Some("No enlargement requested".into());
        }
        let spatial = if active == UpscaleMethod::MetalFxSpatial {
            match Spatial::new(&device, input, output) {
                Ok(s) => Some(s),
                Err(error) => {
                    active = UpscaleMethod::Linear;
                    reason = Some(error);
                    None
                }
            }
        } else {
            None
        };
        let bind = state.linear.bind(&device, input);
        state.views.insert(
            entity,
            Prepared {
                key,
                input: input.clone(),
                output: output.clone(),
                bind,
                spatial,
                status: UpscaleStatus {
                    requested: view.method,
                    active: Some(active),
                    reason,
                    input_size,
                    output_size,
                },
            },
        );
    }
}

fn valid_images(input: &GpuImage, output: &GpuImage) -> bool {
    input.texture.id() != output.texture.id()
        && [input, output].iter().all(|image| {
            image.texture.dimension() == TextureDimension::D2
                && image.texture.depth_or_array_layers() == 1
                && image.texture.sample_count() == 1
        })
        && matches!(
            input.texture.format(),
            TextureFormat::Bgra8UnormSrgb | TextureFormat::Rgba8UnormSrgb
        )
        && input
            .texture
            .usage()
            .contains(TextureUsages::TEXTURE_BINDING)
        && output.texture.format() == TextureFormat::Rgba8Unorm
        && output.texture.usage().contains(
            TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::STORAGE_BINDING
                | TextureUsages::TEXTURE_BINDING,
        )
        && output
            .texture_view_descriptor
            .as_ref()
            .and_then(|v| v.format)
            == Some(TextureFormat::Rgba8UnormSrgb)
}
fn upscale(
    view: ViewQuery<(
        &MainEntity,
        Option<&UpscaleView>,
        Option<&temporal::TemporalView>,
    )>,
    mut state: ResMut<State>,
    bridge: Res<Bridge>,
    mut ctx: RenderContext,
) {
    let entity = view.entity();
    let (main, request, temporal) = view.into_inner();
    if temporal.is_some() {
        return;
    }
    if request.is_none_or(|v| !v.enabled) {
        return;
    }
    let State { linear, views, .. } = &mut *state;
    let Some(prepared) = views.get_mut(&entity) else {
        return;
    };
    if let Some(spatial) = &mut prepared.spatial {
        match spatial.encode(ctx.render_device(), &prepared.input, &prepared.output) {
            Ok(buffers) => {
                for buffer in buffers {
                    ctx.add_command_buffer(buffer);
                }
            }
            Err(error) => {
                prepared.spatial = None;
                prepared.status.active = Some(UpscaleMethod::Linear);
                prepared.status.reason = Some(error);
            }
        }
    }
    if prepared.spatial.is_none() {
        linear.encode(ctx.command_encoder(), &prepared.bind, &prepared.output);
    }
    let mut messages = bridge.0.lock().unwrap();
    messages.statuses.insert(main.id(), prepared.status.clone());
}
