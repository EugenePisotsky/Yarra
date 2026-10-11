//! The production road drawing (`shaders/terrain_road.wesl`) on a native GPU, from records packed
//! as near terrain packs them, against `world::TerrainRoads`, which the compiler lays roads with.
use super::*;
use bevy::render::{
    render_resource::{
        BindGroupDescriptor, BindGroupEntry, BufferDescriptor, BufferInitDescriptor, BufferUsages,
        CommandEncoderDescriptor, ComputePassDescriptor, MapMode, PollType,
        RawComputePipelineDescriptor, ShaderModuleDescriptor, ShaderSource,
    },
    renderer::initialize_renderer,
    settings::{Backends, WgpuSettings},
};
use world::{RoadShape, TerrainRoad, TerrainRoadSegment, TerrainRoads};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Case {
    local: [f32; 2],
    world: [f32; 2],
    roads: [i32; 4],
}

/// A bend of the island's cart track through a 32 m page, its centre line every 1.5 m.
fn bend() -> TerrainRoads {
    let (centre, radius) = ([-10.0_f32, -10.0], 32.0_f32);
    let points: Vec<_> = (0..=26)
        .map(|i| {
            let angle = 0.2 + i as f32 * 1.5 / radius;
            let (sin, cos) = angle.sin_cos();
            (
                [centre[0] + radius * cos, centre[1] + radius * sin],
                [cos, sin],
            )
        })
        .collect();
    TerrainRoads {
        roads: vec![TerrainRoad {
            shape: RoadShape {
                seed: 31,
                track_spacing: 1.7,
                track_width: 1.1,
                edge_softness: 0.25,
                edge_variation: 0.05,
                edge_patch_size: 2.0,
                breakup: 0.3,
                breakup_patch_size: 2.4,
            },
            center_ground: 0.1,
            shoulder_ground: 0.35,
            track_depth: 0.0,
            rut_roundness: 1.0,
            strength: 1.0,
            surfaces: vec![(TerrainSurfaceId([1; 16]), 0.75)],
        }],
        segments: points
            .windows(2)
            .enumerate()
            .map(|(i, pair)| TerrainRoadSegment {
                a: pair[0].0,
                b: pair[1].0,
                normal_a: pair[0].1,
                normal_b: pair[1].1,
                // A gentle taper, as knots of different widths give.
                width: [3.6 + i as f32 * 0.01, 3.61 + i as f32 * 0.01],
                road: 0,
            })
            .collect(),
    }
}

#[test]
#[ignore = "requires native GPU; checks the near road drawing against the compiler's"]
fn near_roads_draw_where_the_compiler_lays_them() {
    let cell = CellCoord { x: 85, z: 70 };
    let mut source = super::tests::source(cell);
    source.cell_size = 32.;
    source.roads = bend();
    let (records, roads) = gpu::road_records(&source, 3 * gpu::ROAD_CAPACITY).unwrap();
    let origin = [cell.x as f64 * 32., cell.z as f64 * 32.];
    let mut cases = vec![];
    for z in 0..160 {
        for x in 0..160 {
            let local = [x as f32 * 0.2 + 0.1, z as f32 * 0.2 + 0.1];
            cases.push(Case {
                local,
                world: [
                    (origin[0] + f64::from(local[0])) as f32,
                    (origin[1] + f64::from(local[1])) as f32,
                ],
                roads: roads.to_array(),
            });
        }
    }
    // The page's records sit at their slot's region of the buffer.
    let mut buffer = vec![Vec4::ZERO; 4 * gpu::ROAD_CAPACITY as usize];
    let base = 3 * gpu::ROAD_CAPACITY as usize;
    buffer[base..base + records.len()].copy_from_slice(&records);
    let results = run(&cases, &buffer);
    let (mut drawn, mut largest) = (0, 0_f64);
    for (case, result) in cases.iter().zip(&results) {
        let local = case.local.map(f64::from);
        let hit = source.roads.nearest(local).unwrap();
        let reference = source.roads.ground(origin, local).unwrap().1;
        let shape = source.roads.roads[0].shape;
        let reach = hit.offset.width * 0.5
            + f64::from(shape.edge_variation)
            + f64::from(shape.edge_softness);
        if result[3] < 0. {
            assert!(
                hit.offset.distance > reach,
                "{local:?}: no road found {} m from the centre line",
                hit.offset.distance
            );
            continue;
        }
        let error = (f64::from(result[0]) - reference).abs();
        largest = largest.max(error);
        assert!(
            error < 2e-3,
            "{local:?}: the shader draws {} where the compiler lays {reference}",
            result[0]
        );
        assert!((f64::from(result[1]) - hit.offset.lateral).abs() < 1e-3);
        drawn += usize::from(reference > 0.5);
    }
    assert!(drawn > 1000, "the road drew {drawn} samples");
    println!("largest difference {largest:e} over {drawn} road samples");
}

fn run(cases: &[Case], roads: &[Vec4]) -> Vec<[f32; 4]> {
    let defs = shader_check::Def::parse_list("MATERIAL_BIND_GROUP=0").unwrap();
    let shader = shader_check::Shaders::get()
        .compose_source(
            r#"
import package::shaders::terrain_road::{nearest_road, road_amount};
struct Case { local: vec2<f32>, world: vec2<f32>, roads: vec4<i32> }
@group(0) @binding(0) var<storage, read> cases: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<vec4<f32>>;
@compute @workgroup_size(64)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&cases) { return; }
    let c = cases[id.x];
    let hit = nearest_road(c.roads, c.local, 32.0);
    if hit.road < 0 { results[id.x] = vec4(-1.0); return; }
    results[id.x] = vec4(road_amount(hit, c.world), hit.lateral, hit.distance, f32(hit.road));
}
"#,
            &defs,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = resources.0.wgpu_device();
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("near roads"),
        source: ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("evaluate"),
        compilation_options: default(),
        cache: None,
    });
    let storage = |contents: &[u8]| {
        resources.0.create_buffer_with_data(&BufferInitDescriptor {
            label: None,
            contents,
            usage: BufferUsages::STORAGE,
        })
    };
    let input = storage(bytemuck::cast_slice(cases));
    let road_buffer = storage(&crate::storage_bytes(&roads.to_vec()));
    let bytes = cases.len() as u64 * 16;
    let output = resources.0.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = resources.0.create_buffer(&BufferDescriptor {
        label: None,
        size: bytes,
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 16,
                resource: road_buffer.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((cases.len() as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
    resources.1.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
    device.poll(PollType::wait_indefinitely()).unwrap();
    receiver.recv().unwrap().unwrap();
    let data = readback
        .slice(..)
        .get_mapped_range()
        .expect("readback is mapped");
    bytemuck::cast_slice(&data).to_vec()
}

#[test]
fn road_tiles_list_every_segment_that_reaches_them_and_records_fit_their_region() {
    let mut source = super::tests::source(CellCoord::ZERO);
    source.cell_size = 32.;
    // Half-metre weights, as the island's: the drawing reaches 2.85 m from a 3.6 m road's line.
    source.weights[0].resolution = 65;
    source.roads = bend();
    let mut second = source.roads.roads[0].clone();
    second.shape.seed = 7;
    source.roads.roads.push(second);
    for s in &mut source.roads.segments[20..] {
        s.road = 1;
    }
    let base = 5 * gpu::ROAD_CAPACITY;
    let (records, roads) = gpu::road_records(&source, base).unwrap();
    let first_segment = base as usize + 12 + 256;
    assert_eq!(roads, IVec4::new(base as i32 + 12, 16, 0, 0));
    assert_eq!(records.len(), 12 + 256 + 26 * 3);
    // The seed survives as two exact halves; the ground's surface is the page's slot 1.
    assert_eq!(records[7].zw(), Vec2::new(7., 0.));
    assert_eq!(records[4], Vec4::new(1., -1., -1., -1.));
    // Each segment names its road's record.
    let segment = |i: usize| records[first_segment - base as usize + i * 3 + 2];
    assert_eq!(segment(19).z, base as f32);
    assert_eq!(segment(20).z, (base + 6) as f32);
    let runs = |x: usize, z: usize| records[12 + z * 16 + x];
    let listed = |x: usize, z: usize, i: usize| {
        let r = runs(x, z);
        let index = (first_segment + i * 3) as f32;
        [r.xy(), r.zw()]
            .iter()
            .any(|r| index >= r.x && index < r.x + r.y * 3.)
    };
    for z in 0..16 {
        for x in 0..16 {
            let lo = Vec2::new(x as f32, z as f32) * 2.;
            for (i, s) in source.roads.segments.iter().enumerate() {
                // Any point of the tile within the drawing's reach of the segment.
                let (a, b) = (Vec2::from_array(s.a), Vec2::from_array(s.b));
                let reaches = (0..=4).any(|u| {
                    (0..=4).any(|v| {
                        let p = lo + Vec2::new(u as f32, v as f32) * 0.5;
                        let t = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0., 1.);
                        p.distance(a + (b - a) * t) < 2.85
                    })
                });
                assert!(
                    !reaches || listed(x, z, i),
                    "tile ({x}, {z}) misses segment {i}"
                );
            }
        }
    }
    assert_eq!(
        runs(15, 15),
        Vec4::ZERO,
        "the far corner is clear of the bend"
    );
    // A tile reached by three runs keeps two, merging the closest.
    let mut scattered = source.clone();
    scattered.roads.segments = (0..10)
        .map(|i| {
            let x = if [0, 1, 5, 9].contains(&i) { 2. } else { 26. };
            TerrainRoadSegment {
                a: [x, 1.],
                b: [x, 2.],
                ..source.roads.segments[0]
            }
        })
        .collect();
    let (records, _) = gpu::road_records(&scattered, base).unwrap();
    let first = (base as usize + 12 + 256) as f32;
    assert_eq!(records[12], Vec4::new(first, 6., first + 27., 1.));
    // Pages whose roads do not fit draw them as their weights carry them.
    source.roads.segments = source.roads.segments.repeat(10);
    assert!(gpu::road_records(&source, base).is_none());
}
