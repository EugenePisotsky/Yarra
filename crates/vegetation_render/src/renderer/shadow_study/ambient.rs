use super::*;

const WIDTH: u32 = 2560;
const HEIGHT: u32 = 1440;

// Offline only: retain the production shape/material, adding visibility to ambient separately.
fn ambient_source() -> String {
    let start = DRAW.find("// Vegetation V2 procedural").unwrap();
    let mut draw =
        super::super::tests::preprocess_band_study(&DRAW[start..], VegetationBladeBands::Off);
    let begin = draw.find("fn directional_shadow_visibility(").unwrap();
    let end = draw.find("fn radiance_tint(").unwrap();
    draw.replace_range(
        begin..end,
        "fn directional_shadow_visibility(input: VertexOutput) -> f32 { return 1.0; }\n",
    );
    for (from, to) in [
        (
            "struct VertexOutput {",
            "struct VertexOutput {\n @location(8) @interpolate(flat) identity: u32,\n @location(9) companion_t: f32,",
        ),
        (
            "output.world_position = world_position;",
            "output.world_position = world_position;\n output.identity = 1u + instance_index * 2u;",
        ),
        (
            "output.blade_t = shading_t;",
            "output.blade_t = shading_t;\n output.companion_t = select(0.0, shading_t, ((vertex_index >> 5u) & 1u) != 0u);",
        ),
        (
            "@fragment\nfn fragment(\n    input: VertexOutput,",
            "fn shade_blade(\n input: VertexOutput, direct_visibility: f32, ambient_visibility: f32,",
        ),
        (") -> @location(0) vec4<f32>", ") -> vec4<f32>"),
        ("view_bindings::view.exposure", "fixture.counts.w"),
        ("pbr_lighting::", ""),
        (
            "* shadow_visibility",
            "* shadow_visibility * direct_visibility",
        ),
        (
            "foliage_ambient + diffuse",
            "foliage_ambient * ambient_visibility + diffuse",
        ),
    ] {
        assert!(draw.contains(from), "production contract changed: {from}");
        draw = draw.replace(from, to);
    }
    format!(
        "{CANOPY}\n{BLADE}\n{draw}\n{}",
        include_str!("ambient_fixture.wgsl")
    )
}

fn tuft_fixture() -> (Vec<ProceduralInstanceGpu>, Vec<[u32; 4]>) {
    let mut roots = Vec::new();
    for (clump, (cx, cz)) in [
        (-0.36f32, -0.35f32),
        (0.38, -0.29),
        (-0.33, 0.37),
        (0.43, 0.43),
    ]
    .into_iter()
    .enumerate()
    {
        for blade in 0..32u32 {
            let angle = blade as f32 * 2.3999631 + clump as f32 * 0.71;
            // A compact crown with upright inner leaves and an arching outer skirt.
            // Keep the same 128 roots in every lighting comparison.
            let radius = 0.16 * ((blade as f32 + 0.5) / 32.0).sqrt();
            let yaw = angle + ((blade * 37 % 19) as f32 / 18.0 - 0.5) * 0.50;
            let species = if blade < 3 {
                2
            } else if blade < 11 {
                1
            } else {
                0
            };
            let mut root = instance(
                cx + radius * angle.sin(),
                cz + radius * angle.cos(),
                yaw,
                12531 + clump as u32 * 23131 + blade * 79,
                species,
            );
            root.root_clump[3] = f32::from_bits([19000, 41000, 27000, 53000][clump]);
            roots.push(root);
        }
    }
    let all = build_topology_indices();
    let mut requests = Vec::new();
    for i in 0..roots.len() as u32 {
        for tri in
            all[SPLIT_HIGH_FIRST_INDEX as usize..SPLIT_LOW_FIRST_INDEX as usize].chunks_exact(3)
        {
            requests.push([u32::from(tri[0]), u32::from(tri[1]), u32::from(tri[2]), i]);
        }
    }
    (roots, requests)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BvhNode {
    minimum: [f32; 4],
    maximum: [f32; 4],
    // first triangle, triangle count (zero for interior), escape index, unused
    links: [u32; 4],
}

fn build_bvh(mut tris: Vec<[[f32; 4]; 3]>) -> (Vec<[[f32; 4]; 3]>, Vec<BvhNode>) {
    fn build(tris: &mut [[[f32; 4]; 3]], offset: usize, nodes: &mut Vec<BvhNode>) {
        let mut low = [f32::INFINITY; 4];
        let mut high = [f32::NEG_INFINITY; 4];
        for tri in tris.iter() {
            for vertex in tri {
                for axis in 0..3 {
                    low[axis] = low[axis].min(vertex[axis]);
                    high[axis] = high[axis].max(vertex[axis]);
                }
            }
        }
        for axis in 0..3 {
            low[axis] -= 0.00001;
            high[axis] += 0.00001;
        }
        low[3] = 0.0;
        high[3] = 0.0;
        let index = nodes.len();
        nodes.push(BvhNode {
            minimum: low,
            maximum: high,
            links: [offset as u32, tris.len() as u32, 0, 0],
        });
        if tris.len() > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| (high[a] - low[a]).total_cmp(&(high[b] - low[b])))
                .unwrap();
            tris.sort_unstable_by(|a, b| {
                let center = |t: &[[f32; 4]; 3]| t[0][axis] + t[1][axis] + t[2][axis];
                center(a).total_cmp(&center(b))
            });
            let middle = tris.len() / 2;
            let (left, right) = tris.split_at_mut(middle);
            nodes[index].links[1] = 0;
            build(left, offset, nodes);
            build(right, offset + middle, nodes);
        }
        nodes[index].links[2] = nodes.len() as u32;
    }
    let mut nodes = Vec::new();
    build(&mut tris, 0, &mut nodes);
    (tris, nodes)
}

#[test]
fn tuft_ambient_shader_contract() {
    let source = ambient_source();
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    assert_eq!(tuft_fixture().0.len(), 128);
}

#[test]
#[ignore = "offline GPU experiment; set YARRA_TUFT_AMBIENT_OUTPUT explicitly"]
fn capture_tuft_ambient() {
    let output = PathBuf::from(
        std::env::var_os("YARRA_TUFT_AMBIENT_OUTPUT").expect("explicit output directory required"),
    );
    fs::create_dir_all(&output).unwrap();
    let study = study();
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
    let (roots, requests) = tuft_fixture();
    let resources = bevy::tasks::block_on(initialize_renderer(
        Backends::PRIMARY,
        None,
        &WgpuSettings::default(),
    ));
    let device = resources.0.wgpu_device();
    let queue = &resources.1;
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("offline shadow fixture / production blade functions"),
        source: ShaderSource::Wgsl(ambient_source().into()),
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
    let grass_index_end = draw_indices.len() as u32;
    if draw_indices.len() % 2 != 0 {
        draw_indices.push(0);
    }
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
    let bvh = buffer(
        "offline triangle BVH",
        &vec![0u8; requests.len() * 2 * 48],
        BufferUsages::STORAGE,
    );
    let shadow_group = bind(
        &compare.get_bind_group_layout(2),
        &[b!(0, &triangles), b!(1, &bvh)],
    );
    let size = u64::from(WIDTH * HEIGHT);
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("offline comparison readback"),
        size: size * 48 + triangles.size(),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let phases = &[0.0f32];
    let origin = Vec3::new(0.0, 0.20, 0.0);
    let cameras = [
        ("overhead", Vec3::new(0.0, 2.6, 2.8), origin),
        ("near", Vec3::new(0.0, 0.85, 2.6), origin),
    ];
    let mut geometry_ready = false;
    let mut manifest = String::from("view,light,phase,stem\n");
    for (name, position, target) in cameras.into_iter() {
        let transform = Transform::from_translation(position).looking_at(target, Vec3::Y);
        let projection = Mat4::perspective_infinite_reverse_rh(
            50f32.to_radians(),
            WIDTH as f32 / HEIGHT as f32,
            0.05,
        );
        let forward = transform.forward();
        for (light_name, sun) in [("sun", Vec3::new(-0.78, 0.42, 0.46).normalize())] {
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
                    sun_radiance: [100000.0, 84900.0, 63830.0, 0.0],
                    ambient_radiance: [1478.5, 2100.0, 3191.4, 0.0],
                    lighting: [
                        study.lighting.diffuse_strength,
                        study.lighting.specular_strength,
                        study.lighting.transmission_strength,
                        study.lighting.received_shadow_strength,
                    ],
                    wind: [study.wind.direction.0, study.wind.direction.1, 0.0, phase],
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
                    64.0,
                    bevy::camera::Exposure {
                        ev100: study.exposure_ev100,
                    }
                    .exposure(),
                    forward.x,
                    forward.y,
                    forward.z,
                    0.35,
                    0.0,
                    0.0015,
                    4.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                ];
                queue.write_buffer(&parameters, 0, bytemuck::cast_slice(&settings));
                if !geometry_ready {
                    let mut encoder = device.create_command_encoder(&default());
                    {
                        let mut pass = encoder.begin_compute_pass(&default());
                        pass.set_pipeline(&export);
                        pass.set_bind_group(0, &export_group, &[]);
                        pass.set_bind_group(1, &export_inputs, &[]);
                        pass.dispatch_workgroups((requests.len() as u32).div_ceil(64), 1, 1);
                    }
                    encoder.copy_buffer_to_buffer(&triangles, 0, &readback, 0, triangles.size());
                    queue.submit([encoder.finish()]);
                    let (sender, receiver) = std::sync::mpsc::channel();
                    readback
                        .slice(..)
                        .map_async(MapMode::Read, move |r| sender.send(r).unwrap());
                    device.poll(PollType::wait_indefinitely()).unwrap();
                    receiver.recv().unwrap().unwrap();
                    let data = readback.slice(..).get_mapped_range();
                    let original: Vec<[[f32; 4]; 3]> =
                        bytemuck::cast_slice(&data[..triangles.size() as usize]).to_vec();
                    let (ordered, nodes) = build_bvh(original);
                    fs::write(output.join("triangles.bin"), bytemuck::cast_slice(&ordered))
                        .unwrap();
                    fs::write(output.join("bvh.bin"), bytemuck::cast_slice(&nodes)).unwrap();
                    drop(data);
                    readback.unmap();
                    queue.write_buffer(&triangles, 0, bytemuck::cast_slice(&ordered));
                    queue.write_buffer(&bvh, 0, bytemuck::cast_slice(&nodes));
                    geometry_ready = true;
                }
                let mut encoder = device.create_command_encoder(&default());
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
                            "ambient visibility and exact sun visibility"
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
                    pass.draw_indexed(6..grass_index_end, 0, 0..roots.len() as u32);
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
                eprintln!(
                    "Captured {stem}: fixed four tufts; ambient visibility and sun shadows separated"
                );
            }
        }
    }
    fs::write(output.join("frames.csv"), manifest).unwrap();
    fs::write(output.join("parameters.txt"), format!("size={WIDTH}x{HEIGHT}\nroots={}\nblades={}\ntriangles={}\nambient_samples=64\nray_range=4m\nwind=off\nmarks=off\nopening=off\nMSAA=off\nambient=production_sky\n", roots.len(), roots.len()*2, requests.len())).unwrap();
}
