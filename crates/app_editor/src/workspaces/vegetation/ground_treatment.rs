//! Static material experiment, compiled only into the editor. Uses the production terrain
//! shader and a coverage bake from source roots, independent of camera, wind and render LOD.
use super::*;
use bevy::{
    asset::{RenderAssetUsages, uuid_handle},
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    pbr::{ExtendedMaterial, MaterialExtension},
    render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension},
    shader::{Shader, ShaderRef},
};
use terrain_render::TerrainMaterial;
use vegetation::{candidate_density_retention, candidate_domain, random01, sample_candidate};

const SHADER: Handle<Shader> = uuid_handle!("08d0e6d7-04e5-4197-92ca-27eb09d40b4e");
const TERRAIN: &str = include_str!("../../../../../assets/shaders/terrain_material.wgsl");
const DETAIL_SOURCE: &str =
    "assets/local/terrain/temperate_meadow/source/uncut_grass_oilpt20/normal_material.png";
// Calibrated against the accepted 44 roots/m² specimen, not against its render LOD.
const ROOTS_FOR_FULL_COVERAGE: f32 = 44.0;
const SMOOTH_RADIUS_METRES: f32 = 0.22;
pub(super) type StudyMaterial = ExtendedMaterial<TerrainMaterial, GroundTreatment>;

#[derive(Clone, Copy, Debug, ShaderType)]
pub(super) struct TreatmentSettings {
    bounds: Vec4,
    controls: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub(super) struct GroundTreatment {
    #[uniform(100)]
    settings: TreatmentSettings,
    #[texture(101)]
    #[sampler(102)]
    coverage: Handle<Image>,
    #[texture(103)]
    #[sampler(104)]
    detail: Handle<Image>,
}

impl MaterialExtension for GroundTreatment {
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

#[derive(Resource, Default)]
pub(super) struct TreatmentAssets {
    pub links: Vec<(Handle<TerrainMaterial>, Handle<StudyMaterial>)>,
    coverage: Handle<Image>,
    detail: Handle<Image>,
    mean: f32,
    revision: Option<u64>,
    pub error: Option<String>,
    pub stats: String,
    mask: Option<CoverageBake>,
    rebuilds: u32,
    texture_bytes: usize,
}

fn shader_source() -> String {
    let hook = "#ifdef TERRAIN_SURFACE_UNLIT\n    // Retain the production albedo blend";
    assert_eq!(
        TERRAIN.matches(hook).count(),
        1,
        "terrain material hook changed"
    );
    let source = TERRAIN.replace(
        hook,
        r#"
    if study_ground.controls.x > 0.5 {
        let coverage = study_ground_coverage(in.world_position.xz);
        if study_ground.controls.x > 2.5 {
            out.color = vec4(vec3(coverage), 1.0);
            return out;
        }
        base = vec4(base.rgb * study_ground_multiplier(in.world_position.xz, coverage), 1.0);
    }
#ifdef TERRAIN_SURFACE_UNLIT
    // Retain the production albedo blend"#,
    );
    format!("{source}\n{}", include_str!("ground_treatment.wgsl"))
}

pub(super) fn register(app: &mut App) {
    app.add_plugins(MaterialPlugin::<StudyMaterial>::default())
        .init_resource::<TreatmentAssets>()
        .add_systems(Update, sync_base_materials)
        .add_systems(
            Update,
            sync.after(super::sync)
                .run_if(in_state(EditorWorkspace::Vegetation)),
        );
    app.world_mut()
        .resource_mut::<Assets<Shader>>()
        .insert(
            SHADER.id(),
            Shader::from_wgsl(shader_source(), "editor/ground_treatment.wgsl"),
        )
        .expect("fixed study shader handle");
}

// The terrain plugin fills in its prepared textures and storage-buffer binding after
// startup. Mirror those changes into all four treatments together; cloning only at
// creation would leave extensions waiting forever for an unbound terrain cache.
fn sync_base_materials(
    mut events: MessageReader<AssetEvent<TerrainMaterial>>,
    assets: Res<TreatmentAssets>,
    sources: Res<Assets<TerrainMaterial>>,
    mut materials: ResMut<Assets<StudyMaterial>>,
) {
    for event in events.read() {
        let id = match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => id,
            _ => continue,
        };
        for (source, target) in &assets.links {
            if source.id() == *id
                && let Some(base) = sources.get(source)
                && let Some(mut material) = materials.get_mut(target)
            {
                material.base = base.clone();
            }
        }
    }
}

impl TreatmentAssets {
    pub fn initialize(&mut self, images: &mut Assets<Image>) -> Result<(), String> {
        // This local source is already the authored AO used by the terrain normal/material
        // array. No new texture artwork or runtime asset pack is required by the experiment.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(DETAIL_SOURCE);
        let source = image::open(&path)
            .map_err(|e| format!("Ground experiment {}: {e}", path.display()))?
            .to_rgba8();
        if !source.width().is_power_of_two() || source.width() != source.height() {
            return Err("Ground study detail must be square and power of two".into());
        }
        let values: Vec<u8> = source
            .pixels()
            .map(|p| ((0.06 + 0.62 * (p[2] as f32 / 255.0).powi(8)) * 255.0).round() as u8)
            .collect();
        self.mean =
            values.iter().map(|&v| v as f64).sum::<f64>() as f32 / (values.len() as f32 * 255.0);
        let detail = scalar_image(source.width(), values, true, true);
        self.texture_bytes = detail.data.as_ref().map_or(0, Vec::len);
        self.detail = images.add(detail);
        self.coverage = images.add(scalar_image(1, vec![0], false, false));
        Ok(())
    }

    pub fn extension(&self, mode: GroundMode) -> GroundTreatment {
        GroundTreatment {
            settings: TreatmentSettings {
                bounds: Vec4::ZERO,
                controls: Vec4::new(mode.treatment().unwrap() as f32, self.mean, 0.5, 0.0),
            },
            coverage: self.coverage.clone(),
            detail: self.detail.clone(),
        }
    }

    pub fn write_diagnostics(&self, directory: &std::path::Path) -> Result<(), String> {
        std::fs::write(directory.join("ground-treatment.txt"), &self.stats)
            .map_err(|e| e.to_string())?;
        if let Some(mask) = &self.mask {
            image::save_buffer(
                directory.join("ground-coverage.png"),
                &mask.values,
                mask.size,
                mask.size,
                image::ColorType::L8,
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn scalar_image(size: u32, values: Vec<u8>, repeat: bool, mips: bool) -> Image {
    let mut image = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        values.clone(),
        TextureFormat::R8Unorm,
        RenderAssetUsages::default(),
    );
    if mips {
        let (data, levels) = mip_chain(size, values);
        image.data = Some(data);
        image.texture_descriptor.mip_level_count = levels;
    }
    let address = if repeat {
        ImageAddressMode::Repeat
    } else {
        ImageAddressMode::ClampToEdge
    };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address,
        address_mode_v: address,
        min_filter: ImageFilterMode::Linear,
        mag_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn mip_chain(mut size: u32, mut level: Vec<u8>) -> (Vec<u8>, u32) {
    let mut data = level.clone();
    let mut levels = 1;
    while size > 1 {
        let next = size / 2;
        let mut pixels = Vec::with_capacity((next * next) as usize);
        for y in 0..next {
            for x in 0..next {
                let i = (y * 2 * size + x * 2) as usize;
                let sum = level[i] as u32
                    + level[i + 1] as u32
                    + level[i + size as usize] as u32
                    + level[i + size as usize + 1] as u32;
                pixels.push(((sum + 2) / 4) as u8);
            }
        }
        data.extend_from_slice(&pixels);
        level = pixels;
        size = next;
        levels += 1;
    }
    (data, levels)
}

struct CoverageBake {
    size: u32,
    bounds: Vec4,
    values: Vec<u8>,
    roots: u32,
}

fn bake_coverage(scene: &vegetation::VegetationScene) -> Result<CoverageBake, String> {
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for page in &scene.pages {
        min = min.min(Vec2::from_array(page.origin_xz));
        max = max.max(Vec2::from_array(page.origin_xz) + Vec2::splat(page.size));
        // The grass study selects one population. Refuse an ambiguous competition bake.
        if page.fields.len() > 1 {
            return Err("Ground experiment currently requires one study population".into());
        }
    }
    if scene.pages.is_empty() {
        return Err("Ground study has no source pages".into());
    }
    let extent = max - min;
    let size = ((extent.max_element() * 16.0).ceil() as u32)
        .next_power_of_two()
        .clamp(64, 1024);
    let mut counts = vec![0.0_f32; (size * size) as usize];
    let mut roots = 0;
    for page in &scene.pages {
        for field in &page.fields {
            let population = scene
                .catalog
                .population(field.population)
                .ok_or("Missing study population")?;
            let domain = candidate_domain(page, population);
            for index in 0..domain.candidate_count() {
                let c = sample_candidate(population, domain, index, field.flow_direction).unwrap();
                if !page.owns(c.root_xz) || c.stable_rank >= candidate_density_retention(population)
                {
                    continue;
                }
                if page
                    .surface
                    .sample(page.origin_xz, page.size, c.root_xz)
                    .validity
                    < 0.5
                {
                    continue;
                }
                if random01(c.seed ^ 0x4cf5_ad43)
                    >= field.sample_coverage(page, c.root_xz) * c.group.density_retention
                {
                    continue;
                }
                let uv = (Vec2::from_array(c.root_xz) - min) / extent;
                let x = (uv.x * size as f32) as u32;
                let z = (uv.y * size as f32) as u32;
                if x < size && z < size {
                    counts[(z * size + x) as usize] += 1.0;
                    roots += 1;
                }
            }
        }
    }
    let pixel_area = extent.x * extent.y / (size * size) as f32;
    let radius = (SMOOTH_RADIUS_METRES * size as f32 / extent.max_element())
        .round()
        .max(1.0) as usize;
    let values = density_mask(&counts, size as usize, radius, pixel_area);
    Ok(CoverageBake {
        size,
        bounds: Vec4::new(min.x, min.y, extent.x.recip(), extent.y.recip()),
        values,
        roots,
    })
}

// Summed-area box filter: fixed world-space footprint, zero outside the specimen,
// independent of page boundaries. A smooth transfer avoids a threshold outline.
fn density_mask(counts: &[f32], size: usize, radius: usize, pixel_area: f32) -> Vec<u8> {
    let pitch = size + 1;
    let mut integral = vec![0.0; pitch * pitch];
    for y in 0..size {
        let mut row = 0.0;
        for x in 0..size {
            row += counts[y * size + x];
            integral[(y + 1) * pitch + x + 1] = integral[y * pitch + x + 1] + row;
        }
    }
    let area = ((radius * 2 + 1).pow(2)) as f32 * pixel_area;
    let mut values = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let x0 = x.saturating_sub(radius);
            let y0 = y.saturating_sub(radius);
            let x1 = (x + radius + 1).min(size);
            let y1 = (y + radius + 1).min(size);
            let count =
                integral[y1 * pitch + x1] - integral[y0 * pitch + x1] - integral[y1 * pitch + x0]
                    + integral[y0 * pitch + x0];
            let t = (count / area / ROOTS_FOR_FULL_COVERAGE).clamp(0.0, 1.0);
            values.push((t * t * (3.0 - 2.0 * t) * 255.0).round() as u8);
        }
    }
    values
}

fn sync(
    mut assets: ResMut<TreatmentAssets>,
    scene: Res<VegetationDebugScene>,
    mut state: ResMut<StudyState>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StudyMaterial>>,
) {
    if state.ground.treatment().is_none()
        || state.signature.is_none()
        || scene.scene().pages.is_empty()
        || assets.error.is_some()
        || assets.revision == Some(scene.revision())
    {
        return;
    }
    let started = Instant::now();
    match bake_coverage(scene.scene()) {
        Ok(mask) => {
            let hash = mask.values.iter().fold(0xcbf29ce484222325u64, |h, &v| {
                (h ^ v as u64).wrapping_mul(0x100000001b3)
            });
            let image = scalar_image(mask.size, mask.values.clone(), false, true);
            let bytes = assets.texture_bytes + image.data.as_ref().map_or(0, Vec::len);
            images
                .insert(assets.coverage.id(), image)
                .expect("study coverage handle");
            for (_, material) in materials.iter_mut() {
                material.extension.settings.bounds = mask.bounds;
            }
            assets.rebuilds += 1;
            assets.stats = format!(
                "Editor-only static ground material experiment\nSource revision: {}\nCoverage hash FNV1a: {hash:016x}\nSource retained roots: {}\nMask: {} x {}\nBounds: {:?}\nFull coverage: {ROOTS_FOR_FULL_COVERAGE} roots/m2\nSmoothing radius: {SMOOTH_RADIUS_METRES} m\nDetail mean: {:.6}\nAdditional texture bytes including mips: {bytes}\nCPU bake: {:.3} ms (debug build, on source edit only)\nRebuilds this session: {}\nAdded fragment reads: original 0; darkened 1; understory 2 (coverage + detail)\nNo additional grass vertices or visible ground draws; GPU timing not measured.\n",
                scene.revision(),
                mask.roots,
                mask.size,
                mask.size,
                mask.bounds,
                assets.mean,
                started.elapsed().as_secs_f64() * 1000.0,
                assets.rebuilds
            );
            info!("{}", assets.stats);
            assets.mask = Some(mask);
            assets.revision = Some(scene.revision());
            state.ready_frames = 0;
        }
        Err(error) => {
            assets.error = Some(error.clone());
            state.error = Some(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mask_preserves_empty_ground_and_increases_with_source_density() {
        assert!(density_mask(&[0.0; 64], 8, 1, 0.01).iter().all(|&v| v == 0));
        let sparse = density_mask(&[0.2; 64], 8, 1, 0.01);
        let dense = density_mask(&[1.0; 64], 8, 1, 0.01);
        assert!(sparse[27] > 0 && sparse[27] < dense[27]);
        assert_eq!(dense[27], 255);
        let mut edge = [0.0; 64];
        for row in edge.chunks_mut(8) {
            row[4..].fill(1.0);
        }
        let edge = density_mask(&edge, 8, 1, 0.01);
        assert_eq!(edge[24], 0);
        assert_eq!(edge[30], 255);
    }
    #[test]
    fn mip_filter_keeps_mean_darkness() {
        let (data, levels) = mip_chain(2, vec![0, 255, 0, 255]);
        assert_eq!(levels, 2);
        assert_eq!(data, [0, 255, 0, 255, 128]);
    }
    #[test]
    fn production_shader_hook_is_unique() {
        assert!(shader_source().contains("base.rgb * study_ground_multiplier"));
    }
}
