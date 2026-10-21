//! The trample map: a world-aligned texture that wraps around the camera, in which characters
//! press grass down (`shaders/vegetation/trample.wesl`) and from which it slowly rises. Blades
//! read it at their roots; the previous frame's copy poses them for motion vectors. Texels the
//! camera's movement wraps onto new ground start unpressed.
use crate::{
    MAX_VEGETATION_BENDERS, VegetationBenders, VegetationProfileMode, VegetationRenderOrigin,
};
use crate::{VegetationSettings, VegetationView};

use bevy::{
    prelude::*,
    render::{
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue},
        view::ExtractedView,
    },
};

use bytemuck::{Pod, Zeroable};

/// Texels per side and metres per texel: 64 m of ground at 12.5 cm.
pub(super) const SIDE: u32 = 512;

pub(super) const TEXEL_METRES: f32 = 0.125;
const FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const SHADER_PATH: &str = "shaders/vegetation/trample.wesl";

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct TrampleGpu {
    // xy: canonical centre this frame, zw: last frame's
    centres: [f32; 4],
    // x: metres per texel, y: texels per side, z: seconds since last frame, w: benders
    params: [f32; 4],
    // Per bender: (canonical x, z, radius, 0), (velocity x, z, 0, 0).
    benders: [[f32; 4]; 2 * MAX_VEGETATION_BENDERS],
}

#[derive(Resource)]
pub(super) struct TrampleMap {
    current: Texture,
    pub(super) current_view: TextureView,
    previous: Texture,
    pub(super) previous_view: TextureView,
    pub(super) sampler: Sampler,
    uniform: Buffer,
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
    bind_group: Option<BindGroup>,
    centre: Option<[f32; 2]>,
    stamp: bool,
}

impl FromWorld for TrampleMap {
    fn from_world(world: &mut World) -> Self {
        let device = world.resource::<RenderDevice>();
        let texture = |label| {
            device.create_texture(&TextureDescriptor {
                label: Some(label),
                size: Extent3d {
                    width: SIDE,
                    height: SIDE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: FORMAT,
                usage: TextureUsages::TEXTURE_BINDING
                    | TextureUsages::STORAGE_BINDING
                    | TextureUsages::COPY_SRC
                    | TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let current = texture("vegetation trample map");
        let previous = texture("vegetation trample map, last frame");
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("vegetation trample map"),
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..default()
        });
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("vegetation trample"),
            size: size_of::<TrampleGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = BindGroupLayoutDescriptor::new(
            "vegetation trample",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer_sized(false, None),
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    texture_storage_2d(FORMAT, StorageTextureAccess::WriteOnly),
                ),
            ),
        );
        let shader = world.resource::<AssetServer>().load(SHADER_PATH);
        let pipeline =
            world
                .resource::<PipelineCache>()
                .queue_compute_pipeline(ComputePipelineDescriptor {
                    label: Some("vegetation trample".into()),
                    layout: vec![layout.clone()],
                    shader,
                    entry_point: Some("stamp".into()),
                    ..default()
                });
        Self {
            current_view: current.create_view(&default()),
            previous_view: previous.create_view(&default()),
            current,
            previous,
            sampler,
            uniform,
            layout,
            pipeline,
            bind_group: None,
            centre: None,
            stamp: false,
        }
    }
}

/// The map's canonical centre for a view: the camera's ground position.
pub(super) fn centre(view: &ExtractedView, origin: &VegetationRenderOrigin) -> [f32; 2] {
    let position = view.world_from_view.translation();
    [
        (f64::from(position.x) + origin.world_xz[0]) as f32,
        (f64::from(position.z) + origin.world_xz[1]) as f32,
    ]
}

/// Side of the map in metres.
pub(super) fn extent() -> f32 {
    SIDE as f32 * TEXEL_METRES
}

#[allow(clippy::too_many_arguments)] // Independent render-world resources.
pub(super) fn prepare(
    mut map: ResMut<TrampleMap>,
    benders: Option<Res<VegetationBenders>>,
    origin: Res<VegetationRenderOrigin>,
    settings: Res<VegetationSettings>,
    views: Query<&ExtractedView, With<VegetationView>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
) {
    map.stamp = false;
    let Some(view) = views.iter().next() else {
        return;
    };

    if settings.profile_mode == VegetationProfileMode::Disabled {
        return;
    }

    let centre = centre(view, &origin);
    let previous = map.centre.unwrap_or(centre);
    map.centre = Some(centre);
    let mut gpu = TrampleGpu::zeroed();
    gpu.centres = [centre[0], centre[1], previous[0], previous[1]];
    let mut count = 0;
    let mut delta = 0.0;

    if let Some(benders) = benders {
        delta = benders.delta_seconds.clamp(0.0, 0.1);
        for bender in benders.benders.iter().take(MAX_VEGETATION_BENDERS) {
            gpu.benders[2 * count] = [
                (f64::from(bender.position.x) + origin.world_xz[0]) as f32,
                (f64::from(bender.position.z) + origin.world_xz[1]) as f32,
                bender.radius,
                0.0,
            ];
            gpu.benders[2 * count + 1] = [bender.velocity.x, bender.velocity.y, 0.0, 0.0];
            count += 1;
        }
    }

    gpu.params = [TEXEL_METRES, SIDE as f32, delta, count as f32];
    queue.write_buffer(&map.uniform, 0, bytemuck::bytes_of(&gpu));

    if map.bind_group.is_none() {
        let layout = cache.get_bind_group_layout(&map.layout);
        map.bind_group = Some(device.create_bind_group(
            Some("vegetation trample"),
            &layout,
            &BindGroupEntries::sequential((
                map.uniform.as_entire_binding(),
                &map.previous_view,
                &map.current_view,
            )),
        ));
    }

    map.stamp = true;
}

/// Keeps last frame's map for motion vectors, then recovers and stamps this frame's.
pub(super) fn stamp(
    mut render_context: RenderContext,
    map: Res<TrampleMap>,
    cache: Res<PipelineCache>,
) {
    let (true, Some(bind_group), Some(pipeline)) = (
        map.stamp,
        map.bind_group.as_ref(),
        cache.get_compute_pipeline(map.pipeline),
    ) else {
        return;
    };
    let encoder = render_context.command_encoder();
    encoder.copy_texture_to_texture(
        map.current.as_image_copy(),
        map.previous.as_image_copy(),
        Extent3d {
            width: SIDE,
            height: SIDE,
            depth_or_array_layers: 1,
        },
    );
    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("vegetation trample"),
        ..default()
    });
    pass.set_bind_group(0, bind_group, &[]);
    pass.set_pipeline(pipeline);
    pass.dispatch_workgroups(SIDE / 8, SIDE / 8, 1);
}
