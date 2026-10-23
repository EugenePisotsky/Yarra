//! The layout's bands, and the compute pass (`shaders/tree/lod.wesl`) run on a native GPU
//! against the CPU's timed LOD (`trees::timed`).
use super::super::timed::TimedLod;
use super::layout::bands;
use super::*;
use crate::object_lod::{ForcedLod, LodProjection};

use bevy::render::{
    render_resource::{
        BindGroupDescriptor, BindGroupEntry, Buffer, BufferDescriptor, BufferInitDescriptor,
        BufferUsages, CommandEncoderDescriptor, ComputePassDescriptor, MapMode, PollType,
        RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
    },
    renderer::{RenderDevice, RenderQueue, initialize_renderer},
    settings::{Backends, WgpuSettings},
};

use bevy::shape::ViewFrustum;

/// A spruce's chain: LOD0, LOD1 and LOD2 (too close to LOD1 for a band), then the impostor.
const SPRUCE: [f32; 4] = [480.0, 240.0, 200.0, 0.0];

const SPRUCE_HEIGHT: f32 = 14.05;

/// A fullscreen MacBook window: 1084 logical pixels high, 45 degrees.
const PIXELS_PER_METRE: f32 = 1308.5;

#[test]
fn bands_skip_crowded_lods_and_are_not_limited_by_residency() {
    let projection = LodProjection::perspective(PIXELS_PER_METRE, 2.0);
    let (start, end) = bands(&projection, &SPRUCE, SPRUCE_HEIGHT);
    let switch = |threshold: f32| SPRUCE_HEIGHT * PIXELS_PER_METRE / threshold;
    assert_eq!(start[0], 0.0);
    assert!((end[0] - switch(480.0)).abs() < 1e-3);
    assert_eq!(start[1], end[0]);

    // LOD2 has no band: LOD1 reaches the impostor's switch.
    assert_eq!((start[2], end[2]), (-1.0, -1.0));
    assert!((end[1] - switch(200.0)).abs() < 1e-3);
    assert_eq!((start[3], end[3]), (end[1], f32::MAX));

    // A tall tree's meshes reach past the cells' 166 m hand-off.
    let (start, _) = bands(&projection, &SPRUCE, 30.0);
    assert!(start[3] > 190.0);
}

#[test]
fn impostors_take_over_at_a_texel_per_output_pixel() {
    // Retina: two output pixels per logical pixel. The spruce's views hold 256 texels
    // across 16.78 m.
    let projection = LodProjection::perspective(PIXELS_PER_METRE, 2.0);
    let texels_per_metre = 256.0 / 16.78;
    let spruce = super::super::impostor::TreeFormSpec {
        impostor: "spruce.impostor.json".into(),
        height: SPRUCE_HEIGHT,
        meshes: vec![
            ("lod0".into(), 480.0),
            ("lod1".into(), 240.0),
            ("lod2".into(), 200.0),
        ],
    };
    let handoff = projection.impostor_threshold(SPRUCE_HEIGHT, texels_per_metre);
    let thresholds = spruce.thresholds(handoff);
    assert_eq!(thresholds[..2], [480.0, 240.0]);
    assert_eq!(thresholds[3], 0.0);
    let (start, end) = bands(&projection, &thresholds, SPRUCE_HEIGHT);

    // At the switch, a metre of the spruce covers as many output pixels as its views hold
    // texels; LOD2 draws from LOD1's switch to there.
    let output_pixels_per_metre = |d: f32| PIXELS_PER_METRE * 2.0 / d;
    assert!((output_pixels_per_metre(end[2]) - texels_per_metre).abs() < 1e-3);
    assert!((end[2] - 171.6).abs() < 0.5, "{}", end[2]);
    assert!((start[2] - SPRUCE_HEIGHT * PIXELS_PER_METRE / 240.0).abs() < 1e-3);
    assert_eq!(start[3], end[2]);

    // Twice the texels: the impostor is sharp enough too soon after LOD1's switch for LOD2
    // to fit a band, and LOD1 reaches it.
    let handoff = projection.impostor_threshold(SPRUCE_HEIGHT, 2.0 * texels_per_metre);
    let (start, end) = bands(&projection, &spruce.thresholds(handoff), SPRUCE_HEIGHT);
    assert_eq!((start[2], end[2]), (-1.0, -1.0));
    assert_eq!(start[3], end[1]);
}

#[test]
#[ignore = "requires native GPU access; runs the trees' LOD and fade pass"]
fn gpu_casters_shadowing_nothing_in_view_are_not_drawn() {
    let (form, buckets) = spruce_form(1);
    let harness = Harness::new(&[instance(Vec3::ZERO, None)], form, &buckets);
    let casts = |camera: &GpuCamera| {
        let drawn = harness.frame(camera);
        drawn.views[1].iter().any(|r| !r.is_empty())
    };

    // 30 m ahead under a high sun, its shadow is in view.
    let ahead = Vec3::new(0.0, 7.0, 30.0);
    assert!(casts(&camera(ahead, 0.016, 0, 1, &form, 1)));

    // 30 m behind, its shadow falls behind too.
    let behind = Vec3::new(0.0, 7.0, -30.0);
    assert!(!casts(&camera(behind, 0.016, 0, 1, &form, 1)));

    // 45 m ahead, past the 20 m this cascade covers: a high sun keeps its shadow out there...
    let far = Vec3::new(0.0, 7.0, 45.0);
    let mut high = camera(far, 0.016, 0, 1, &form, 1);
    receive(&mut high, far, 0.0..20.0, Vec3::NEG_Y);
    assert!(!casts(&high));

    // ...and a low sun behind it, shining toward the camera, draws it into the cascade's depths.
    let mut low = high;
    receive(&mut low, far, 0.0..20.0, Vec3::new(0.0, -0.25, 1.0));
    assert!(casts(&low));

    // A cascade's depths start where the one before blends into it: nearer is not its own.
    let mut later = camera(ahead, 0.016, 0, 1, &form, 1);
    receive(&mut later, ahead, 60.0..120.0, Vec3::NEG_Y);
    assert!(!casts(&later));
}

/// The compute pass's buffers for a handful of instances of one form.
struct Harness {
    device: RenderDevice,
    queue: RenderQueue,
    lod: bevy::render::render_resource::ComputePipeline,
    finalize: bevy::render::render_resource::ComputePipeline,
    camera: Buffer,
    instances: Buffer,
    origins: Buffer,
    forms: Buffer,
    states: Buffer,
    counters: Buffer,
    records: Buffer,
    entries: Buffer,
    args: Buffer,
    frames: Buffer,
    /// The wind's two poses (`trees::wind::WindBuffer`), calm.
    wind: Buffer,
    form: GpuForm,
    /// Records per bucket.
    capacity: u32,
    count: u32,
}

/// What one frame drew: per view and representation, each instance with its dither level.
#[derive(Debug, Default, PartialEq)]
struct Drawn {
    views: Vec<Vec<Vec<(u32, i32)>>>,
    counts: Vec<u32>,
}

impl Harness {
    fn new(instances: &[GpuInstance], form: GpuForm, buckets: &[GpuBucket]) -> Self {
        let resources = bevy::tasks::block_on(initialize_renderer(
            Backends::PRIMARY,
            None,
            &WgpuSettings::default(),
        ));
        let (device, queue) = (resources.0, resources.1);
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/shaders/tree/lod.wesl"
        ))
        .unwrap();
        let shader = shader_check::Shaders::get()
            .compose_source(&source, &[])
            .unwrap_or_else(|error| panic!("{error}"));
        let module = device
            .wgpu_device()
            .create_shader_module(ShaderModuleDescriptor {
                label: Some("tree LOD"),
                source: ShaderSource::Wgsl(shader.into()),
            });
        let pipeline = |entry| {
            device
                .wgpu_device()
                .create_compute_pipeline(&RawComputePipelineDescriptor {
                    label: None,
                    layout: None,
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: default(),
                    cache: None,
                })
        };
        let storage = |label, bytes: u64| {
            device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size: bytes.max(16),
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let with = |contents: &[u8], usage| {
            device.create_buffer_with_data(&BufferInitDescriptor {
                label: None,
                contents,
                usage: usage | BufferUsages::COPY_DST,
            })
        };
        let representations = form.representations;
        let capacity = buckets[0].capacity;
        Self {
            lod: pipeline("lod").into(),
            finalize: pipeline("finalize").into(),
            camera: with(
                &[0u8; std::mem::size_of::<GpuCamera>()],
                BufferUsages::UNIFORM,
            ),
            instances: with(bytemuck::cast_slice(instances), BufferUsages::STORAGE),
            origins: with(
                bytemuck::cast_slice(&[[0.0f32, 0.0, 0.0, 1.0]]),
                BufferUsages::STORAGE,
            ),
            forms: with(bytemuck::bytes_of(&form), BufferUsages::STORAGE),
            states: storage("states", instances.len() as u64 * 4),
            counters: storage("counters", (VIEWS as u64) * u64::from(representations) * 4),
            records: storage(
                "records",
                VIEWS as u64 * u64::from(representations * capacity) * 4,
            ),
            entries: with(bytemuck::cast_slice(buckets), BufferUsages::STORAGE),
            args: storage("args", VIEWS as u64 * u64::from(representations) * 20),
            frames: storage("frames", instances.len() as u64 * 64),
            wind: storage("wind", 2 * 7 * 16),
            form,
            capacity,
            count: instances.len() as u32,
            device,
            queue,
        }
    }

    fn read(&self, buffer: &Buffer) -> Vec<u32> {
        let size = buffer.size();
        let readback = self.device.create_buffer(&BufferDescriptor {
            label: None,
            size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(buffer, 0, &readback, 0, size);
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
        self.device
            .wgpu_device()
            .poll(PollType::wait_indefinitely())
            .unwrap();
        receiver.recv().unwrap().unwrap();
        let data = readback.slice(..).get_mapped_range().expect("mapped");
        bytemuck::cast_slice(&data).to_vec()
    }

    /// Runs one frame and returns what each view drew.
    fn frame(&self, camera: &GpuCamera) -> Drawn {
        self.queue
            .write_buffer(&self.camera, 0, bytemuck::bytes_of(camera));
        let group = |pipeline: &bevy::render::render_resource::ComputePipeline,
                     bindings: &[(u32, &Buffer)]| {
            self.device
                .wgpu_device()
                .create_bind_group(&BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &bindings
                        .iter()
                        .map(|(binding, buffer)| BindGroupEntry {
                            binding: *binding,
                            resource: buffer.as_entire_binding(),
                        })
                        .collect::<Vec<_>>(),
                })
        };
        let lod = group(
            &self.lod,
            &[
                (0, &self.camera),
                (1, &self.instances),
                (2, &self.origins),
                (3, &self.forms),
                (4, &self.states),
                (5, &self.counters),
                (6, &self.records),
                (7, &self.entries),
                (9, &self.frames),
                (10, &self.wind),
            ],
        );
        let finalize = group(
            &self.finalize,
            &[
                (0, &self.camera),
                (5, &self.counters),
                (7, &self.entries),
                (8, &self.args),
            ],
        );
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        encoder.clear_buffer(&self.counters, 0, None);

        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
            pass.set_pipeline(&self.lod);
            pass.set_bind_group(0, &lod, &[]);
            pass.dispatch_workgroups(self.count.div_ceil(64), 1, 1);
            pass.set_pipeline(&self.finalize);
            pass.set_bind_group(0, &finalize, &[]);
            pass.dispatch_workgroups(
                (self.form.representations * VIEWS as u32).div_ceil(64),
                1,
                1,
            );
        }

        self.queue.submit([encoder.finish()]);
        let counters = self.read(&self.counters);
        let records = self.read(&self.records);
        let args = self.read(&self.args);
        let reps = self.form.representations as usize;
        let capacity = self.capacity as usize;
        let per_view = reps * capacity;
        let views = (0..VIEWS)
            .map(|view| {
                (0..reps)
                    .map(|r| {
                        let count = (counters[view * reps + r] as usize).min(capacity);
                        let base = view * per_view + r * capacity;
                        let mut drawn: Vec<(u32, i32)> = records[base..base + count]
                            .iter()
                            .map(|&record| (record & 0xff_ffff, (record >> 24) as i32 - 16))
                            .collect();
                        drawn.sort_unstable();
                        drawn
                    })
                    .collect()
            })
            .collect();
        Drawn {
            views,
            counts: (0..VIEWS * reps).map(|e| args[e * 5 + 1]).collect(),
        }
    }
}

fn instance(position: Vec3, forced: Option<u32>) -> GpuInstance {
    GpuInstance {
        position_yaw: position.extend(0.0).to_array(),
        scale: 1.0,
        form: 0,
        origin: 0,
        flags: ALIVE | forced.map_or(0, |r| r + 1),
    }
}

/// A camera at `eye` looking along -Z, and one orthographic shadow view 40 m wide round
/// the origin, lit from straight above, which every depth up to 1 km samples.
fn camera(
    eye: Vec3,
    dt: f32,
    shadow_lod: u32,
    count: u32,
    form: &GpuForm,
    capacity: u32,
) -> GpuCamera {
    let view = Transform::from_translation(eye).looking_to(Vec3::NEG_Z, Vec3::Y);
    let clip = glam::camera::rh::proj::directx::perspective_infinite_reverse(
        std::f32::consts::FRAC_PI_4,
        1.6,
        0.1,
    ) * view.to_matrix().inverse();
    let planes = |frustum: ViewFrustum| -> [[f32; 4]; 6] {
        std::array::from_fn(|i| frustum.half_spaces[i].normal_d().to_array())
    };
    let shadow =
        glam::camera::rh::proj::directx::orthographic(-20.0, 20.0, -20.0, 20.0, -100.0, 100.0)
            * Transform::from_xyz(0.0, 50.0, 0.0)
                .looking_at(Vec3::ZERO, Vec3::Z)
                .to_matrix()
                .inverse();
    let mut shadow_planes = [[0.0; 4]; 6 * MAX_SHADOW_VIEWS];
    shadow_planes[..6].copy_from_slice(&planes(ViewFrustum::from_clip_from_world(&shadow)));
    let mut camera = GpuCamera {
        eye_dt: eye.extend(dt).to_array(),
        planes: planes(ViewFrustum::from_clip_from_world(&clip)),
        shadow_planes,
        shadow_receivers: [[0.0; 4]; 3 * MAX_SHADOW_VIEWS],
        counts: [count, 1, shadow_lod, form.representations * capacity],
        sizes: [form.representations, 0, 0, 0],
        wind: [0.0; 4],
    };
    receive(&mut camera, eye, 0.0..1000.0, Vec3::NEG_Y);
    camera
}

/// Makes the depths `depths` from the camera at `eye` (looking along -Z) sample the shadow
/// view, its light travelling along `sweep`.
fn receive(camera: &mut GpuCamera, eye: Vec3, depths: std::ops::Range<f32>, sweep: Vec3) {
    let depth = Vec3::NEG_Z.dot(eye);
    camera.shadow_receivers[0] = Vec3::NEG_Z.extend(-depth - depths.start).to_array();
    camera.shadow_receivers[1] = Vec3::Z.extend(depth + depths.end).to_array();
    camera.shadow_receivers[2] = sweep.normalize().extend(0.0).to_array();
}

/// A spruce's form, one primitive per LOD (bucket n for representation n).
fn spruce_form(capacity: u32) -> (GpuForm, Vec<GpuBucket>) {
    let (start, end) = bands(
        &LodProjection::perspective(PIXELS_PER_METRE, 2.0),
        &SPRUCE,
        SPRUCE_HEIGHT,
    );
    let mut form = GpuForm {
        height: SPRUCE_HEIGHT,
        representations: 4,
        meshes: 3,
        ready: 1,
        ..default()
    };

    for r in 0..MAX_REPRESENTATIONS {
        form.band_start[r / 4][r % 4] = start[r];
        form.band_end[r / 4][r % 4] = end[r];
    }

    let mut buckets = Vec::new();

    for r in 0..4 {
        form.first_bucket[0][r] = r as u32;
        form.buckets[0][r] = 1;
        let impostor = r == 3;
        buckets.push(GpuBucket {
            centre: [0.0, 7.0, 0.0, f32::from(u8::from(impostor))],
            extent: if impostor {
                [12.0, 0.0, 0.0, 0.0]
            } else {
                [5.0, 8.0, 5.0, 0.0]
            },
            records: r as u32 * capacity,
            capacity,
            ..default()
        });
    }

    (form, buckets)
}

#[test]
#[ignore = "requires native GPU access; runs the trees' LOD and fade pass"]
fn gpu_fades_match_the_cpu_timed_lod() {
    // One spruce walked through its switches by the camera, frame by frame, as the CPU's
    // timed LOD would draw it; plus a spruce behind the camera and one forced to LOD2.
    let spruce = instance(Vec3::ZERO, None);
    let behind = instance(Vec3::new(0.0, 0.0, 400.0), None);
    let forced = instance(Vec3::new(5.0, 0.0, -30.0), Some(2));
    let (form, buckets) = spruce_form(3);
    let harness = Harness::new(&[spruce, behind, forced], form, &buckets);
    let projection = LodProjection::perspective(PIXELS_PER_METRE, 2.0);
    let mut cpu = TimedLod::default();
    cpu.set_bands(&projection, &SPRUCE, SPRUCE_HEIGHT);

    // Distances from the spruce over time: inside LOD0, across to LOD1 (dissolving over
    // 0.6 s), turning back mid-dissolve, out to the impostor. No dissolve ends exactly on a
    // frame, where the GPU's 16-bit progress and the CPU's could round apart.
    let mut path = vec![(10.0, 0.016)];
    path.extend([(45.0, 0.13); 3]);
    path.push((10.0, 0.05));
    path.extend([(10.0, 0.13); 3]);
    path.extend([(120.0, 0.13); 7]);

    for (step, &(distance, dt)) in path.iter().enumerate() {
        let eye = Vec3::new(0.0, 7.0, distance);
        let drawn = harness.frame(&camera(eye, dt, 0, 3, &form, 3));
        cpu.step(ForcedLod::Auto, Vec3::ZERO.distance(eye), dt);
        let expected: Vec<(usize, i32)> = (0..4)
            .filter_map(|r| cpu.level(r).map(|level| (r, level)))
            .filter(|(_, level)| level.abs() < 16)
            .collect();
        let gpu: Vec<(usize, i32)> = (0..4)
            .flat_map(|r| {
                drawn.views[0][r]
                    .iter()
                    .filter(|(index, _)| *index == 0)
                    .map(move |&(_, level)| (r, level))
            })
            .collect();
        assert_eq!(gpu, expected, "step {step}, {distance} m");

        // The instance behind the camera never draws in the main view.
        assert!(
            drawn.views[0]
                .iter()
                .flatten()
                .all(|(index, _)| *index != 1)
        );

        // The forced spruce draws LOD2 whole wherever the camera is.
        assert!(drawn.views[0][2].contains(&(2, 0)), "step {step}");

        // Each draw's instance count is its bucket's.
        for r in 0..4 {
            assert_eq!(drawn.counts[r] as usize, drawn.views[0][r].len());
        }
    }
}

#[test]
#[ignore = "requires native GPU access; runs the trees' LOD and fade pass"]
fn gpu_trees_beside_the_view_draw_as_far_as_the_wind_moves_them() {
    // A spruce beside the view whose trunk bends from 14 m up and whose limbs reach 3 m: its
    // own bounds and a push miss the frustum, the lean a gale gives it does not.
    let (form, mut buckets) = spruce_form(1);

    for bucket in &mut buckets[..3] {
        bucket.sweep = [14.0, 0.0, 3.0, 3.0];
        bucket.response = [1.0, 1.0, 0.0, 0.0];
    }

    let harness = Harness::new(
        &[instance(Vec3::new(34.0, 0.0, -30.0), Some(0))],
        form,
        &buckets,
    );
    let drawn = |turns: f32| {
        let mut camera = camera(Vec3::new(0.0, 7.0, 0.0), 0.016, 0, 1, &form, 1);
        camera.wind = [turns, turns, 0.0, 0.0];
        harness.frame(&camera).views[0].iter().flatten().count()
    };
    assert_eq!(drawn(0.0), 0);
    assert_eq!(drawn(10.0), 1);
}

#[test]
#[ignore = "requires native GPU access; runs the trees' LOD and fade pass"]
fn gpu_trees_frames_hold_the_wind_strength_at_each_tree() {
    // The two poses' strengths, without gusts to swell them: `WindPose`'s field.w is its
    // fourth float, the response's swell its twelfth, and the previous pose follows seven
    // vec4s on.
    let (form, buckets) = spruce_form(1);
    let harness = Harness::new(
        &[instance(Vec3::new(3.0, 0.0, -30.0), None)],
        form,
        &buckets,
    );
    let mut wind = [0.0f32; 56];
    wind[3] = 1.3;
    wind[28 + 3] = 0.9;
    harness
        .queue
        .write_buffer(&harness.wind, 0, bytemuck::cast_slice(&wind));
    harness.frame(&camera(Vec3::new(0.0, 7.0, 0.0), 0.016, 0, 1, &form, 1));
    let frame: Vec<f32> = harness
        .read(&harness.frames)
        .into_iter()
        .map(f32::from_bits)
        .collect();
    assert_eq!(frame[12..14], [1.3, 0.9]);
}

#[test]
#[ignore = "requires native GPU access; runs the trees' LOD and fade pass"]
fn gpu_shadows_cast_from_coarser_lods_and_merge_mid_fade() {
    let (form, buckets) = spruce_form(1);
    let harness = Harness::new(&[instance(Vec3::ZERO, None)], form, &buckets);

    // Shown at LOD0, it casts LOD1's shadow with a bias of one.
    let near = Vec3::new(0.0, 7.0, 10.0);
    let drawn = harness.frame(&camera(near, 0.016, 1, 1, &form, 1));
    assert_eq!(drawn.views[0][0], [(0, 0)]);
    assert_eq!(drawn.views[1][1], [(0, 0)]);
    assert!(drawn.views[1][0].is_empty());

    // Dissolving from LOD0 into LOD1, each casts from the next LOD, with its level.
    let middle = Vec3::new(0.0, 7.0, 45.0);
    harness.frame(&camera(middle, 0.016, 1, 1, &form, 1));
    let drawn = harness.frame(&camera(middle, 0.3, 1, 1, &form, 1));
    assert_eq!(drawn.views[0][0], [(0, 8)]);
    assert_eq!(drawn.views[0][1], [(0, -8)]);
    assert_eq!(drawn.views[1][1], [(0, 8)]);
    assert_eq!(drawn.views[1][2], [(0, -8)]);

    // Two LODs on, both cast from LOD2 (the last mesh): whole, mid-dissolve.
    let drawn = harness.frame(&camera(middle, 0.0, 2, 1, &form, 1));
    assert_eq!(drawn.views[1][2], [(0, 0)]);
    assert!(drawn.views[1][0].is_empty() && drawn.views[1][1].is_empty());

    // Impostors cast nothing.
    let far = Vec3::new(0.0, 7.0, 300.0);

    for _ in 0..10 {
        harness.frame(&camera(far, 0.1, 0, 1, &form, 1));
    }

    let drawn = harness.frame(&camera(far, 0.1, 0, 1, &form, 1));
    assert_eq!(drawn.views[0][3], [(0, 0)]);
    assert!(drawn.views[1].iter().all(Vec::is_empty));
}
