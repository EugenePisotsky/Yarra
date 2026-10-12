//! Worn ground in near terrain (`shaders/terrain_wear.wesl`) on a native GPU against
//! `world::TerrainWear`, which the cook and the distant bake lay it with, and how pages pack
//! their wear for it.
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
use world::{NO_WORN_SLOT, TerrainWear};

/// Page slots 0-2 natural (meadow, forest floor, dirt), 3-5 what they wear to; the meadow is
/// trampled to slot 5 (dry grass) and the forest floor to slot 3.
fn trampled() -> Vec<u8> {
    vec![5, 3, NO_WORN_SLOT, NO_WORN_SLOT, NO_WORN_SLOT, NO_WORN_SLOT]
}
fn worn() -> Vec<[(u8, u8); 2]> {
    let none = (NO_WORN_SLOT, 0);
    vec![
        [(2, 153), (3, 102)],
        [(4, 255), none],
        [(5, 128), (3, 127)],
        [none, none],
        [none, none],
        [none, none],
    ]
}

#[test]
#[ignore = "requires native GPU; checks near terrain's worn ground against the cook's"]
fn near_terrain_wears_the_ground_as_the_cook_lays_it() {
    let wear = TerrainWear {
        resolution: 2,
        texels: vec![[0; 4]; 4],
        worn: worn(),
        trampled: trampled(),
    };
    let mut cases = vec![];
    for i in 0..64 {
        let mut weights = [0.0_f32; 8];
        let a = (i % 4) as f32 / 4.0;
        weights[0] = 1.0 - a;
        weights[1] = a * 0.6;
        weights[2] = a * 0.4;
        cases.push((weights, i as f32 / 63.0, ((i * 7) % 11) as f32 / 10.0));
    }
    let slots: Vec<[f32; 4]> = (0..8)
        .map(|i| {
            let pair = wear.worn.get(i).copied().unwrap_or([(NO_WORN_SLOT, 0); 2]);
            let to = |(s, share): (u8, u8)| {
                if s == NO_WORN_SLOT {
                    [-1.0, 0.0]
                } else {
                    [f32::from(s), f32::from(share) / 255.0]
                }
            };
            let (a, b) = (to(pair[0]), to(pair[1]));
            [a[0], a[1], b[0], b[1]]
        })
        .collect();
    let trampled: Vec<f32> = (0..8)
        .map(|i| match wear.trampled.get(i) {
            Some(&t) if t != NO_WORN_SLOT => f32::from(t),
            _ => -1.0,
        })
        .collect();
    let results = run(&cases, &slots, &trampled, wear.worn.len() as u32);
    for ((weights, w, bare), result) in cases.iter().zip(&results) {
        let mut expected: Vec<f64> = weights[..6].iter().map(|&v| f64::from(v)).collect();
        wear.lay(&mut expected, f64::from(*w), f64::from(*bare));
        for (slot, want) in expected.iter().enumerate() {
            assert!(
                (f64::from(result[slot]) - want).abs() < 1e-5,
                "wear {w}: slot {slot} lays {} where the cook lays {want}",
                result[slot]
            );
        }
        assert!((result.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }
}

#[test]
fn pages_pack_their_wear_and_what_their_surfaces_wear_to() {
    let mut s = super::tests::source(CellCoord::ZERO);
    let n = usize::from(world::WEAR_RESOLUTION);
    let worn: Vec<_> = s
        .surfaces
        .iter()
        .enumerate()
        .map(|(i, _)| {
            if i == 0 {
                [(1, 255), (NO_WORN_SLOT, 0)]
            } else {
                [(NO_WORN_SLOT, 0); 2]
            }
        })
        .collect();
    s.wear = Some(TerrainWear {
        resolution: world::WEAR_RESOLUTION,
        texels: vec![[200, 10, 128, 255]; n * n],
        worn,
        trampled: (0..s.surfaces.len())
            .map(|i| if i == 0 { 1 } else { NO_WORN_SLOT })
            .collect(),
    });
    let m = super::tests::material(&s);
    let tile = gpu::tile(3, &s, None);
    assert!(tile.has_wear());
    let entry = gpu::entry(&s, &m, 3, 1., tile.has_wear(), 0);
    assert_eq!(entry.wear.x, 1.);
    assert_eq!(entry.slots[0].worn, Vec4::new(1., 1., -1., 0.));
    assert_eq!(entry.slots[1].worn.x, -1.);
    assert_eq!(entry.slots[0].material.w, 1.);
    assert_eq!(entry.slots[1].material.w, -1.);
    // A field of the wrong size draws none.
    s.wear.as_mut().unwrap().resolution = 64;
    assert!(!gpu::tile(3, &s, None).has_wear());
}

fn run(
    cases: &[([f32; 8], f32, f32)],
    slots: &[[f32; 4]],
    trampled: &[f32],
    count: u32,
) -> Vec<[f32; 8]> {
    let defs = shader_check::Def::parse_list("MATERIAL_BIND_GROUP=0").unwrap();
    let shader = shader_check::Shaders::get()
        .compose_source(
            &r#"
import package::shaders::terrain_wear::lay_worn;
struct Case { weights: array<vec4<f32>, 2>, wear: vec4<f32> }
@group(0) @binding(0) var<storage, read> cases: array<Case>;
@group(0) @binding(1) var<storage, read_write> results: array<array<vec4<f32>, 2>>;
@group(0) @binding(3) var<storage, read> slots: array<vec4<f32>, 8>;
@group(0) @binding(4) var<storage, read> trampled: array<vec4<f32>, 2>;
@compute @workgroup_size(64)
fn evaluate(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&cases) { return; }
    let c = cases[id.x];
    let w = array<f32, 8>(c.weights[0].x, c.weights[0].y, c.weights[0].z, c.weights[0].w,
        c.weights[1].x, c.weights[1].y, c.weights[1].z, c.weights[1].w);
    var worn: array<vec4<f32>, 8>;
    for (var i = 0u; i < 8u; i += 1u) { worn[i] = slots[i]; }
    let t = array<f32, 8>(trampled[0].x, trampled[0].y, trampled[0].z, trampled[0].w,
        trampled[1].x, trampled[1].y, trampled[1].z, trampled[1].w);
    let out = lay_worn(w, worn, t, COUNTu, c.wear.x, c.wear.y);
    results[id.x] = array(vec4(out[0], out[1], out[2], out[3]), vec4(out[4], out[5], out[6], out[7]));
}
"#
            .replace("COUNT", &count.to_string()),
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
        label: Some("near wear"),
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
    let packed: Vec<f32> = cases
        .iter()
        .flat_map(|(w, wear, bare)| w.iter().copied().chain([*wear, *bare, 0., 0.]))
        .collect();
    let input = storage(bytemuck::cast_slice(&packed));
    let mut slot_data: Vec<f32> = slots.iter().flatten().copied().collect();
    slot_data.resize(32, 0.);
    let slot_buffer = storage(bytemuck::cast_slice(&slot_data));
    let trampled_buffer = storage(bytemuck::cast_slice(trampled));
    let bytes = cases.len() as u64 * 32;
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
                binding: 3,
                resource: slot_buffer.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 4,
                resource: trampled_buffer.as_entire_binding(),
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
