//! Characters pushing branches aside (`shaders/tree/touch.wesl`): a world-aligned 3D field
//! around the player, 16 m across and 8 m high at 25 cm, in which every voxel is a spring holding
//! how far the foliage resting there is pushed. Each frame a compute pass
//! (`shaders/tree/touch_stamp.wesl`) moves the springs towards the push of the characters among
//! the plants ([`VegetationBenders`], as upright capsules) and lets the others swing back; tree
//! vertices read it at their rest position and turn their limb about its root by it, in every
//! pass, so shadows and motion follow. Voxels the window's movement wraps onto new ground start at
//! rest.
#[cfg(test)]
mod gpu_tests;

use bevy::{
    asset::RenderAssetUsages,
    core_pipeline::schedule::camera_driver,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        Render, RenderApp, RenderStartup, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue},
        storage::{GpuShaderBuffer, ShaderBuffer},
        texture::GpuImage,
    },
};

use bytemuck::{Pod, Zeroable};
use vegetation_render::{
    MAX_VEGETATION_BENDERS, VegetationBenders, VegetationLodFocus, VegetationRenderOrigin,
};

const SHADER: &str = "shaders/tree/touch_stamp.wesl";

/// Voxels across the field and up it, and metres per voxel.
const SIDE: u32 = 64;

const HEIGHT: u32 = 32;

const VOXEL_METRES: f32 = 0.25;

/// How far above the player's feet the field's middle lies: it reaches 3 m below them, down
/// slopes, and 5 m above.
const CENTRE_RISE: f32 = 1.0;

/// A character's body as branches meet it: an upright capsule.
const BODY_RADIUS: f32 = 0.3;

const BODY_HEIGHT: f32 = 1.8;

const FORMAT: TextureFormat = TextureFormat::Rgba16Float;

/// Voxels per side of a workgroup.
const WORKGROUP: u32 = 4;

pub(super) fn field_handle() -> Handle<Image> {
    bevy::asset::uuid_handle!("2b6f0e8a-4c1d-4f37-9a5e-7d3c8b1f6e24")
}

pub(super) fn window_handle() -> Handle<ShaderBuffer> {
    bevy::asset::uuid_handle!("9e4a7c21-5b3f-4d8e-a106-3f2d9c7b5a18")
}

/// Where the field lies (`TouchWindow` in the shaders), which every tree material binds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C)]
struct TouchWindow {
    /// The centre in canonical coordinates (render x and z plus the origin's) and metres per
    /// voxel, 0 without a field.
    centre: [f32; 4],
    /// Voxels across, voxels high, seconds since the last frame.
    sides: [f32; 4],
    /// The render origin's canonical x and z.
    origin: [f32; 4],
    /// The columns whose voxels move this frame, which the pass marks: none until then.
    moving: [u32; 4],
}

/// No column marked yet: the lowest above the highest.
const NONE_MOVING: [u32; 4] = [u32::MAX, u32::MAX, 0, 0];

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct TouchGpu {
    window: TouchWindow,
    /// Last frame's centre, and the bodies.
    previous: [f32; 4],
    /// Per body: canonical feet and radius; velocity x, z and height.
    bodies: [[f32; 4]; 2 * MAX_VEGETATION_BENDERS],
}

/// Whether characters push branches aside (F1 compares it off): off, no field is stepped and tree
/// vertices skip it.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeTouch {
    pub enabled: bool,
}

impl Default for TreeTouch {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// This frame's field: where it lies and the bodies in it.
#[derive(Resource, ExtractResource, Clone, Debug, Default)]
#[extract_app(bevy::render::RenderApp)]
pub(super) struct TouchFrame {
    window: TouchWindow,
    bodies: Vec<[[f32; 4]; 2]>,
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TouchFrame>()
        .init_resource::<TreeTouch>()
        .add_systems(Startup, setup)
        .add_systems(
            PostUpdate,
            follow_bodies.after(crate::world_vegetation::list_vegetation_benders),
        );

    if app.get_sub_app(RenderApp).is_none() {
        return;
    }

    app.add_plugins(ExtractResourcePlugin::<TouchFrame>::default());
    app.sub_app_mut(RenderApp)
        .add_systems(RenderStartup, init_pass)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
        .add_systems(RenderGraph, stamp.before(camera_driver));
}

/// The field and its window under the handles every tree material binds.
fn setup(mut images: ResMut<Assets<Image>>, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let mut field = Image::new_uninit(
        Extent3d {
            width: SIDE,
            height: HEIGHT,
            depth_or_array_layers: SIDE,
        },
        TextureDimension::D3,
        FORMAT,
        RenderAssetUsages::RENDER_WORLD,
    );
    field.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING | TextureUsages::COPY_SRC;
    field.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    images
        .insert(field_handle().id(), field)
        .expect("the touch field's handle is free");
    buffers
        .insert(
            window_handle().id(),
            ShaderBuffer::new(vec![TouchWindow::zeroed()], RenderAssetUsages::RENDER_WORLD),
        )
        .expect("the touch window's handle is free");
}

/// The field follows the player; the characters among the plants push it.
fn follow_bodies(
    time: Res<Time>,
    touch: Res<TreeTouch>,
    focus: Option<Res<VegetationLodFocus>>,
    origin: Option<Res<VegetationRenderOrigin>>,
    benders: Option<Res<VegetationBenders>>,
    mut frame: ResMut<TouchFrame>,
) {
    frame.bodies.clear();
    let Some(feet) = focus.and_then(|f| f.position).filter(|_| touch.enabled) else {
        frame.window = TouchWindow::default();
        return;
    };
    let origin = origin.map_or([0.0; 2], |o| o.world_xz);
    let canonical = |p: Vec3| {
        [
            (f64::from(p.x) + origin[0]) as f32,
            p.y,
            (f64::from(p.z) + origin[1]) as f32,
        ]
    };
    let [x, y, z] = canonical(feet + Vec3::Y * CENTRE_RISE);
    frame.window = TouchWindow {
        centre: [x, y, z, VOXEL_METRES],
        sides: [SIDE as f32, HEIGHT as f32, time.delta_secs(), 0.0],
        origin: [origin[0] as f32, origin[1] as f32, 0.0, 0.0],
        moving: NONE_MOVING,
    };

    for bender in benders.iter().flat_map(|b| &b.benders) {
        let [x, y, z] = canonical(bender.position);
        frame.bodies.push([
            [x, y, z, BODY_RADIUS],
            [bender.velocity.x, bender.velocity.y, BODY_HEIGHT, 0.0],
        ]);
    }

    frame.bodies.truncate(MAX_VEGETATION_BENDERS);
}

#[derive(Resource)]
struct TouchPass {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
    uniform: Buffer,
    previous: Texture,
    previous_view: TextureView,
    bind_group: Option<BindGroup>,
    /// Where the field lay last frame it was stepped.
    centre: Option<[f32; 3]>,
    step: bool,
}

fn init_pass(
    mut commands: Commands,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    server: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "tree touch",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer_sized(false, None),
                texture_3d(TextureSampleType::Float { filterable: false }),
                texture_storage_3d(FORMAT, StorageTextureAccess::WriteOnly),
                storage_buffer_sized(false, None),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("tree touch".into()),
        layout: vec![layout.clone()],
        shader: server.load(SHADER),
        entry_point: Some("stamp".into()),
        ..default()
    });
    let previous = device.create_texture(&TextureDescriptor {
        label: Some("tree touch, last frame"),
        size: Extent3d {
            width: SIDE,
            height: HEIGHT,
            depth_or_array_layers: SIDE,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D3,
        format: FORMAT,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    commands.insert_resource(TouchPass {
        layout,
        pipeline,
        uniform: device.create_buffer(&BufferDescriptor {
            label: Some("tree touch"),
            size: size_of::<TouchGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        previous_view: previous.create_view(&default()),
        previous,
        bind_group: None,
        centre: None,
        step: false,
    });
}

#[allow(clippy::too_many_arguments)] // The pass, the frame and the GPU assets it writes.
fn prepare(
    mut pass: ResMut<TouchPass>,
    frame: Res<TouchFrame>,
    images: Res<RenderAssets<GpuImage>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
) {
    pass.step = false;

    // Materials read the window even while there is no field: it tells them so.
    let Some(window) = buffers.get(&window_handle()) else {
        return;
    };
    let unmarked = TouchWindow {
        moving: NONE_MOVING,
        ..frame.window
    };
    queue.write_buffer(&window.buffer, 0, bytemuck::bytes_of(&unmarked));
    let [x, y, z, voxel] = frame.window.centre;
    let Some(field) = images.get(&field_handle()).filter(|_| voxel > 0.0) else {
        pass.centre = None;
        return;
    };

    // A field taken up again starts at rest: last frame's lay a whole window away.
    let centre = [x, y, z];
    let previous = pass
        .centre
        .unwrap_or([x + SIDE as f32 * voxel, y, z + SIDE as f32 * voxel]);
    pass.centre = Some(centre);
    let mut gpu = TouchGpu::zeroed();
    gpu.window = frame.window;
    gpu.previous = [
        previous[0],
        previous[1],
        previous[2],
        frame.bodies.len() as f32,
    ];

    for (i, body) in frame.bodies.iter().enumerate() {
        gpu.bodies[2 * i] = body[0];
        gpu.bodies[2 * i + 1] = body[1];
    }

    queue.write_buffer(&pass.uniform, 0, bytemuck::bytes_of(&gpu));

    if pass.bind_group.is_none() {
        let bind_group = device.create_bind_group(
            Some("tree touch"),
            &cache.get_bind_group_layout(&pass.layout),
            &BindGroupEntries::sequential((
                pass.uniform.as_entire_binding(),
                &pass.previous_view,
                &field.texture_view,
                window.buffer.as_entire_binding(),
            )),
        );
        pass.bind_group = Some(bind_group);
    }

    pass.step = true;
}

/// Keeps last frame's field, then steps this frame's.
fn stamp(
    mut render_context: RenderContext,
    pass: Res<TouchPass>,
    images: Res<RenderAssets<GpuImage>>,
    cache: Res<PipelineCache>,
) {
    let (true, Some(bind_group), Some(pipeline), Some(field)) = (
        pass.step,
        pass.bind_group.as_ref(),
        cache.get_compute_pipeline(pass.pipeline),
        images.get(&field_handle()),
    ) else {
        return;
    };
    let encoder = render_context.command_encoder();
    encoder.copy_texture_to_texture(
        field.texture.as_image_copy(),
        pass.previous.as_image_copy(),
        Extent3d {
            width: SIDE,
            height: HEIGHT,
            depth_or_array_layers: SIDE,
        },
    );
    let mut compute = encoder.begin_compute_pass(&ComputePassDescriptor {
        label: Some("tree touch"),
        ..default()
    });
    compute.set_pipeline(pipeline);
    compute.set_bind_group(0, bind_group, &[]);
    compute.dispatch_workgroups(SIDE / WORKGROUP, HEIGHT / WORKGROUP, SIDE / WORKGROUP);
}

#[cfg(test)]
mod tests {
    use super::*;

    use vegetation_render::VegetationBender;

    /// The frame `follow_bodies` places with one character at the focus.
    fn placed(enabled: bool) -> TouchFrame {
        let feet = Vec3::new(1.0, 2.0, 3.0);
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<TouchFrame>()
            .insert_resource(TreeTouch { enabled })
            .insert_resource(VegetationLodFocus {
                position: Some(feet),
            })
            .insert_resource(VegetationBenders {
                benders: vec![VegetationBender {
                    position: feet,
                    velocity: Vec2::ZERO,
                    radius: 0.7,
                }],
                delta_seconds: 0.0,
            })
            .add_systems(Update, follow_bodies);
        app.update();
        app.world().resource::<TouchFrame>().clone()
    }

    #[test]
    fn switched_off_no_field_is_placed_and_no_body_pushes() {
        let on = placed(true);
        assert_eq!(on.window.centre[3], VOXEL_METRES);
        assert_eq!(on.bodies.len(), 1);

        let off = placed(false);
        assert_eq!(off.window, TouchWindow::default());
        assert!(off.bodies.is_empty());
    }
}
