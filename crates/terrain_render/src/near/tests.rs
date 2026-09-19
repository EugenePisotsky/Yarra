use super::*;
use world::{PageDomain, TerrainTextureSetId};
fn source(cell: CellCoord) -> NearSource {
    let set = TerrainTextureSetId([3; 16]);
    let layers: Vec<_> = (0..2)
        .map(|i| TerrainSurfaceLayer {
            layer: i,
            surface: TerrainSurface {
                id: TerrainSurfaceId([i as u8; 16]),
                key: format!("surface-{i}"),
                display_name: String::new(),
                tile_size: 1.3 + i as f32,
                anti_tiling: true,
                normal_y_sign: 1.,
                normal_strength: 1.,
                roughness_min: 0.6,
                roughness_max: 1.,
            },
        })
        .collect();
    NearSource {
        key: PageKey {
            space: WorldSpaceId(1),
            cell,
            domain: PageDomain::TerrainRender,
            lod: 0,
        },
        cell_size: 8.,
        height_bounds: [0., 0.],
        surfaces: layers.iter().map(|l| l.surface.id).collect(),
        weights: vec![TerrainWeightPage {
            resolution: 3,
            rgba: [255, 0, 0, 0, 128, 127, 0, 0, 0, 255, 0, 0].repeat(3),
        }],
        profile: TerrainProfile {
            space: WorldSpaceId(1),
            texture_set: set,
            weight_resolution: 3,
            macro_scales: [7., 19., 43.],
            macro_contrast: 1.,
            macro_albedo_strength: 0.3,
        },
        texture_set: TerrainTextureSet {
            id: set,
            key: "fixture".into(),
            base_color_universal_uri: String::new(),
            normal_material_universal_uri: String::new(),
            macro_variation_universal_uri: String::new(),
            base_color_astc_uri: String::new(),
            normal_material_astc_uri: String::new(),
            macro_variation_astc_uri: String::new(),
            universal_gpu_bytes: 1024,
            astc_gpu_bytes: 1024,
        },
        layers,
    }
}
fn material(s: &NearSource) -> TerrainMaterial {
    TerrainMaterial {
        source_only: true,
        shading_mode: TerrainShadingMode::Production,
        stochastic_cached: false,
        prepared: false,
        prepared_albedo: false,
        source_weights: default(),
        source_base_color_array: default(),
        weights: default(),
        base_color_array: default(),
        normal_material_array: default(),
        macro_variation: default(),
        stochastic_cache: default(),
        canopy_bounds: Vec4::ZERO,
        canopy_shading: default(),
        canopy_coverage: None,
        settings: TerrainMaterialUniform {
            chunk_minimum: Vec2::from_array(s.key.cell.origin(s.cell_size).map(|v| v as f32)),
            chunk_extent: Vec2::splat(s.cell_size),
            surface_layers: Vec4::new(0., 1., 2., 0.),
            tile_sizes: Vec4::new(
                s.layers[0].surface.tile_size,
                s.layers[1].surface.tile_size,
                0.,
                0.,
            ),
            normal_settings: Vec4::ONE,
            roughness_ranges: Vec4::new(0.6, 1., 0.6, 1.),
            macro_scales: Vec4::new(7., 19., 43., 0.3),
            macro_settings: Vec4::ONE,
            cache_origins: Vec4::ZERO,
            cache_size: UVec4::ZERO,
        },
    }
}
#[test]
fn phases_keep_large_negative_cell_boundaries_and_altitude_stops_near_demand() {
    for x in [-2_000_000_000, -9, 0, 100_000_000] {
        let a = source(CellCoord { x, z: -7 });
        let b = source(CellCoord { x: x + 1, z: -7 });
        let m = material(&a);
        let mut pack = Pack::from_material(&m);
        pack.period = 4.;
        let ea = gpu::entry(&a, &m, &pack, 0, 1.);
        let eb = gpu::entry(&b, &m, &pack, 1, 1.);
        let circular_error = |a: f64, b: f64| {
            let e = (a - b).rem_euclid(1.);
            e.min(1. - e)
        };
        assert!(
            circular_error(
                ea.phase0.x as f64 + a.cell_size as f64 / m.settings.tile_sizes.x as f64,
                eb.phase0.x as f64
            ) < 0.00001
        );
        assert!(
            circular_error(
                ea.phase0.z as f64 + a.cell_size as f64 / m.settings.tile_sizes.x as f64 / 4.,
                eb.phase0.z as f64
            ) < 0.00001
        );
        let min = a.key.cell.origin(8.);
        assert_eq!(
            distance_squared(&a, DVec3::new(min[0] + 4., 0., min[1] + 4.)),
            0.
        );
        assert!(
            distance_squared(&a, DVec3::new(min[0] + 4., 500., min[1] + 4.))
                > f64::from(NEAR_END).powi(2)
        );
    }
    assert!(gpu::bytes() < 12 * 1024 * 1024);
}

#[derive(Resource, Default, Clone)]
struct Pixels(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
#[test]
#[ignore = "native GPU readback: near cache, surfaces, canopy, rebasing and eviction"]
fn near_surface_streams_on_unchanged_mesh_and_returns_to_baked_ground() {
    use bevy::{
        app::PluginsState,
        camera::{RenderTarget, ScalingMode},
        render::{
            RenderApp, RenderPlugin,
            gpu_readback::{Readback, ReadbackComplete},
            pipelined_rendering::PipelinedRenderingPlugin,
            render_resource::{
                CachedPipelineState, PipelineCache, TextureUsages, TextureViewDescriptor,
                TextureViewDimension,
            },
        },
        window::ExitCondition,
        winit::WinitPlugin,
    };
    use world::{TerrainComposite, TerrainCompositeMip, TerrainMaterialKey, TerrainNodeKey};
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(AssetPlugin {
                file_path: format!("{}/../../assets", env!("CARGO_MANIFEST_DIR")),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>(),
    )
    .add_plugins(TerrainRenderPlugin)
    .init_resource::<Pixels>()
    .insert_resource(ClearColor(Color::BLACK))
    .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1. / 60.),
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    while app.plugins_state() != PluginsState::Ready {
        assert!(std::time::Instant::now() < deadline);
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();
    let mut target = Image::new_target_texture(256, 256, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    app.world_mut()
        .spawn(Readback::texture(target.clone()))
        .observe(|e: On<ReadbackComplete>, p: Res<Pixels>| {
            *p.0.lock().unwrap() = e.data.clone();
        });
    let camera = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            RenderTarget::Image(target.into()),
            Transform::from_xyz(-4., 8., 4.).looking_at(Vec3::new(-4., 0., 4.), Vec3::NEG_Z),
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical {
                    viewport_height: 8.,
                },
                ..OrthographicProjection::default_3d()
            }),
            Msaa::Off,
        ))
        .id();
    app.world_mut().spawn((
        DirectionalLight {
            illuminance: 20000.,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -1.1, -0.4, 0.)),
    ));
    let s = source(CellCoord { x: -1, z: 0 });
    let composite = TerrainComposite {
        key: TerrainMaterialKey(TerrainNodeKey::leaf(s.key.space, s.key.cell)),
        fingerprint: [0; 32],
        mips: (0..3)
            .map(|l| TerrainCompositeMip {
                color: [75, 50, 25, 255].repeat(TerrainComposite::mip_size(l).pow(2)),
                response: [128, 128, 220, 255].repeat(TerrainComposite::mip_size(l).pow(2)),
            })
            .collect(),
    };
    let baked_data = composite.clone();
    let composite = TerrainCompositeMaterial::from_composite(
        composite,
        CellCoord::ZERO,
        8.,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
    )
    .unwrap();
    let composite = app
        .world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .add(composite);
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(
        build_heightfield_mesh(
            &TerrainHeightfield::from_heights(2, &[0.; 4], 0., 0., 8.).unwrap(),
            8.,
        )
        .unwrap(),
    );
    let ground = app
        .world_mut()
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(composite.clone()),
            Transform::from_xyz(-4., 0., 4.),
        ))
        .id();
    let settle = |app: &mut App| {
        let mut last = vec![];
        let mut stable = 0;
        let mut frames = 0;
        loop {
            assert!(std::time::Instant::now() < deadline, "near render timeout");
            app.update();
            for p in app
                .sub_app(RenderApp)
                .world()
                .resource::<PipelineCache>()
                .pipelines()
            {
                if let CachedPipelineState::Err(e) = &p.state {
                    panic!("near shader: {e}")
                }
            }
            let pixels = app.world().resource::<Pixels>().0.lock().unwrap().clone();
            let changing = app
                .world()
                .resource::<Cache>()
                .pages
                .values()
                .any(|p| !p.uploaded || p.fade < 1.);
            stable = if !changing && pixels == last && pixels.iter().any(|v| *v > 0) {
                stable + 1
            } else {
                0
            };
            last = pixels;
            frames += 1;
            if stable >= 5 && frames > 20 {
                break last;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let baked = settle(&mut app);
    let array = |colors: [[u8; 4]; 2], srgb| {
        let mut image = Image::new(
            Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 2,
            },
            TextureDimension::D2,
            colors
                .into_iter()
                .flat_map(|c| {
                    (0..16).flat_map(move |i| {
                        let scale = if srgb {
                            0.65 + (i % 4) as f32 * 0.1
                        } else {
                            1.
                        };
                        [
                            (c[0] as f32 * scale) as u8,
                            (c[1] as f32 * scale) as u8,
                            (c[2] as f32 * scale) as u8,
                            c[3],
                        ]
                    })
                })
                .collect(),
            if srgb {
                TextureFormat::Rgba8UnormSrgb
            } else {
                TextureFormat::Rgba8Unorm
            },
            RenderAssetUsages::default(),
        );
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..default()
        });
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        });
        image
    };
    let mut m = material(&s);
    {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        m.base_color_array = images.add(array([[220, 70, 25, 255], [40, 170, 30, 255]], true));
        m.source_base_color_array = m.base_color_array.clone();
        m.normal_material_array =
            images.add(array([[180, 128, 255, 220], [180, 128, 255, 220]], false));
        m.macro_variation = images.add(Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![128, 128, 128, 255],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        ));
        m.weights = images.add(crate::make_weight_image(&s.surfaces, &s.weights).unwrap());
        m.source_weights = m.weights.clone();
    }
    let material = app
        .world_mut()
        .resource_mut::<Assets<TerrainMaterial>>()
        .add(m);
    let entity = app
        .world_mut()
        .spawn((s.clone(), MeshMaterial3d(material.clone())))
        .id();
    let identity = Some(("test".into(), s.key.space));
    {
        let mut cache = app.world_mut().resource_mut::<Cache>();
        cache.identity = identity.clone();
        cache.pages.insert(
            entity,
            Resident {
                cell: s.key.cell,
                slot: 0,
                material: material.clone(),
                fade: 0.,
                uploaded: false,
                canopy: None,
            },
        );
    }
    *app.world_mut().resource_mut::<NearView>() = NearView {
        identity,
        origin: CellCoord::ZERO,
        eye: DVec3::new(-4., 8., 4.),
    };
    let near = settle(&mut app);
    // Fully available close material must not depend on the distant cache's
    // color or coarse normal. Loading edges must still reveal that fallback.
    let original_composite = app
        .world()
        .resource::<Assets<TerrainCompositeMaterial>>()
        .get(&composite)
        .unwrap()
        .clone();
    let mut changed_bake = baked_data;
    for mip in &mut changed_bake.mips {
        for p in mip.color.chunks_exact_mut(4) {
            p.copy_from_slice(&[25, 75, 180, 255]);
        }
        for p in mip.response.chunks_exact_mut(4) {
            p.copy_from_slice(&[195, 85, 160, 255]);
        }
    }
    let changed_composite = TerrainCompositeMaterial::from_composite(
        changed_bake,
        CellCoord::ZERO,
        8.,
        &mut app.world_mut().resource_mut::<Assets<Image>>(),
    )
    .unwrap();
    *app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap() = changed_composite;
    let different_fallback = settle(&mut app);
    for y in 64..192 {
        for x in 64..192 {
            let i = (y * 256 + x) * 4;
            assert_eq!(&near[i..i + 4], &different_fallback[i..i + 4]);
        }
    }
    assert_ne!(
        &near[128 * 256 * 4..][..32],
        &different_fallback[128 * 256 * 4..][..32]
    );
    *app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap() = original_composite;
    assert_eq!(near, settle(&mut app));
    // Keep the source resident while moving through the distance fade. The
    // shader must still blend in the transition and use only the bake beyond it.
    for (height, should_match_bake) in [(32., false), (48., true)] {
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .y = height;
        let with_near = settle(&mut app);
        app.world_mut()
            .resource_mut::<Assets<TerrainCompositeMaterial>>()
            .get_mut(&composite)
            .unwrap()
            .near_disabled = true;
        let without_near = settle(&mut app);
        assert_eq!(with_near == without_near, should_match_bake);
        app.world_mut()
            .resource_mut::<Assets<TerrainCompositeMaterial>>()
            .get_mut(&composite)
            .unwrap()
            .near_disabled = false;
    }
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .y = 8.;
    assert_eq!(near, settle(&mut app));
    // The profiling bypass retains the source cache but must render the baked
    // ground, while the same-mesh diagnostic variants must really change shading.
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap()
        .near_disabled = true;
    let bypassed = settle(&mut app);
    assert!(
        bypassed
            .iter()
            .zip(&baked)
            .all(|(a, b)| a.abs_diff(*b) <= 2)
    );
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap()
        .near_disabled = false;
    for mode in [
        TerrainShadingMode::Flat,
        TerrainShadingMode::SingleTexture,
        TerrainShadingMode::SurfaceUnlit,
    ] {
        app.world_mut()
            .resource_mut::<Assets<TerrainCompositeMaterial>>()
            .get_mut(&composite)
            .unwrap()
            .shading_mode = mode;
        assert!(
            settle(&mut app) != near,
            "diagnostic shader did not change {mode:?}"
        );
    }
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap()
        .shading_mode = TerrainShadingMode::Production;
    assert!(
        settle(&mut app) == near,
        "diagnostics changed the production surface"
    );
    let green = |p: &[u8]| {
        p.chunks_exact(4)
            .filter(|p| p[1] > p[0].saturating_add(10))
            .count()
    };
    assert!(
        green(&near) > 10000 && green(&baked) == 0,
        "painted second surface did not reach unchanged mesh"
    );
    // Compare the detailed interior against the original surface shader on exactly
    // the same tangent-bearing mesh; the one-metre loading edge intentionally blends.
    app.world_mut()
        .entity_mut(ground)
        .remove::<MeshMaterial3d<TerrainCompositeMaterial>>()
        .insert(MeshMaterial3d(material.clone()));
    let original = settle(&mut app);
    let mut difference = 0u64;
    for y in 64..192 {
        for x in 64..192 {
            for c in 0..3 {
                let i = (y * 256 + x) * 4 + c;
                difference += original[i].abs_diff(near[i]) as u64;
            }
        }
    }
    assert!(
        difference < 128 * 128 * 3 * 2,
        "detailed shader diverged from original: mean {}",
        difference as f64 / (128. * 128. * 3.)
    );
    app.world_mut()
        .entity_mut(ground)
        .remove::<MeshMaterial3d<TerrainMaterial>>()
        .insert(MeshMaterial3d(composite.clone()));
    // Exercise a sloped mesh too: close normals use its geometry, not the
    // distant bake's deliberately flat normal field.
    let slope = app.world_mut().resource_mut::<Assets<Mesh>>().add(
        build_heightfield_mesh(
            &TerrainHeightfield::from_heights(2, &[-2., 0., 0., 2.], -2., 2., 8.).unwrap(),
            8.,
        )
        .unwrap(),
    );
    app.world_mut().entity_mut(ground).insert(Mesh3d(slope));
    let near_slope = settle(&mut app);
    app.world_mut()
        .entity_mut(ground)
        .remove::<MeshMaterial3d<TerrainCompositeMaterial>>()
        .insert(MeshMaterial3d(material.clone()));
    let original_slope = settle(&mut app);
    let mut difference = 0u64;
    for y in 64..192 {
        for x in 64..192 {
            for c in 0..3 {
                let i = (y * 256 + x) * 4 + c;
                difference += original_slope[i].abs_diff(near_slope[i]) as u64;
            }
        }
    }
    assert!(
        difference < 128 * 128 * 3 * 2,
        "sloped near shading diverged from original"
    );
    app.world_mut()
        .entity_mut(ground)
        .remove::<MeshMaterial3d<TerrainMaterial>>()
        .insert((Mesh3d(mesh), MeshMaterial3d(composite.clone())));
    let _ = settle(&mut app);
    let uploads = app.world().resource::<NearStats>().tile_uploads;
    let _ = settle(&mut app);
    assert_eq!(
        uploads,
        app.world().resource::<NearStats>().tile_uploads,
        "stationary inputs reuploaded"
    );
    app.world_mut()
        .resource_mut::<Assets<TerrainMaterial>>()
        .get_mut(&material)
        .unwrap()
        .settings
        .normal_settings = Vec4::new(1., 0., 1., 0.);
    let flat = settle(&mut app);
    assert_ne!(near, flat, "micro normals had no lighting effect");
    let origin = CellCoord { x: -9, z: 7 };
    let shift = Vec3::new(-72., 0., 56.);
    for e in [camera, ground] {
        app.world_mut().get_mut::<Transform>(e).unwrap().translation -= shift;
    }
    app.world_mut().resource_mut::<NearView>().origin = origin;
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap()
        .set_origin(origin, 8.);
    let rebased = settle(&mut app);
    assert!(
        flat.iter().zip(&rebased).all(|(a, b)| a.abs_diff(*b) <= 2),
        "near pattern moved on rebase"
    );
    let coverage = Image::new(
        Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![255; 8],
        TextureFormat::Rg8Unorm,
        RenderAssetUsages::default(),
    );
    let coverage = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(coverage);
    {
        let mut materials = app.world_mut().resource_mut::<Assets<TerrainMaterial>>();
        let mut m = materials.get_mut(&material).unwrap();
        m.set_canopy_coverage(Some(coverage), Vec4::new(64., -56., 0.125, 0.125));
        m.set_canopy_shading([
            [0.6, 1., 0., 1.],
            [0.5, 0.5, 1., 0.],
            [0., 50., 0., 0.],
            [-72., 56., 0.1, 0.],
        ]);
    }
    let shaded = settle(&mut app);
    let light = |p: &[u8]| {
        p.chunks_exact(4)
            .map(|c| c[0] as u64 + c[1] as u64 + c[2] as u64)
            .sum::<u64>()
    };
    assert!(
        light(&shaded) < light(&rebased) * 9 / 10,
        "canopy coverage did not shade the ground"
    );
    let mut prepared_source = app
        .world()
        .resource::<Assets<TerrainMaterial>>()
        .get(&material)
        .unwrap()
        .clone();
    prepared_source.canopy_coverage = None;
    prepared_source.canopy_shading = default();
    app.world_mut().despawn(entity);
    let retired = settle(&mut app);
    assert!(
        baked.iter().zip(&retired).all(|(a, b)| a.abs_diff(*b) <= 2),
        "removed source left stale near shading"
    );
    assert_eq!(app.world().resource::<NearStats>().pages, 0);
    // Exercise the optional periodic prepared albedo and its phase across another
    // rebase, without needing the development machine's local texture pack.
    app.world_mut().resource_mut::<NearView>().identity = None;
    let mut pack = Pack::from_material(&prepared_source);
    pack.prepared = Some(pack.base.clone());
    pack.period = 4.;
    let hub = app.world().resource::<NearUploadHub>().clone();
    let atlas = app
        .world_mut()
        .resource_scope(|w, mut images: Mut<Assets<Image>>| {
            NearAtlas::new(
                &mut images,
                &mut w.resource_mut::<Assets<ShaderBuffer>>(),
                &hub,
                pack.clone(),
            )
        });
    while !atlas.ready() {
        assert!(std::time::Instant::now() < deadline);
        app.update();
    }
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap()
        .set_near(&atlas);
    let mut table = vec![NearEntry::default(); NEAR_TABLE];
    table[hash(s.key.cell)] = gpu::entry(&s, &prepared_source, &pack, 0, 0.);
    atlas.submit(
        table.clone(),
        vec![gpu::tile(0, &s, &prepared_source, None, origin)],
    );
    let unavailable = settle(&mut app);
    assert_eq!(
        unavailable, retired,
        "unavailable prepared inputs must keep baked shading"
    );
    table[hash(s.key.cell)].state.x = 1.;
    atlas.submit(table, vec![]);
    let prepared = settle(&mut app);
    assert_ne!(prepared, retired);
    let next_origin = CellCoord { x: 13, z: -17 };
    let delta = Vec3::new(
        (next_origin.x - origin.x) as f32 * 8.,
        0.,
        (next_origin.z - origin.z) as f32 * 8.,
    );
    for e in [camera, ground] {
        app.world_mut().get_mut::<Transform>(e).unwrap().translation -= delta;
    }
    app.world_mut()
        .resource_mut::<Assets<TerrainCompositeMaterial>>()
        .get_mut(&composite)
        .unwrap()
        .set_origin(next_origin, 8.);
    let moved = settle(&mut app);
    assert!(
        prepared
            .iter()
            .zip(&moved)
            .all(|(a, b)| a.abs_diff(*b) <= 2),
        "prepared pattern moved after rebasing"
    );
}
