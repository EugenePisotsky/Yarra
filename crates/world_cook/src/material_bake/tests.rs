use super::*;
use crate::{cook_project_with_materials, demo_project_document, road_demo};
use filter::{N, Pixel};
use world_db::{ProjectDocument, RuntimeReader, write_project_database};

struct Fixture {
    dir: std::path::PathBuf,
}
impl Fixture {
    fn new(project: &ProjectDocument) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "yarra-materials-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = Self { dir };
        write_project_database(&f.source(), project).unwrap();
        f
    }
    fn source(&self) -> std::path::PathBuf {
        self.dir.join("source.sqlite")
    }
    fn runtime(&self) -> std::path::PathBuf {
        self.dir.join("runtime.sqlite")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn small_project() -> ProjectDocument {
    let mut p = demo_project_document();
    let keep = |space, cell: CellCoord| {
        space == WorldSpaceId(1) && (-2..2).contains(&cell.x) && (-2..2).contains(&cell.z)
    };
    p.cells.retain(|c| keep(c.space, c.cell));
    p.terrain_cell_heightfields
        .retain(|c| keep(c.space, c.cell));
    p.environment_cells.clear();
    p.objects.clear();
    p
}
fn key(x: i32, z: i32) -> TerrainMaterialKey {
    TerrainMaterialKey(TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord { x, z }))
}

#[test]
fn regional_live_paint_rebuilds_ancestors_and_gutters_without_geometry_or_drift() {
    use std::sync::Arc;
    let project = small_project();
    let fixture = Fixture::new(&project);
    let library = TerrainBakeLibrary::fixture(&project.terrain_texture_sets[0]);
    let report =
        cook_project_with_materials(&fixture.source(), &fixture.runtime(), &library).unwrap();
    let reader = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
    let mut resources = reader
        .read_terrain_resources(PageKey {
            space: WorldSpaceId(1),
            cell: CellCoord { x: -1, z: -1 },
            domain: PageDomain::TerrainRender,
            lod: 0,
        })
        .unwrap();
    let cell = CellCoord { x: -1, z: -1 };
    let mut page = preview::published_terrain_leaf(&reader, WorldSpaceId(1), cell)
        .unwrap()
        .unwrap();
    resources.surfaces = project
        .terrain_surfaces
        .iter()
        .map(|surface| world_db::RuntimeTerrainSurface {
            surface: surface.clone(),
            layer: project
                .terrain_texture_layers
                .iter()
                .find(|l| l.surface == surface.id && l.texture_set == resources.texture_set.id)
                .unwrap()
                .layer,
        })
        .collect();
    let other = resources
        .surfaces
        .iter()
        .find(|s| s.surface.id != page.surfaces[0])
        .unwrap()
        .surface
        .id;
    page.surfaces = vec![other];
    page.weight_pages.clear();
    let leaves = std::collections::BTreeMap::from([(cell, Arc::new(page))]);
    let a = preview::bake_terrain_preview(
        &reader,
        WorldSpaceId(1),
        leaves.clone(),
        &resources,
        &library,
        || false,
    )
    .unwrap();
    assert!(a.nodes.is_empty(), "paint must not rebuild any geometry");
    assert!(a.composites.contains_key(&key(-1, -1)));
    assert!(a.composites.keys().any(|k| k.0.level > 0));
    let b = preview::bake_terrain_preview(
        &reader,
        WorldSpaceId(1),
        leaves,
        &resources,
        &library,
        || false,
    )
    .unwrap();
    assert_eq!(
        a.composites, b.composites,
        "repeated edits must start from published interiors, never accumulate filtering drift"
    );
    for level in 0..TERRAIN_COMPOSITE_MIPS {
        let width = TerrainComposite::mip_size(level);
        let offset = N >> level;
        let overlap = (2 * TERRAIN_COMPOSITE_GUTTER) >> level;
        let left = &a.composites[&key(-1, -1)].mips[level];
        let right = &a.composites[&key(0, -1)].mips[level];
        for (l, r) in [
            (&left.color, &right.color),
            (&left.response, &right.response),
        ] {
            for y in 0..width {
                for x in 0..overlap {
                    assert_eq!(
                        &l[(y * width + offset + x) * 4..(y * width + offset + x + 1) * 4],
                        &r[(y * width + x) * 4..(y * width + x + 1) * 4]
                    );
                }
            }
        }
    }
    assert_eq!(
        reader.manifest().generation_id,
        report.manifest.generation_id
    );
    assert!(
        preview::bake_terrain_preview(
            &reader,
            WorldSpaceId(1),
            a.leaves,
            &resources,
            &library,
            || true
        )
        .is_err()
    );
}

#[test]
fn regional_live_relief_checks_external_borders_and_rebuilds_only_changed_paths() {
    use std::sync::Arc;
    let project = crate::terrain_fixture::mountain_project(2);
    let fixture = Fixture::new(&project);
    let library = TerrainBakeLibrary::fixture(&project.terrain_texture_sets[0]);
    cook_project_with_materials(&fixture.source(), &fixture.runtime(), &library).unwrap();
    let reader = RuntimeReader::open_immutable(&fixture.runtime()).unwrap();
    let resources = reader
        .read_terrain_resources(PageKey {
            space: WorldSpaceId(1),
            cell: CellCoord { x: -1, z: -1 },
            domain: PageDomain::TerrainRender,
            lod: 0,
        })
        .unwrap();
    let cell = CellCoord { x: -1, z: -1 };
    let mut page = preview::published_terrain_leaf(&reader, WorldSpaceId(1), cell)
        .unwrap()
        .unwrap();
    let n = usize::from(page.heightfield.resolution);
    page.heightfield.heights[n * n / 2] += 0.2;
    let p = preview::bake_terrain_preview(
        &reader,
        WorldSpaceId(1),
        [(cell, Arc::new(page.clone()))].into(),
        &resources,
        &library,
        || false,
    )
    .unwrap();
    assert!(p.nodes.contains_key(&key(-1, -1).0));
    assert!(
        p.nodes
            .contains_key(&key(-1, -1).0.parent().unwrap().unwrap())
    );
    assert!(!p.nodes.contains_key(&key(0, 0).0));
    // The +X edge crosses the X=0 coarse-root boundary. A parent check alone
    // would miss this broken seam; the live regional admission must reject it.
    page.heightfield.heights[n - 1] += 1.;
    let error = preview::bake_terrain_preview(
        &reader,
        WorldSpaceId(1),
        [(cell, Arc::new(page))].into(),
        &resources,
        &library,
        || false,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("neighboring relief"),
        "{error:#}"
    );
}
fn synthetic_core(x: i32, z: i32) -> Core {
    let pixels = (0..N * N)
        .map(|i| {
            let x = x as f32 * N as f32 + (i % N) as f32;
            let z = z as f32 * N as f32 + (i / N) as f32;
            Pixel {
                color: [
                    (x * 0.025).sin() * 0.25 + 0.4,
                    (z * 0.03).cos() * 0.3 + 0.4,
                    0.1,
                ],
                normal: filter::normalize([x * 0.002, 1., z * 0.001]),
                roughness: 0.8,
                ao: 1.,
                valid: true,
            }
        })
        .collect();
    Core {
        fingerprint: [1; 32],
        pixels,
    }
}
fn neighbors(x: i32, z: i32) -> [Option<Core>; 9] {
    std::array::from_fn(|i| Some(synthetic_core(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1)))
}

#[test]
fn composites_filter_linear_light_and_vectors_not_encoded_channels() {
    let p = Pixel::mean(&[
        Pixel {
            color: [0.; 3],
            normal: [1., 0., 0.],
            roughness: 0.2,
            ao: 0.5,
            valid: true,
        },
        Pixel {
            color: [1.; 3],
            normal: [0., 1., 0.],
            roughness: 0.8,
            ao: 1.,
            valid: true,
        },
    ]);
    assert_eq!(p.color, [0.5; 3]);
    assert_eq!(inputs::srgb(p.color[0]), 188);
    assert!((p.normal[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    assert_eq!(p.roughness, 0.5);
    assert_eq!(p.ao, 0.75);
}

#[test]
fn neighboring_gutters_match_at_every_mip_in_negative_coordinates() {
    let a = filter::finish(key(-1, -1), &neighbors(-1, -1)).unwrap();
    let b = filter::finish(key(0, -1), &neighbors(0, -1)).unwrap();
    let c = filter::finish(key(-1, 0), &neighbors(-1, 0)).unwrap();
    for l in 0..TERRAIN_COMPOSITE_MIPS {
        let n = TerrainComposite::mip_size(l);
        let offset = N >> l;
        let overlap = (2 * TERRAIN_COMPOSITE_GUTTER) >> l;
        for (a, b, c) in [
            (&a.mips[l].color, &b.mips[l].color, &c.mips[l].color),
            (
                &a.mips[l].response,
                &b.mips[l].response,
                &c.mips[l].response,
            ),
        ] {
            for y in 0..n {
                for x in 0..overlap {
                    assert_eq!(
                        &a[(y * n + offset + x) * 4..(y * n + offset + x + 1) * 4],
                        &b[(y * n + x) * 4..(y * n + x + 1) * 4]
                    );
                    assert_eq!(
                        &a[((offset + x) * n + y) * 4..((offset + x) * n + y + 1) * 4],
                        &c[(x * n + y) * 4..(x * n + y + 1) * 4]
                    );
                }
            }
        }
    }
    let bytes = encode_terrain_composite(&a).unwrap();
    assert_eq!(decode_terrain_composite(&bytes).unwrap(), a);
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(decode_terrain_composite(&trailing).is_err());
    let mut wrong = bytes.clone();
    wrong[0] = 99;
    assert!(decode_terrain_composite(&wrong).is_err());
    assert!(decode_terrain_composite(&bytes[..bytes.len() - 1]).is_err());
}

#[test]
fn parent_filter_preserves_area_and_partial_coverage() {
    let children = std::array::from_fn(|i| {
        if i == 3 {
            None
        } else {
            Some(Core {
                fingerprint: [i as u8; 32],
                pixels: vec![
                    Pixel {
                        color: [i as f32 * 0.25; 3],
                        normal: [0., 1., 0.],
                        roughness: 0.5,
                        ao: 1.,
                        valid: true
                    };
                    N * N
                ],
            })
        }
    });
    let p = filter::parent(&children);
    assert!(!p.pixels[N * N - 1].valid);
    assert_eq!(p.pixels[(N - 1) * N].color, [0.5; 3]);
    let mut halo = std::array::from_fn(|_| None);
    halo[4] = Some(p);
    assert!(
        filter::finish(
            TerrainMaterialKey(TerrainNodeKey {
                level: 1,
                ..key(0, 0).0
            }),
            &halo
        )
        .is_err()
    );
}

#[test]
fn production_cook_is_deterministic_bounded_and_material_changes_leave_geometry_identical() {
    let project = small_project();
    let f = Fixture::new(&project);
    let inputs = TerrainBakeLibrary::fixture(&project.terrain_texture_sets[0]);
    let a = cook_project_with_materials(&f.source(), &f.runtime(), &inputs).unwrap();
    let stats = a.materials.unwrap();
    assert_eq!(stats.tiles, 20);
    assert_eq!(stats.peak_filter_cores, 9);
    assert_eq!(stats.peak_core_pixels, 9 * 64 * 64);
    assert_eq!(stats.tile_gpu_bytes, 54432);
    let reader = RuntimeReader::open_immutable(&f.runtime()).unwrap();
    assert!(reader.has_terrain_composites(WorldSpaceId(1)).unwrap());
    assert!(reader.has_terrain_composites(WorldSpaceId(2)).unwrap()); // declared empty world
    let geometry = reader
        .read_terrain_node(key(0, 0).0)
        .unwrap()
        .unwrap()
        .descriptor
        .checksum;
    let before = reader
        .read_terrain_composite(key(0, 0))
        .unwrap()
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(
        reader
            .read_terrain_composite_descriptors(&[key(0, 0)])
            .unwrap()[0]
            .as_ref()
            .unwrap()
            .gpu_bytes,
        54432
    );
    assert!(
        reader
            .read_terrain_composite_descriptors(&vec![key(0, 0); 129])
            .is_err()
    );
    for r in reader.read_terrain_roots(WorldSpaceId(1)).unwrap() {
        reader
            .read_terrain_composite(TerrainMaterialKey(r.key))
            .unwrap()
            .unwrap()
            .decode()
            .unwrap();
    }
    drop(reader);
    let b = cook_project_with_materials(&f.source(), &f.runtime(), &inputs).unwrap();
    assert_eq!(a.manifest.content_hash, b.manifest.content_hash);
    let mut changed = project.clone();
    for s in &mut changed.terrain_surfaces {
        s.roughness_min = 0.1;
        s.roughness_max = 0.2;
    }
    let source = f.dir.join("changed.sqlite");
    write_project_database(&source, &changed).unwrap();
    let c = cook_project_with_materials(&source, &f.runtime(), &inputs).unwrap();
    assert_ne!(c.manifest.content_hash, b.manifest.content_hash);
    let reader = RuntimeReader::open_immutable(&f.runtime()).unwrap();
    assert_eq!(
        reader
            .read_terrain_node(key(0, 0).0)
            .unwrap()
            .unwrap()
            .descriptor
            .checksum,
        geometry
    );
    let after = reader
        .read_terrain_composite(key(0, 0))
        .unwrap()
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!(before.mips[0].color, after.mips[0].color);
    assert_ne!(before.mips[0].response, after.mips[0].response);
    assert_ne!(before.fingerprint, after.fingerprint);
    drop(reader);
    // Corrupt encoded data and declared allocations are rejected independently.
    let db = rusqlite::Connection::open(f.runtime()).unwrap();
    db.execute("UPDATE terrain_composites SET payload=zeroblob(length(payload)) WHERE level=0 AND node_x=0 AND node_z=0",[]).unwrap();
    drop(db);
    let reader = RuntimeReader::open_immutable(&f.runtime()).unwrap();
    assert!(
        reader
            .read_terrain_composite(key(0, 0))
            .unwrap()
            .unwrap()
            .decode()
            .is_err()
    );
}

#[test]
fn missing_assets_do_not_replace_the_published_database() {
    let project = small_project();
    let f = Fixture::new(&project);
    let inputs = TerrainBakeLibrary::fixture(&project.terrain_texture_sets[0]);
    let old = cook_project_with_materials(&f.source(), &f.runtime(), &inputs)
        .unwrap()
        .manifest;
    let mut changed = project.clone();
    changed.terrain_texture_sets[0].base_color_universal_uri = "missing.ktx2".into();
    let source = f.dir.join("missing.sqlite");
    write_project_database(&source, &changed).unwrap();
    assert!(cook_project_with_materials(&source, &f.runtime(), &inputs).is_err());
    assert_eq!(
        RuntimeReader::open_immutable(&f.runtime())
            .unwrap()
            .manifest()
            .generation_id,
        old.generation_id
    );
    assert!(std::fs::read_dir(&f.dir).unwrap().all(|p| {
        !p.unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".building")
    }));
}

#[test]
fn road_composite_uses_final_ground_weights_and_keeps_relief_out_of_albedo() {
    let project = road_demo::document();
    let inputs = TerrainBakeLibrary::fixture(&project.terrain_texture_sets[0]);
    let f = Fixture::new(&project);
    // Reference pack only needs one road cell to test its actual mixed material evaluator.
    let build = crate::build_runtime(project).unwrap();
    world_db::write_runtime_database(&f.runtime(), &build).unwrap();
    let reader = RuntimeReader::open_immutable(&f.runtime()).unwrap();
    let mut found = false;
    for p in build
        .pages
        .iter()
        .filter(|p| p.key.domain == PageDomain::TerrainRender && p.key.lod == 0)
    {
        let PagePayload::TerrainHeightfield(mut page) = p.clone().decode().unwrap().payload else {
            continue;
        };
        if page.surfaces.len() != 2 {
            continue;
        }
        let resources = reader.read_terrain_resources(p.key).unwrap();
        let core = evaluate::leaf(
            TerrainMaterialKey(TerrainNodeKey::leaf(p.key.space, p.key.cell)),
            8.,
            &page,
            &resources,
            inputs.get(&resources.texture_set).unwrap(),
        )
        .unwrap();
        let low = core
            .pixels
            .iter()
            .map(|p| p.color[0])
            .fold(f32::INFINITY, f32::min);
        let high = core.pixels.iter().map(|p| p.color[0]).fold(0., f32::max);
        if high - low < 0.02 {
            continue;
        }
        for height in &mut page.heightfield.heights {
            *height += 100.;
        }
        let shifted = evaluate::leaf(
            TerrainMaterialKey(TerrainNodeKey::leaf(p.key.space, p.key.cell)),
            8.,
            &page,
            &resources,
            inputs.get(&resources.texture_set).unwrap(),
        )
        .unwrap();
        assert_eq!(core.fingerprint, shifted.fingerprint);
        assert_eq!(core.encode().unwrap(), shifted.encode().unwrap());
        found = true;
        break;
    }
    assert!(found, "no mixed road ground found");
}

#[test]
fn incomplete_material_pass_rolls_back_and_decode_checks_declared_bytes() {
    let project = small_project();
    let f = Fixture::new(&project);
    let before = crate::cook_project_with_report(&f.source(), &f.runtime())
        .unwrap()
        .manifest;
    {
        let store = TerrainMaterialCookStore::open_staging(&f.runtime()).unwrap();
        let tile = filter::finish(key(0, 0), &neighbors(0, 0)).unwrap();
        store.insert(&tile).unwrap();
        let mut encoded = store
            .reader()
            .read_terrain_composite(key(0, 0))
            .unwrap()
            .unwrap();
        encoded.descriptor.decoded_bytes = MAX_TERRAIN_COMPOSITE_BYTES as u64 + 1;
        assert!(encoded.decode().is_err());
        assert!(store.finish().is_err());
    }
    let reader = RuntimeReader::open_immutable(&f.runtime()).unwrap();
    assert_eq!(reader.manifest().content_hash, before.content_hash);
    assert!(!reader.has_terrain_composites(WorldSpaceId(1)).unwrap());
    assert!(reader.read_terrain_composite(key(0, 0)).unwrap().is_none());
}
