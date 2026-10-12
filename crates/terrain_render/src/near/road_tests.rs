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
use world::{RoadShape, TerrainRoad, TerrainRoadJunction, TerrainRoadSegment, TerrainRoads};

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
                track_wander: 0.25,
                track_wander_length: 7.0,
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
        junctions: vec![],
    }
}

/// A cart road along the page and a footpath leaving it at a junction, 40 degrees off.
fn fork() -> TerrainRoads {
    let line = |from: [f32; 2], to: [f32; 2], road: u16, width: f32| {
        let d = [to[0] - from[0], to[1] - from[1]];
        let length = d[0].hypot(d[1]);
        let normal = [-d[1] / length, d[0] / length];
        let steps = (length / 1.5).ceil() as usize;
        (0..steps)
            .map(move |i| {
                let at = |t: f32| [from[0] + d[0] * t, from[1] + d[1] * t];
                TerrainRoadSegment {
                    a: at(i as f32 / steps as f32),
                    b: at((i + 1) as f32 / steps as f32),
                    normal_a: normal,
                    normal_b: normal,
                    width: [width; 2],
                    road,
                }
            })
            .collect::<Vec<_>>()
    };
    let mut roads = bend();
    let mut path = roads.roads[0].clone();
    path.shape.seed = 9;
    path.shape.track_spacing = 0.0;
    path.shape.track_width = 0.6;
    path.shape.edge_softness = 0.1;
    path.shape.track_wander = 0.1;
    roads.roads.push(path);
    roads.segments = line([-5.0, 16.0], [40.0, 16.0], 0, 3.6);
    roads.segments.extend(line(
        [16.0, 16.0],
        [16.0 + 24.0 * 0.766, 16.0 + 24.0 * 0.643],
        1,
        1.6,
    ));
    roads.junctions = vec![TerrainRoadJunction {
        centre: [16.0, 16.0],
        radius: 4.0,
        seed: 99,
        road: 0,
        members: 0b11,
    }];
    roads
}

/// Draws `roads` on a 32 m page with the production shader and checks every layer against
/// `world::TerrainRoads::layers`, which the compiler lays roads with.
/// Returns how many samples each layer (two roads, a junction) covers more than half.
fn draws_as_compiled(roads: TerrainRoads) -> [usize; 3] {
    let cell = CellCoord { x: 85, z: 70 };
    let mut source = super::tests::source(cell);
    source.cell_size = 32.;
    source.weights[0].resolution = 65;
    source.roads = roads;
    let base = 3 * gpu::ROAD_CAPACITY;
    let (records, entry) = gpu::road_records(&source, base).unwrap();
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
                roads: entry.to_array(),
            });
        }
    }
    // The page's records sit at their slot's region of the buffer.
    let mut buffer = vec![Vec4::ZERO; 4 * gpu::ROAD_CAPACITY as usize];
    buffer[base as usize..base as usize + records.len()].copy_from_slice(&records);
    let results = run(&cases, &buffer, base);
    let (mut drawn, mut largest) = ([0; 3], 0_f64);
    for (case, result) in cases.iter().zip(&results) {
        let local = case.local.map(f64::from);
        let reference = source.roads.layers(origin, local);
        let mut expected = [0.0; 3];
        expected[..reference.len()].copy_from_slice(&reference);
        for (layer, &want) in expected.iter().enumerate() {
            let error = (f64::from(result[layer]) - want).abs();
            largest = largest.max(error);
            assert!(
                error < 2e-3,
                "{local:?}: layer {layer} draws {} where the compiler lays {want}",
                result[layer]
            );
        }
        if result[3] > -1e8 {
            let hit = source.roads.nearest(local).unwrap();
            assert!((f64::from(result[3]) - hit.offset.lateral).abs() < 1e-3);
        }
        for (count, &value) in drawn.iter_mut().zip(&expected) {
            *count += usize::from(value > 0.5);
        }
    }
    println!("largest difference {largest:e}; samples over half per layer {drawn:?}");
    drawn
}

#[test]
#[ignore = "requires native GPU; checks the near road drawing against the compiler's"]
fn near_roads_draw_where_the_compiler_lays_them() {
    assert!(draws_as_compiled(bend())[0] > 1000);
}

#[test]
#[ignore = "requires native GPU; checks roads meeting at a junction against the compiler's"]
fn near_roads_branch_as_the_compiler_lays_them() {
    let [road, path, junction] = draws_as_compiled(fork());
    assert!(
        road > 1000 && path > 100 && junction > 100,
        "{road} {path} {junction}"
    );
}

fn run(cases: &[Case], roads: &[Vec4], base: u32) -> Vec<[f32; 4]> {
    let defs = shader_check::Def::parse_list("MATERIAL_BIND_GROUP=0").unwrap();
    let shader = shader_check::Shaders::get()
        .compose_source(
            &r#"
import package::shaders::terrain_road::{RoadHit, nearest_roads, road_amount, road_reach, junction_ground, junctions_over};
struct Case { local: vec2<f32>, world: vec2<f32>, roads: vec4<i32> }
@group(0) @binding(0) var<storage, read> cases: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<vec4<f32>>;
// Per case: the amount each of the page's first two roads lays, the first junction's, and the
// nearest road's lateral offset (-1e9: no road within reach), as near terrain lays them.
fn laid(c: Case, hit: RoadHit) -> f32 {
    if hit.road < 0 || hit.distance >= road_reach(hit, 0.75) { return 0.0; }
    let centre = c.world - c.local + hit.point;
    return road_amount(hit, c.world, centre) * (1.0 - junctions_over(c.roads, hit, c.local));
}
@compute @workgroup_size(64)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&cases) { return; }
    let c = cases[id.x];
    let pair = nearest_roads(c.roads, c.local, 32.0);
    var out = vec4(0.0, 0.0, 0.0, -1.0e9);
    let base = BASE;
    if pair.first.road >= 0 { out[(u32(pair.first.road) - base)/6u] = laid(c, pair.first); }
    if pair.second.road >= 0 { out[(u32(pair.second.road) - base)/6u] = laid(c, pair.second); }
    if c.roads.w > 0 { out.z = junction_ground(u32(c.roads.z), c.local, c.world); }
    if pair.first.road >= 0 && pair.first.distance < road_reach(pair.first, 0.75) { out.w = pair.first.lateral; }
    results[id.x] = out;
}
"#
            .replace("BASE", &format!("{base}u")),
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
    assert_eq!(roads, IVec4::new(base as i32 + 12, 16, base as i32 + 12, 0));
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
            .any(|r| index >= r.x && index < r.x + r.y.abs() * 3.)
    };
    // Tiles reached by both roads say so; tiles of one road do not.
    let both = (0..256).filter(|&i| records[12 + i].y < 0.).count();
    assert!(both > 0 && both < 20, "{both} tiles hold both roads");
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
