mod cell_cook;
mod control_cook;
mod environment_cook;
mod parallel;
mod streaming_cook;
mod terrain_cook;
mod terrain_preview;

pub use terrain_preview::{
    GroundLayers, MAX_PREVIEW_LEAVES, preview_terrain, published_terrain_control,
    published_terrain_leaf,
};

#[cfg(test)]
mod terrain_fixture;

use environment_cook::demo_environment;

pub use streaming_cook::{CookReport, CookStats, cook_project_fresh, cook_project_with_report};

mod heightfield_import;
mod terrain_world;

pub use heightfield_import::{HeightfieldImportReport, import_heightfield};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
    sync::OnceLock,
};

use anyhow::{Context, Result, bail};
use glam::Vec2;

use world::{
    AssetId, CellCoord, DEFAULT_CELL_SIZE, GameplayObjectInstance, GameplayObjectsPage,
    MAX_TERRAIN_SURFACES_PER_CELL, MAX_TERRAIN_WEIGHT_RESOLUTION, ObjectActivationPolicy,
    ObjectDefinitionId, PageCodec, PageDomain, PageKey, PagePayload, RUNTIME_SCHEMA_VERSION,
    StableObjectId, StaticObjectInstance, StaticObjectsPage, TerrainHeightfieldPage,
    TerrainProfile, TerrainSurface, TerrainSurfaceId, TerrainTextureSet, TerrainTextureSetId,
    WorldSpaceId, encode_page_payload,
};

use world_db::{
    AssetVariantRecord, EncodedPage, ProjectDocument, RuntimeCell, RuntimeCellRecord,
    RuntimeHeader, RuntimeManifest, RuntimeObjectDefinition, SourceAssetRecord,
    SourceAssetVariantRecord, SourceCellRecord, SourceObjectDefinitionRecord, SourceObjectRecord,
    SourceTerrainCellHeightfieldRecord, WorldSpaceRecord, domain_bit, write_project_database,
};

/// The demo world's tree, from its tracked pack catalog: the smallest leafy tree of the current
/// kit, three mesh LODs and an impostor, as the island's trees are drawn. Its runtime files live
/// under `assets/local/yarra_bay`.
const DEMO_TREE_KEY: &str = "yarra_bay/bay_upright";

const DEMO_TREE_CATALOG: &str = include_str!("../../../assets/packs/yarra_bay/bay.catalog.ron");
const DEMO_WORLD_CELL_RANGE: std::ops::Range<i32> = -8..8;
// Half-metre source masks and endpoint-inclusive compiled ground weights.
const DEMO_TERRAIN_WEIGHT_RESOLUTION: u16 = 65;
const DEMO_TERRAIN_HEIGHTFIELD_RESOLUTION: u16 = 33;
const DEMO_TERRAIN_MINIMUM_HEIGHT: f32 = -4.0;
const DEMO_TERRAIN_MAXIMUM_HEIGHT: f32 = 4.0;

mod grass_pack;
mod road_demo;
mod road_import;
mod terrain_pack;

pub use road_demo::create_road_demo_project;
use terrain_pack::TerrainPack;

pub use road_import::{RoadImportReport, import_roads};

/// The catalog of grass pack `text` for the terrain pack worlds are created with.
pub fn grass_catalog(text: &str) -> Result<vegetation::VegetationCatalog> {
    grass_pack::catalog(text, &TerrainPack::baltic()).map_err(anyhow::Error::msg)
}

pub fn create_demo_project(path: &Path) -> Result<()> {
    let document = demo_project_document();
    write_project_database(path, &document)
        .with_context(|| format!("failed to create demo project at {}", path.display()))
}

pub fn cook_project(project_path: &Path, runtime_path: &Path) -> Result<RuntimeManifest> {
    Ok(cook_project_with_report(project_path, runtime_path)?.manifest)
}

/// Cooks a project held in memory as its database would be cooked: written to `directory`
/// as `project.sqlite` and cooked afresh to `runtime.sqlite` beside it, whose path returns.
#[cfg(test)]
fn cook_document(document: &ProjectDocument, directory: &Path) -> Result<std::path::PathBuf> {
    fs::create_dir_all(directory)?;
    let project = directory.join("project.sqlite");
    let runtime = directory.join("runtime.sqlite");
    write_project_database(&project, document)?;
    cook_project_fresh(&project, &runtime)?;
    Ok(runtime)
}

/// A directory under the system's temporary one, removed with this value.
#[cfg(test)]
struct TestDirectory(std::path::PathBuf);

#[cfg(test)]
impl TestDirectory {
    fn new(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "yarra-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// `document` cooked in a directory of its own here, opened.
    fn cook(&self, document: &ProjectDocument) -> world_db::RuntimeReader {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = self.0.join(
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .to_string(),
        );
        let runtime = cook_document(document, &directory).unwrap();
        world_db::RuntimeReader::open_immutable(&runtime).unwrap()
    }
}

#[cfg(test)]
impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Every page of a runtime's world, decoded, in cell order; for tests.
#[cfg(test)]
fn decoded_pages(
    reader: &world_db::RuntimeReader,
    space: WorldSpaceId,
) -> Vec<world_db::DecodedPage> {
    reader
        .terrain_cells(space)
        .unwrap()
        .into_iter()
        .flat_map(|cell| {
            PageDomain::ALL.map(|domain| PageKey {
                space,
                cell,
                domain,
            })
        })
        .filter_map(|key| reader.read_page(key).unwrap())
        .map(|page| page.decode().unwrap())
        .collect()
}

struct RuntimeStaging {
    path: std::path::PathBuf,
}

impl RuntimeStaging {
    fn new(runtime_path: &Path) -> Result<Self> {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = runtime_path
            .file_name()
            .and_then(|s| s.to_str())
            .context("runtime database path must have a UTF-8 file name")?;
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        Ok(Self {
            path: runtime_path.with_file_name(format!(
                ".{name}.{}-{nanos}-{sequence}.building",
                std::process::id()
            )),
        })
    }
}

impl Drop for RuntimeStaging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let mut journal = self.path.as_os_str().to_os_string();
        journal.push("-journal");
        let _ = fs::remove_file(std::path::PathBuf::from(journal));
    }
}

/// What an incremental cook changed: the cells it cooked again, and those of them whose ground
/// page changed.
struct Changes {
    cells: terrain_cook::ChangedCells,
    ground: terrain_cook::ChangedCells,
}

/// Builds the staged runtime's terrain hierarchy and the control tiles above its cells (only
/// above changed cells when given), validates its roots and publishes it; also returns the
/// hierarchy's seconds.
fn publish_staged_runtime(
    runtime_path: &Path,
    temporary_path: &Path,
    spaces: &[WorldSpaceRecord],
    changes: Option<&Changes>,
) -> Result<(RuntimeManifest, f64)> {
    let start = std::time::Instant::now();
    let manifest =
        terrain_cook::cook_hierarchy(temporary_path, spaces, changes.map(|c| &c.ground))?;
    let hierarchy_seconds = start.elapsed().as_secs_f64();
    control_cook::cook(temporary_path, spaces, changes.map(|c| &c.cells))?;
    let reader = world_db::RuntimeReader::open_immutable(temporary_path)
        .context("cooked runtime database did not pass validation")?;

    for space in &manifest.world_spaces {
        // The roots are the minimum fallback cover: validate their actual payloads
        // before replacing a previously working generation.
        for root in reader.read_terrain_roots(space.id)? {
            reader
                .read_terrain_node(root.key)?
                .context("missing cooked terrain root")?
                .decode()?;
        }
    }

    drop(reader);
    fs::rename(temporary_path, runtime_path).with_context(|| {
        format!(
            "failed to publish runtime database {}",
            runtime_path.display()
        )
    })?;
    Ok((manifest, hierarchy_seconds))
}

fn demo_tree() -> world_db::AssetImport {
    let catalog: world_db::AssetImportCatalog =
        ron::from_str(DEMO_TREE_CATALOG).expect("the tracked tree catalog parses");
    catalog
        .assets
        .into_iter()
        .find(|asset| asset.key == DEMO_TREE_KEY)
        .expect("the demo tree is in its catalog")
}

fn demo_project_document() -> ProjectDocument {
    let overworld = WorldSpaceRecord {
        atmosphere: Default::default(),
        atmosphere_revision: 1,
        id: WorldSpaceId(1),
        name: "demo-overworld".into(),
        cell_size: DEFAULT_CELL_SIZE,
        minimum_y: DEMO_TERRAIN_MINIMUM_HEIGHT,
        maximum_y: DEMO_TERRAIN_MAXIMUM_HEIGHT,
        sea_level: None,
    };
    let interior = WorldSpaceRecord {
        atmosphere: world::atmosphere::AtmosphereProfile {
            outdoor: false,
            ..Default::default()
        },
        atmosphere_revision: 1,
        id: WorldSpaceId(2),
        name: "demo-interior".into(),
        cell_size: DEFAULT_CELL_SIZE,
        minimum_y: 0.0,
        maximum_y: 0.0,
        sea_level: None,
    };
    let overworld_id = overworld.id;
    let interior_id = interior.id;
    let pack = TerrainPack::baltic();
    let terrain_texture_set = pack.texture_set.id;
    let meadow = pack.surface("meadow");
    let dry_meadow = pack.surface("dry-meadow");
    let tree = demo_tree();
    let tree_asset = AssetId(*blake3::hash(tree.key.as_bytes()).as_bytes());
    let tree_definition = definition_id("demo-tree");
    let proximity_marker_definition = definition_id("demo-proximity-marker");
    let mut cells = Vec::new();
    let mut objects = Vec::new();
    let mut terrain_cell_heightfields = Vec::new();

    for x in DEMO_WORLD_CELL_RANGE {
        for z in DEMO_WORLD_CELL_RANGE {
            cells.push(SourceCellRecord {
                space: overworld.id,
                cell: CellCoord { x, z },
                height: 0.0,
                source_revision: 1,
            });
            terrain_cell_heightfields.push(SourceTerrainCellHeightfieldRecord {
                space: overworld.id,
                cell: CellCoord { x, z },
                resolution: DEMO_TERRAIN_HEIGHTFIELD_RESOLUTION,
                heights: demo_terrain_heights(CellCoord { x, z }),
                source_revision: 1,
            });
            let lod_test_line = x == z && (-3..=-1).contains(&x);

            if lod_test_line || (x.rem_euclid(5) == 2 && z.rem_euclid(5) == 2) {
                let hash = blake3::hash(format!("demo-tree:{x}:{z}").as_bytes());
                let mut object_id = [0; 16];
                object_id.copy_from_slice(&hash.as_bytes()[..16]);
                objects.push(SourceObjectRecord {
                    id: StableObjectId(object_id),
                    space: overworld.id,
                    owner_cell: CellCoord { x, z },
                    definition: tree_definition,
                    local_translation: [
                        DEFAULT_CELL_SIZE * 0.5,
                        demo_terrain_height(Vec2::new(
                            (x as f32 + 0.5) * DEFAULT_CELL_SIZE,
                            (z as f32 + 0.5) * DEFAULT_CELL_SIZE,
                        )),
                        DEFAULT_CELL_SIZE * 0.5,
                    ],
                    yaw: ((x * 17 + z * 31).rem_euclid(360) as f32).to_radians(),
                    scale: 1.0,
                    source_revision: 1,
                });
            }
        }
    }

    for x in -4_i32..=4 {
        for z in -4_i32..=4 {
            cells.push(SourceCellRecord {
                space: interior.id,
                cell: CellCoord { x, z },
                height: 0.0,
                source_revision: 1,
            });
        }
    }

    objects.push(SourceObjectRecord {
        id: StableObjectId(proximity_marker_definition.0),
        space: interior.id,
        owner_cell: CellCoord::ZERO,
        definition: proximity_marker_definition,
        local_translation: [DEFAULT_CELL_SIZE * 0.5, 0.0, DEFAULT_CELL_SIZE * 0.5],
        yaw: 0.0,
        scale: 1.0,
        source_revision: 1,
    });

    let (presets, environments, environment_cells) =
        demo_environment(overworld_id, interior_id, &pack, meadow, dry_meadow);
    ProjectDocument {
        default_world_space: overworld.id,
        start_view: None,
        gameplay_areas: Default::default(),
        world_spaces: vec![overworld, interior],
        vegetation_catalog: Some(grass_pack::baltic(&pack)),
        cells,
        terrain_surfaces: pack.surfaces,
        terrain_texture_sets: vec![pack.texture_set],
        terrain_texture_layers: pack.layers,
        terrain_profiles: vec![
            TerrainProfile {
                space: overworld_id,
                texture_set: terrain_texture_set,
                weight_resolution: DEMO_TERRAIN_WEIGHT_RESOLUTION,
                macro_scales: [7.7, 31.5, 235.0],
                macro_contrast: 2.5,
                macro_albedo_strength: 0.395,
            },
            TerrainProfile {
                space: interior_id,
                texture_set: terrain_texture_set,
                weight_resolution: DEMO_TERRAIN_WEIGHT_RESOLUTION,
                macro_scales: [7.7, 31.5, 235.0],
                macro_contrast: 2.5,
                macro_albedo_strength: 0.395,
            },
        ],
        presets,
        environments,
        environment_cells,
        roads: Default::default(),
        terrain_cell_heightfields,
        asset_variants: tree
            .variants
            .iter()
            .enumerate()
            .map(|(lod, variant)| SourceAssetVariantRecord {
                asset: tree_asset,
                lod: lod as u8,
                uri: variant.uri.clone(),
                bounds: variant.bounds,
                gpu_bytes_estimate: variant.gpu_bytes_estimate,
                shadow_policy: 1,
                minimum_screen_height: variant.minimum_screen_height,
            })
            .collect(),
        assets: vec![SourceAssetRecord {
            id: tree_asset,
            key: tree.key,
            kind: "gltf-scene".into(),
            source_uri: tree.source_uri,
        }],
        definitions: vec![
            SourceObjectDefinitionRecord {
                id: tree_definition,
                key: "demo-tree".into(),
                display_name: "Demo tree".into(),
                visual_asset: Some(tree_asset),
                activation: ObjectActivationPolicy::RenderOnly,
            },
            SourceObjectDefinitionRecord {
                id: proximity_marker_definition,
                key: "demo-proximity-marker".into(),
                display_name: "Proximity gameplay marker".into(),
                visual_asset: None,
                activation: ObjectActivationPolicy::Proximity,
            },
        ],
        objects,
    }
}

fn demo_terrain_heights(cell: CellCoord) -> Vec<f32> {
    let resolution = usize::from(DEMO_TERRAIN_HEIGHTFIELD_RESOLUTION);
    let intervals = (resolution - 1) as f32;
    let mut heights = Vec::with_capacity(resolution * resolution);

    for z in 0..resolution {
        for x in 0..resolution {
            let world = Vec2::new(
                cell.x as f32 * DEFAULT_CELL_SIZE + x as f32 * DEFAULT_CELL_SIZE / intervals,
                cell.z as f32 * DEFAULT_CELL_SIZE + z as f32 * DEFAULT_CELL_SIZE / intervals,
            );
            heights.push(demo_terrain_height(world));
        }
    }

    heights
}

/// A deliberately gentle, continuous relief fixture rather than a production landscape tool.
///
/// The small level area around the origin keeps the current non-physical character controller
/// usable. Beyond it, two coherent scales expose page seams, surface conformance, silhouettes,
/// interaction direction, and shadow behaviour without producing extreme slopes.
fn demo_terrain_height(world: Vec2) -> f32 {
    const RELIEF_SEED: u32 = 0x7a31_c5d9;
    let broad = terrain_fbm(
        world / 92.0 + terrain_material_offset(RELIEF_SEED, 0),
        RELIEF_SEED,
    ) * 2.35;
    let medium = terrain_fbm(
        world / 34.0 + terrain_material_offset(RELIEF_SEED, 1),
        RELIEF_SEED ^ 0x6a09_e667,
    ) * 0.62;
    let origin_plateau = smoothstep(6.0, 24.0, world.length());
    ((broad + medium) * origin_plateau).clamp(
        DEMO_TERRAIN_MINIMUM_HEIGHT + 0.25,
        DEMO_TERRAIN_MAXIMUM_HEIGHT - 0.25,
    )
}

const DEMO_TERRAIN_SEED: u32 = 1_863_996_882;
const DEMO_TERRAIN_MIX_SCALE: f32 = 8.7;
const DEMO_TERRAIN_DETAIL_SIZE: f32 = 0.5;
const DEMO_TERRAIN_FINE_VARIATION: f32 = 0.82;
const DEMO_TERRAIN_VARIANT_MOTTLING: f32 = 0.71;
const DEMO_TERRAIN_BLEND_SOFTNESS: f32 = 0.14;
const DEMO_DRY_COVERAGE: f32 = 0.32 / (0.62 + 0.32);

fn demo_terrain_mix_signal(world: Vec2) -> f32 {
    let region_scale = DEMO_TERRAIN_MIX_SCALE;
    let broad_offset = terrain_material_offset(DEMO_TERRAIN_SEED, 1);
    let warp = Vec2::new(
        terrain_fbm(
            world / (region_scale * 3.2) + broad_offset,
            DEMO_TERRAIN_SEED ^ 0x3c6e_f372,
        ),
        terrain_fbm(
            world / (region_scale * 3.2) + broad_offset + Vec2::new(17.3, -29.1),
            DEMO_TERRAIN_SEED ^ 0xbb67_ae85,
        ),
    ) * (region_scale * 0.58);
    let warped_world = world + warp;
    let broad = terrain_patch_signal(
        warped_world / region_scale + broad_offset,
        DEMO_TERRAIN_SEED ^ 0x679d_443f,
    );
    let medium = terrain_patch_signal(
        warped_world / (region_scale * 0.55 * DEMO_TERRAIN_DETAIL_SIZE)
            + terrain_material_offset(DEMO_TERRAIN_SEED, 2),
        DEMO_TERRAIN_SEED ^ 0x243f_6a88,
    );
    let small = terrain_patch_signal(
        (world + warp * 0.35) / (region_scale * 0.28 * DEMO_TERRAIN_DETAIL_SIZE)
            + terrain_material_offset(DEMO_TERRAIN_SEED, 3),
        DEMO_TERRAIN_SEED ^ 0xb7e1_5163,
    );
    let hole_field = terrain_patch_signal(
        (world - warp * 0.2) / (region_scale * 0.36 * DEMO_TERRAIN_DETAIL_SIZE)
            + terrain_material_offset(DEMO_TERRAIN_SEED, 4),
        DEMO_TERRAIN_SEED ^ 0x1319_8a2e,
    );
    let detail = smootherstep(DEMO_TERRAIN_FINE_VARIATION);
    let medium_strength = 0.65 * detail;
    let small_strength = 0.35 * detail;
    let variation = (broad + medium * medium_strength + small * small_strength)
        / (1.0 + medium_strength + small_strength);
    let green_mottling =
        (smoothstep(0.05, 0.75, hole_field) - 0.35) * DEMO_TERRAIN_VARIANT_MOTTLING * 0.42;

    // Flat terrain has the same neutral feature response as the legacy
    // sampler: moisture 0.65, slope 0.0.
    let flat_terrain_bias = (0.65 - 0.5) * -0.55 * 0.2 + (0.0 - 0.35) * 0.2 * 0.14;
    variation - green_mottling + flat_terrain_bias
}

fn demo_terrain_mix_amplitude() -> f32 {
    const HARD_AMPLITUDE: f32 = 3.0;
    const SOFT_AMPLITUDE: f32 = 0.10;
    HARD_AMPLITUDE
        * (SOFT_AMPLITUDE / HARD_AMPLITUDE).powf(DEMO_TERRAIN_BLEND_SOFTNESS.clamp(0.0, 1.0))
}

fn demo_terrain_mix_offset() -> f32 {
    static OFFSET: OnceLock<f32> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        // The original compiler calibrates coverage over the complete area.
        // A deterministic stratified grid is sufficient for this flat demo
        // and avoids making each cell choose its own incompatible threshold.
        const SIDE: usize = 192;
        let extent = DEFAULT_CELL_SIZE * 60.0;
        let minimum = -extent * 0.5;
        let signals = (0..SIDE)
            .flat_map(|z| {
                (0..SIDE).map(move |x| {
                    let world = Vec2::new(
                        minimum + (x as f32 + 0.5) / SIDE as f32 * extent,
                        minimum + (z as f32 + 0.5) / SIDE as f32 * extent,
                    );
                    demo_terrain_mix_signal(world)
                })
            })
            .collect::<Vec<_>>();
        let amplitude = demo_terrain_mix_amplitude();
        let mut lower = -(amplitude * 2.0 + 1.0);
        let mut upper = amplitude * 2.0 + 1.0;

        for _ in 0..24 {
            let offset = (lower + upper) * 0.5;
            let average = signals
                .iter()
                .map(|signal| (offset + signal * amplitude).clamp(0.0, 1.0))
                .sum::<f32>()
                / signals.len() as f32;

            if average < DEMO_DRY_COVERAGE {
                lower = offset;
            } else {
                upper = offset;
            }
        }

        (lower + upper) * 0.5
    })
}

fn terrain_patch_signal(mut point: Vec2, seed: u32) -> f32 {
    let mut signal = terrain_value_noise(point, seed) * 0.7;
    point = Vec2::new(
        point.x * 1.71 - point.y * 1.04 + 7.3,
        point.x * 1.04 + point.y * 1.71 - 11.9,
    );
    signal += terrain_value_noise(point, seed ^ 0x510e_527f) * 0.22;
    point = Vec2::new(
        point.x * 1.53 + point.y * 1.29 - 19.7,
        -point.x * 1.29 + point.y * 1.53 + 3.1,
    );
    signal + terrain_value_noise(point, seed ^ 0x9b05_688c) * 0.08
}

fn terrain_material_offset(seed: u32, index: usize) -> Vec2 {
    Vec2::new(
        terrain_hash01(index as i32, 17, seed ^ 0xa409_3822) * 97.0,
        terrain_hash01(index as i32, 53, seed ^ 0x299f_31d0) * 97.0,
    )
}

fn terrain_value_noise(point: Vec2, seed: u32) -> f32 {
    let cell = point.floor();
    let local = point - cell;
    let fade = Vec2::new(smootherstep(local.x), smootherstep(local.y));
    let x = cell.x as i32;
    let z = cell.y as i32;
    let lower = mix(
        terrain_hash_signed(x, z, seed),
        terrain_hash_signed(x + 1, z, seed),
        fade.x,
    );
    let upper = mix(
        terrain_hash_signed(x, z + 1, seed),
        terrain_hash_signed(x + 1, z + 1, seed),
        fade.x,
    );
    mix(lower, upper, fade.y)
}

fn terrain_fbm(mut point: Vec2, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 0.55;
    let mut normalization = 0.0;

    for octave in 0..5_u32 {
        sum += terrain_value_noise(point, seed ^ octave.wrapping_mul(0x9e37_79b9)) * amplitude;
        normalization += amplitude;
        point = Vec2::new(
            point.x * 1.76 - point.y * 1.13 + 13.1,
            point.x * 1.13 + point.y * 1.76 - 7.9,
        );
        amplitude *= 0.5;
    }

    sum / normalization
}

fn terrain_hash_signed(x: i32, z: i32, seed: u32) -> f32 {
    terrain_hash01(x, z, seed) * 2.0 - 1.0
}

fn terrain_hash01(x: i32, z: i32, seed: u32) -> f32 {
    let mut value =
        seed ^ (x as u32).wrapping_mul(0x9e37_79b1) ^ (z as u32).wrapping_mul(0x85eb_ca77);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^= value >> 16;
    value as f32 / u32::MAX as f32
}

fn smootherstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

fn mix(a: f32, b: f32, amount: f32) -> f32 {
    a + (b - a) * amount
}

fn smoothstep(minimum: f32, maximum: f32, value: f32) -> f32 {
    let normalized = ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0);
    normalized * normalized * (3.0 - 2.0 * normalized)
}

fn definition_id(key: &str) -> ObjectDefinitionId {
    ObjectDefinitionId(stable_id(key))
}

fn stable_id(key: &str) -> [u8; 16] {
    let hash = blake3::hash(key.as_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hash.as_bytes()[..16]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_cook_is_large_logically_but_page_addressable() {
        let project = demo_project_document();
        assert_eq!(project.environments.len(), 2);
        assert_eq!(project.terrain_cell_heightfields.len(), 16 * 16);
        let directory = TestDirectory::new("demo-cook");
        let reader = directory.cook(&project);
        assert_eq!(reader.manifest().world_spaces.len(), 2);
        assert_eq!(reader.manifest().default_world_space, WorldSpaceId(1));
        let cells = |space| reader.terrain_cells(space).unwrap().len();
        assert_eq!(cells(WorldSpaceId(1)), 16 * 16);
        assert_eq!(cells(WorldSpaceId(2)), 9 * 9);
        let overworld = decoded_pages(&reader, WorldSpaceId(1));
        let interior = decoded_pages(&reader, WorldSpaceId(2));
        let pages: Vec<_> = overworld.iter().chain(&interior).collect();
        assert!(pages.len() > 16 * 16 + 9 * 9);

        // Every static-object page names each of the tree's LODs.
        for page in pages
            .iter()
            .filter(|page| page.key.domain == PageDomain::StaticObjects)
        {
            let lods: Vec<_> = reader
                .read_dependencies(page.key)
                .unwrap()
                .iter()
                .map(|d| d.asset_lod)
                .collect();
            assert_eq!(lods, [0, 1, 2, 3]);
        }

        let bounds = reader
            .read_cell_descriptors(
                WorldSpaceId(1),
                CellCoord { x: -8, z: -8 },
                CellCoord { x: 7, z: 7 },
            )
            .unwrap();
        assert!(
            bounds.iter().any(|cell| cell.maximum_y > 8.0),
            "static model bounds did not expand cell visibility bounds"
        );
        let terrain_page = |cell| {
            let page = overworld
                .iter()
                .find(|page| page.key.cell == cell && page.key.domain == PageDomain::Terrain)
                .unwrap();
            let PagePayload::TerrainHeightfield(page) = &page.payload else {
                panic!("demo overworld terrain must cook to heightfields");
            };
            page.clone()
        };
        let left = terrain_page(CellCoord::ZERO);
        let right = terrain_page(CellCoord { x: 1, z: 0 });
        let resolution = usize::from(left.heightfield.resolution);

        for z in 0..resolution {
            let left_index = z * resolution + resolution - 1;
            let right_index = z * resolution;
            assert_eq!(
                left.heightfield.heights[left_index],
                right.heightfield.heights[right_index]
            );
            assert_eq!(
                left.heightfield.normals_oct[left_index],
                right.heightfield.normals_oct[right_index]
            );
        }

        // Grass grows on every cell's ground: the overworld's 16×16 and the interior's 9×9.
        let vegetation: Vec<_> = pages
            .iter()
            .filter(|page| page.key.domain == PageDomain::Vegetation)
            .collect();
        assert!(!vegetation.is_empty());
        assert!(vegetation.len() <= 16 * 16 + 9 * 9);
        assert_eq!(vegetation[0].key.cell, CellCoord { x: -8, z: -8 });
        let PagePayload::Vegetation(page) = &vegetation[0].payload else {
            panic!("vegetation page decoded to the wrong domain");
        };
        let catalog = reader.manifest().vegetation_catalog.as_ref().unwrap();
        page.validate(catalog).unwrap();
        assert!(!page.fields.is_empty());
        assert!(page.fields.len() <= catalog.grass.len() + catalog.details.len());
        let gameplay: Vec<_> = pages
            .iter()
            .filter(|page| page.key.domain == PageDomain::GameplayObjects)
            .collect();
        assert_eq!(gameplay.len(), 1);
        assert_eq!(
            reader
                .read_object_definitions(gameplay[0].key)
                .unwrap()
                .len(),
            1
        );
        assert!(pages.iter().all(|page| page.decoded_bytes > 0));
    }

    #[test]
    fn tracked_pack_catalogs_import() {
        let packs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/packs");
        let mut checked = 0;

        for pack in fs::read_dir(packs).unwrap() {
            let pack = pack.unwrap().path();

            if !pack.is_dir() {
                continue;
            }

            for file in fs::read_dir(pack).unwrap() {
                let path = file.unwrap().path();

                if !path.to_string_lossy().ends_with(".catalog.ron") {
                    continue;
                }

                let catalog: world_db::AssetImportCatalog =
                    ron::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                catalog
                    .validate()
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                checked += 1;
            }
        }

        assert!(checked > 0);
    }

    #[test]
    fn demo_tree_has_mesh_lods_and_an_impostor() {
        let tree = demo_tree();
        let (impostor, meshes) = tree.variants.split_last().unwrap();
        assert!(world::is_impostor_uri(&impostor.uri));
        assert_eq!(meshes.len(), 3);
        assert!(meshes.iter().all(|v| v.uri.ends_with(".gltf")));
    }
}
