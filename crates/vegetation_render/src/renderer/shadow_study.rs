//! Opt-in native fixture capture. No code or allocation in non-test game/editor builds.
use super::*;
use bevy::render::{
    render_resource::*,
    renderer::initialize_renderer,
    settings::{Backends, WgpuSettings},
};
use serde::Deserialize;
use std::{fs, path::PathBuf};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const DRAW: &str = include_str!("../../../../assets/shaders/vegetation_debug_draw.wgsl");
const CANOPY: &str = include_str!("../../../../assets/shaders/grass_canopy.wgsl");
const BLADE: &str = include_str!("../../../../assets/shaders/vegetation_blade.wgsl");

mod ambient;

// Assemble the actual geometry and material functions. Only the Bevy view/shadow bindings
// are replaced: the fixture has no world objects/CSM. New visibility attenuates direct
// light only; it does not compound the production receiver's ambient/body darkening.
fn source() -> String {
    let start = DRAW.find("// Vegetation V2 procedural").unwrap();
    let mut draw = super::tests::preprocess_band_study(&DRAW[start..], VegetationBladeBands::Off);
    let begin = draw.find("fn directional_shadow_visibility(").unwrap();
    let end = draw.find("fn radiance_tint(").unwrap();
    draw.replace_range(
        begin..end,
        "fn directional_shadow_visibility(input: VertexOutput) -> f32 { return 1.0; }\n",
    );
    for (from, to) in [
        (
            "struct VertexOutput {",
            "struct VertexOutput {\n    @location(8) @interpolate(flat) identity: u32,\n    @location(9) companion_t: f32,",
        ),
        (
            "output.world_position = world_position;",
            "output.world_position = world_position;\n    output.identity = 1u + instance_index * 2u;",
        ),
        (
            "output.blade_t = shading_t;",
            "output.blade_t = shading_t;\n    output.companion_t = select(0.0, shading_t, ((vertex_index >> 5u) & 1u) != 0u);",
        ),
        (
            "@fragment\nfn fragment(\n    input: VertexOutput,",
            "fn shade_blade(\n    input: VertexOutput, detail_visibility: f32,",
        ),
        (") -> @location(0) vec4<f32>", ") -> vec4<f32>"),
        ("view_bindings::view.exposure", "fixture.counts.w"),
        ("pbr_lighting::", ""),
        (
            "* shadow_visibility",
            "* shadow_visibility * detail_visibility",
        ),
    ] {
        assert!(
            draw.contains(from),
            "production shader contract changed: {from}"
        );
        draw = draw.replace(from, to);
    }
    format!(
        "{CANOPY}\n{BLADE}\n{draw}\n{}\n{}",
        include_str!("shadow_study/fixture.wgsl"),
        include_str!("shadow_study/pixel_trace.wgsl")
    )
}

#[derive(Deserialize)]
struct Study {
    catalog: vegetation::VegetationCatalog,
    wind: Wind,
    lighting: VegetationLighting,
    exposure_ev100: f32,
}
#[derive(Deserialize)]
struct Wind {
    direction: (f32, f32),
    strength: f32,
    spatial_frequency: f32,
    speed: f32,
    gustiness: f32,
    flutter: f32,
}

fn study() -> Study {
    let path = std::env::var_os("YARRA_SHADOW_STUDY_SOURCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../content/vegetation/distance-01.ron")
        });
    ron::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn instance(x: f32, z: f32, yaw: f32, seed: u32, species: u32) -> ProceduralInstanceGpu {
    let (s, c) = yaw.sin_cos();
    let pack = |v: f32| (v * 32767.0).round() as i16 as u16 as u32;
    ProceduralInstanceGpu {
        root_clump: [x, 0.0, z, f32::from_bits(28000)],
        geometry: [
            pack(s) | (pack(c) << 16),
            species | (32767 << 16),
            0,
            seed | (255 << 24),
        ],
    }
}

fn fixture() -> (Vec<ProceduralInstanceGpu>, Vec<[u32; 4]>) {
    // Root 0: isolated main blade. Roots 1/2: two crossing main blades. Rest: 40
    // pairs, authored distribution 65/25/10; empty space separates all three cases.
    let mut roots = vec![
        instance(-1.55, -0.55, 0.3, 1453, 0),
        instance(-0.85, 0.30, 1.1, 12345, 0),
        instance(-0.4483, 0.5607, -0.5, 12863, 2),
    ];
    for i in 0..40u32 {
        let jitter = |v: u32| ((v * 79 % 101) as f32 / 101.0 - 0.5) * 0.09;
        let x = 0.30 + (i % 8) as f32 * 0.15 + jitter(i + 1);
        let z = -0.50 + (i / 8) as f32 * 0.15 + jitter(i + 7);
        let species = if i % 20 < 13 {
            0
        } else if i % 20 < 18 {
            1
        } else {
            2
        };
        roots.push(instance(
            x,
            z,
            -0.4 + (i * 41 % 29) as f32 * 0.09,
            12345 + i * 79,
            species,
        ));
    }
    let all = build_topology_indices();
    let indices = &all[SPLIT_HIGH_FIRST_INDEX as usize..SPLIT_LOW_FIRST_INDEX as usize];
    let mut requests = Vec::new();
    for i in 0..roots.len() as u32 {
        for tri in indices.chunks_exact(3) {
            if i < 3 && tri.iter().any(|&vertex| vertex >= 32) {
                continue;
            }
            requests.push([u32::from(tri[0]), u32::from(tri[1]), u32::from(tri[2]), i]);
        }
    }
    assert_eq!(requests.len(), 541);
    assert!(roots.len() * 2 <= 256);
    (roots, requests)
}

#[test]
fn shadow_fixture_shader_and_catalog_contract() {
    let module = naga::front::wgsl::parse_str(&source())
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source())));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    let source = study();
    for name in [
        "short_split_fill_ribbon",
        "study_middle_arch",
        "study_high_arch",
    ] {
        assert!(source.catalog.species.iter().any(|s| s.key == name));
    }
    let (roots, requests) = fixture();
    assert_eq!(roots.len(), 43);
    assert_eq!(requests.iter().filter(|r| r[3] == 0).count(), 7);
}

#[test]
#[ignore = "offline GPU experiment; use tools/grass_shadow_study.py"]
fn capture_shadow_study() {
    let output = PathBuf::from(
        std::env::var_os("YARRA_SHADOW_STUDY_OUTPUT").expect("explicit output directory required"),
    );
    fs::create_dir_all(&output).unwrap();
    let study = study();
    let method = std::env::var("YARRA_SHADOW_STUDY_METHOD").unwrap_or_else(|_| "pixel".into());
    assert!(matches!(method.as_str(), "point" | "pixel"));
    let parameter = |name: &str, fallback: f32| -> f32 {
        let value = std::env::var(name)
            .map(|s| s.parse::<f32>().unwrap())
            .unwrap_or(fallback);
        assert!(value.is_finite());
        value
    };
    let steps = parameter("YARRA_SHADOW_STUDY_STEPS", 16.0);
    let strength = parameter("YARRA_SHADOW_STUDY_STRENGTH", 0.45);
    let thickness = parameter(
        "YARRA_SHADOW_STUDY_THICKNESS",
        if method == "point" { 0.018 } else { 0.004 },
    );
    assert!(steps == 8.0 || steps == 16.0);
    assert!((0.0..=1.0).contains(&strength) && (0.0001..=0.05).contains(&thickness));
    let packed: Vec<_> = [
        "short_split_fill_ribbon",
        "study_middle_arch",
        "study_high_arch",
    ]
    .into_iter()
    .map(|key| {
        pack_species(
            study.catalog.species.iter().find(|s| s.key == key).unwrap(),
            [12.0; 2],
        )
    })
    .collect();
    let (roots, requests) = fixture();
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = resources.0.wgpu_device();
    let queue = &resources.1;
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("offline shadow fixture / production blade functions"),
        source: ShaderSource::Wgsl(source().into()),
    });
    let buffer = |label, bytes: &[u8], usage| {
        let result = device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: bytes.len() as u64,
            usage: usage | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&result, 0, bytes);
        result
    };
    let root_buffer = buffer(
        "fixture roots",
        bytemuck::cast_slice(&roots),
        BufferUsages::STORAGE,
    );
    let species_buffer = buffer(
        "authored species",
        bytemuck::cast_slice(&packed),
        BufferUsages::STORAGE,
    );
    let diagnostic = buffer(
        "unused diagnostic data",
        &vec![0u8; roots.len() * 64],
        BufferUsages::STORAGE,
    );
    let config = buffer(
        "full physical geometry",
        bytemuck::bytes_of(&DebugConfigGpu {
            values: [0, 2, 0, 0],
            workload: [0, 0, 0, 288], // opening disabled, full packed morph
        }),
        BufferUsages::UNIFORM,
    );
    let camera = buffer(
        "fixture camera",
        bytemuck::bytes_of(&CameraGpu::zeroed()),
        BufferUsages::UNIFORM,
    );
    // Fallback vertex evaluation is the production path; prepared indices are all zero.
    let prepared = buffer(
        "unused prepared arena",
        &vec![0u8; 851968 * 4 + 128],
        BufferUsages::STORAGE,
    );
    let parameters = buffer("fixture controls", &[0u8; 64], BufferUsages::UNIFORM);
    let request_buffer = buffer(
        "actual index templates",
        bytemuck::cast_slice(&requests),
        BufferUsages::STORAGE,
    );
    let triangles = buffer(
        "exact wind-deformed triangles",
        &vec![0u8; requests.len() * 48],
        BufferUsages::STORAGE | BufferUsages::COPY_SRC,
    );
    let mut draw_indices = vec![0u16, 1, 2, 1, 3, 2];
    draw_indices.extend(
        build_topology_indices()[SPLIT_HIGH_FIRST_INDEX as usize..SPLIT_LOW_FIRST_INDEX as usize]
            .iter(),
    );
    let indices = buffer(
        "ground and actual grass indices",
        bytemuck::cast_slice(&draw_indices),
        BufferUsages::INDEX,
    );
    let export = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: Some("export real grass triangles"),
        layout: None,
        module: &module,
        entry_point: Some("export_geometry"),
        compilation_options: default(),
        cache: None,
    });
    let targets = [
        TextureFormat::Rgba8UnormSrgb,
        TextureFormat::Rgba8UnormSrgb,
        TextureFormat::Rgba8UnormSrgb,
        TextureFormat::Rgba32Float,
        TextureFormat::Rgba8Unorm,
    ];
    let pipeline = |entry, formats: &[TextureFormat]| {
        device.create_render_pipeline(&RawRenderPipelineDescriptor {
            label: Some(entry),
            layout: None,
            vertex: RawVertexState {
                module: &module,
                entry_point: Some("fixture_vertex"),
                buffers: &[],
                compilation_options: default(),
            },
            fragment: Some(RawFragmentState {
                module: &module,
                entry_point: Some(entry),
                targets: &formats
                    .iter()
                    .map(|&format| {
                        Some(ColorTargetState {
                            format,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })
                    })
                    .collect::<Vec<_>>(),
                compilation_options: default(),
            }),
            primitive: PrimitiveState {
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: default(),
                bias: default(),
            }),
            multisample: default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let capture = pipeline("capture_position", &[TextureFormat::Rgba32Float]);
    let compare = pipeline("compare", &targets);
    let texture = |format, usage| {
        device.create_texture(&TextureDescriptor {
            label: None,
            size: Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let gbuffer = texture(
        TextureFormat::Rgba32Float,
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC,
    );
    let position_view = gbuffer.create_view(&default());
    let depth = texture(
        TextureFormat::Depth32Float,
        TextureUsages::RENDER_ATTACHMENT,
    );
    let depth_view = depth.create_view(&default());
    let colors = targets.map(|f| {
        texture(
            f,
            TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        )
    });
    let views = colors.each_ref().map(|t| t.create_view(&default()));
    let bind = |layout: &_, entries: &[BindGroupEntry<'_>]| {
        device.create_bind_group(&BindGroupDescriptor {
            label: None,
            layout,
            entries,
        })
    };
    macro_rules! b {
        ($binding:expr, $buf:expr) => {
            BindGroupEntry {
                binding: $binding,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: $buf,
                    offset: 0,
                    size: None,
                }),
            }
        };
    }
    let inputs = [
        b!(0, &root_buffer),
        b!(1, &diagnostic),
        b!(2, &species_buffer),
        b!(3, &camera),
        b!(4, &config),
        b!(5, &prepared),
    ];
    let export_inputs = bind(&export.get_bind_group_layout(1), &inputs);
    let capture_inputs = bind(&capture.get_bind_group_layout(1), &inputs);
    let compare_inputs = bind(&compare.get_bind_group_layout(1), &inputs);
    let export_group = bind(
        &export.get_bind_group_layout(0),
        &[
            b!(0, &parameters),
            b!(1, &triangles),
            b!(2, &request_buffer),
        ],
    );
    let capture_group = bind(&capture.get_bind_group_layout(0), &[b!(0, &parameters)]);
    let compare_group = bind(&compare.get_bind_group_layout(0), &[b!(0, &parameters)]);
    let shadow_group = bind(
        &compare.get_bind_group_layout(2),
        &[
            b!(0, &triangles),
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::TextureView(&position_view),
            },
        ],
    );
    let size = u64::from(WIDTH * HEIGHT);
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("offline comparison readback"),
        size: size * 48 + triangles.size(),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let quick = std::env::var_os("YARRA_SHADOW_STUDY_QUICK").is_some();
    let phases: &[f32] = if quick {
        &[0.0]
    } else {
        &[0.0, 1.0 / 60.0, 0.5, 1.0]
    };
    let origin = Vec3::new(0.0, 0.12, 0.0);
    let cameras = [
        (
            "patch",
            Vec3::new(0.85, 1.1, 1.25),
            Vec3::new(0.85, 0.15, 0.0),
        ),
        (
            "crossing",
            Vec3::new(-0.63, 0.78, 1.72),
            Vec3::new(-0.63, 0.15, 0.68),
        ),
        (
            "isolated",
            Vec3::new(-1.45, 0.75, 0.85),
            Vec3::new(-1.45, 0.07, -0.30),
        ),
        ("oblique", Vec3::new(0.0, 2.3, 3.3), origin),
        ("top", Vec3::new(0.0, 4.2, 0.01), origin),
        ("low", Vec3::new(0.0, 0.8, 3.7), origin),
        ("orbit", Vec3::new(-2.4, 1.7, 2.5), origin),
    ];
    let mut manifest = String::from("view,light,phase,stem\n");
    for (name, position, target) in cameras.into_iter().take(if quick { 1 } else { 7 }) {
        let transform = Transform::from_translation(position).looking_at(target, Vec3::Y);
        let projection = Mat4::perspective_infinite_reverse_rh(
            50f32.to_radians(),
            WIDTH as f32 / HEIGHT as f32,
            0.05,
        );
        let forward = transform.forward();
        for (light_name, sun) in [
            ("high", Vec3::new(-0.35, 0.92, 0.18).normalize()),
            ("low", Vec3::new(-0.92, 0.30, 0.25).normalize()),
        ]
        .into_iter()
        .take(if quick { 1 } else { 2 })
        {
            for (phase_index, &phase) in phases.iter().enumerate() {
                let stem = format!("{name}-{light_name}-{phase_index}");
                let c = CameraGpu {
                    lod_focus: [0.0; 4],
                    canopy: study.lighting.canopy.packed([0.0; 2]),
                    clip_from_world: (projection * transform.to_matrix().inverse()).to_cols_array(),
                    camera_position: position.extend(1.0).to_array(),
                    projection: [
                        HEIGHT as f32 / (2.0 * 25f32.to_radians().tan()),
                        WIDTH as f32,
                        HEIGHT as f32,
                        0.0,
                    ],
                    sun_direction: sun.extend(1.0).to_array(),
                    sun_radiance: [100000.0, 100000.0, 100000.0, 0.0],
                    ambient_radiance: [1.0; 4],
                    lighting: [
                        study.lighting.diffuse_strength,
                        study.lighting.specular_strength,
                        study.lighting.transmission_strength,
                        study.lighting.received_shadow_strength,
                    ],
                    wind: [
                        study.wind.direction.0,
                        study.wind.direction.1,
                        study.wind.strength,
                        phase,
                    ],
                    wind_shape: [
                        study.wind.spatial_frequency,
                        study.wind.speed,
                        study.wind.gustiness,
                        study.wind.flutter,
                    ],
                };
                queue.write_buffer(&camera, 0, bytemuck::bytes_of(&c));
                let settings = [
                    requests.len() as f32,
                    roots.len() as f32,
                    steps,
                    bevy::camera::Exposure {
                        ev100: study.exposure_ev100,
                    }
                    .exposure(),
                    forward.x,
                    forward.y,
                    forward.z,
                    0.35,
                    thickness,
                    0.0015,
                    2.0,
                    0.0,
                    if method == "pixel" { 1.0 } else { 0.0 },
                    strength,
                    0.0015,
                    0.60,
                ];
                queue.write_buffer(&parameters, 0, bytemuck::cast_slice(&settings));
                let mut encoder =
                    device.create_command_encoder(&CommandEncoderDescriptor::default());
                {
                    let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
                    pass.set_pipeline(&export);
                    pass.set_bind_group(0, &export_group, &[]);
                    pass.set_bind_group(1, &export_inputs, &[]);
                    pass.dispatch_workgroups((requests.len() as u32).div_ceil(64), 1, 1);
                }
                for stage in 0..2 {
                    let attachments: Vec<_> = if stage == 0 {
                        vec![&position_view]
                    } else {
                        views.iter().collect()
                    }
                    .into_iter()
                    .map(|view| {
                        Some(RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: Operations {
                                load: LoadOp::Clear(Default::default()),
                                store: StoreOp::Store,
                            },
                        })
                    })
                    .collect();
                    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                        label: Some(if stage == 0 {
                            "capture nearest surface"
                        } else {
                            "offline shadow reference and approximation"
                        }),
                        color_attachments: &attachments,
                        depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(Operations {
                                load: LoadOp::Clear(0.0),
                                store: StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        ..default()
                    });
                    if stage == 0 {
                        pass.set_pipeline(&capture);
                        pass.set_bind_group(0, &capture_group, &[]);
                        pass.set_bind_group(1, &capture_inputs, &[]);
                    } else {
                        pass.set_pipeline(&compare);
                        pass.set_bind_group(0, &compare_group, &[]);
                        pass.set_bind_group(1, &compare_inputs, &[]);
                        pass.set_bind_group(2, &shadow_group, &[]);
                    }
                    pass.set_index_buffer(indices.slice(..), IndexFormat::Uint16);
                    pass.draw_indexed(0..6, 0, roots.len() as u32..roots.len() as u32 + 1);
                    pass.draw_indexed(6..30, 0, 0..3);
                    pass.draw_indexed(6..48, 0, 3..roots.len() as u32);
                }
                let mut offset = 0;
                for (tex, bytes) in colors.iter().zip([4, 4, 4, 16, 4]).chain([(&gbuffer, 16)]) {
                    encoder.copy_texture_to_buffer(
                        TexelCopyTextureInfo {
                            texture: tex,
                            mip_level: 0,
                            origin: Origin3d::ZERO,
                            aspect: TextureAspect::All,
                        },
                        TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: TexelCopyBufferLayout {
                                offset,
                                bytes_per_row: Some(WIDTH * bytes),
                                rows_per_image: Some(HEIGHT),
                            },
                        },
                        Extent3d {
                            width: WIDTH,
                            height: HEIGHT,
                            depth_or_array_layers: 1,
                        },
                    );
                    offset += size * u64::from(bytes);
                }
                encoder.copy_buffer_to_buffer(&triangles, 0, &readback, offset, triangles.size());
                queue.submit([encoder.finish()]);
                let (sender, receiver) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(MapMode::Read, move |result| sender.send(result).unwrap());
                device.poll(PollType::wait_indefinitely()).unwrap();
                receiver.recv().unwrap().unwrap();
                {
                    let data = readback.slice(..).get_mapped_range();
                    fs::write(output.join(format!("{stem}.bin")), &data).unwrap();
                }
                readback.unmap();
                manifest.push_str(&format!("{name},{light_name},{phase},{stem}\n"));
                eprintln!("Captured {stem}: 83 blades, 541 triangles, frozen wind {phase:.6}s");
            }
        }
    }
    fs::write(output.join("frames.csv"), manifest).unwrap();
    fs::write(output.join("parameters.txt"), format!("size={WIDTH}x{HEIGHT}\nblades=83\ntriangles={}\nmethod={method}\nsteps={steps}\nstrength={strength}\nlength=0.35\nthickness={thickness}\nbias=0.0015\nreference_length=2\nopening=off\nMSAA=off\n", requests.len())).unwrap();
}
