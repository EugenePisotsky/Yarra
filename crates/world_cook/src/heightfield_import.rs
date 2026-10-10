//! Imports a heightfield from a terrain tool (Houdini, World Creator, Gaea) as the terrain of a
//! project's default world. The neutral format is a JSON manifest beside little-endian f32
//! samples on a regular XZ grid, plus optional named masks of one byte per sample on the same
//! grid; `tools/houdini_export_heightfield.py` writes it from Houdini. The heightfield's
//! footprint defines the world. Layers with an imported mask take their coverage from the mask
//! of that name, or from a mask generated from height and slope; painted layers are kept. A
//! re-import rewrites only cells whose heights or imported coverage changed, so the following
//! cook is incremental.
use super::*;
use serde::Deserialize;
use std::path::PathBuf;
use terrain_world::{HeightFn, highest, nearest_shore, terrain_cell, view};
use world_db::{TerrainImportStats, TerrainImportWriter};

const FORMAT: &str = "yarra-heightfield";
const VERSION: u32 = 2;
/// 1 GiB of samples, e.g. 16k × 16k.
const MAX_SAMPLES: usize = 1 << 28;
const MAX_MASKS: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    /// Samples along X and Z.
    samples: [usize; 2],
    /// Metres between neighbouring samples.
    spacing: f32,
    /// World XZ of the first sample.
    origin: [f32; 2],
    /// Little-endian f32 heights in rows along +X, one row per Z; relative to the manifest.
    heights: PathBuf,
    sea_level: f32,
    /// Named masks on the heightfield's grid.
    #[serde(default)]
    masks: Vec<ManifestMask>,
    /// Where the player starts; by default, the shore nearest the centre.
    #[serde(default)]
    start: Option<[f32; 2]>,
    /// Free text naming the scene and node it came from.
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestMask {
    /// Layers read the mask by this name (`environment::valid_mask_name`).
    name: String,
    /// One byte per sample, 0 to 255 for 0 to 1, in the heights' order; relative to the
    /// manifest.
    file: PathBuf,
}

struct Heightfield {
    samples: [usize; 2],
    spacing: f32,
    origin: Vec2,
    heights: Vec<f32>,
    range: [f32; 2],
    sea_level: f32,
    start: Option<Vec2>,
    /// Name and samples of each mask.
    masks: Vec<(String, Vec<u8>)>,
}

impl Heightfield {
    fn load(path: &Path) -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(&fs::read(path)?)
            .with_context(|| format!("invalid heightfield manifest {}", path.display()))?;
        if manifest.format != FORMAT || manifest.version != VERSION {
            bail!("expected a {FORMAT} version {VERSION} manifest");
        }
        let [nx, nz] = manifest.samples;
        if nx < 2 || nz < 2 || nx.saturating_mul(nz) > MAX_SAMPLES {
            bail!(
                "heightfield sample counts must be at least 2 and at most {MAX_SAMPLES} in total"
            );
        }
        let positive = manifest.spacing.is_finite() && manifest.spacing > 0.;
        if !positive
            || !manifest.origin.iter().all(|v| v.is_finite())
            || !manifest.sea_level.is_finite()
        {
            bail!("heightfield spacing must be positive; spacing, origin and sea level finite");
        }
        let directory = path.parent().unwrap_or(Path::new("."));
        let read = |name: &Path, bytes_per_sample: usize| -> Result<Vec<u8>> {
            let file = directory.join(name);
            let bytes =
                fs::read(&file).with_context(|| format!("cannot read {}", file.display()))?;
            if bytes.len() != nx * nz * bytes_per_sample {
                bail!(
                    "{} holds {} bytes, expected {nx} × {nz} samples of {bytes_per_sample} bytes",
                    file.display(),
                    bytes.len()
                );
            }
            Ok(bytes)
        };
        let bytes = read(&manifest.heights, 4)?;
        let heights: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        if heights.iter().any(|h| !h.is_finite()) {
            bail!("{} contains non-finite heights", manifest.heights.display());
        }
        let range = heights
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], &h| {
                [lo.min(h), hi.max(h)]
            });
        if manifest.masks.len() > MAX_MASKS {
            bail!("a heightfield may have at most {MAX_MASKS} masks");
        }
        let mut masks: Vec<(String, Vec<u8>)> = Vec::with_capacity(manifest.masks.len());
        for mask in &manifest.masks {
            if !environment::valid_mask_name(&mask.name) {
                bail!(
                    "mask name {:?} must be 1–32 lowercase letters, digits or underscores, \
                     starting with a letter",
                    mask.name
                );
            }
            if masks.iter().any(|(name, _)| *name == mask.name) {
                bail!("mask {:?} appears twice", mask.name);
            }
            masks.push((mask.name.clone(), read(&mask.file, 1)?));
        }
        if let Some(source) = &manifest.source {
            println!("Heightfield from {source}");
        }
        Ok(Self {
            samples: manifest.samples,
            spacing: manifest.spacing,
            origin: Vec2::from(manifest.origin),
            heights,
            range,
            sea_level: manifest.sea_level,
            start: manifest.start.map(Vec2::from),
            masks,
        })
    }

    /// Mask `index` at `p`, from 0 to 1, interpolated linearly so it never overshoots. Beyond
    /// the grid the border samples continue.
    fn mask(&self, index: usize, p: Vec2) -> f32 {
        let [nx, nz] = self.samples;
        let samples = &self.masks[index].1;
        let u = ((p - self.origin) / self.spacing)
            .clamp(Vec2::ZERO, Vec2::new(nx as f32 - 1., nz as f32 - 1.));
        let (x, z) = ((u.x as usize).min(nx - 2), (u.y as usize).min(nz - 2));
        let f = u - Vec2::new(x as f32, z as f32);
        let at = |x: usize, z: usize| f32::from(samples[z * nx + x]) / 255.;
        let near = at(x, z) + (at(x + 1, z) - at(x, z)) * f.x;
        let far = at(x, z + 1) + (at(x + 1, z + 1) - at(x, z + 1)) * f.x;
        near + (far - near) * f.y
    }

    fn sample(&self, x: isize, z: isize) -> f32 {
        let [nx, nz] = self.samples;
        let x = x.clamp(0, nx as isize - 1) as usize;
        let z = z.clamp(0, nz as isize - 1) as usize;
        self.heights[z * nx + x]
    }

    /// Catmull-Rom interpolation, so upsampled ground has no creases along source samples.
    /// Beyond the grid the border samples continue.
    fn height(&self, p: Vec2) -> f32 {
        let u = (p - self.origin) / self.spacing;
        let base = u.floor();
        let f = u - base;
        let weights = |t: f32| {
            let t2 = t * t;
            let t3 = t2 * t;
            [
                0.5 * (-t3 + 2. * t2 - t),
                0.5 * (3. * t3 - 5. * t2 + 2.),
                0.5 * (-3. * t3 + 4. * t2 + t),
                0.5 * (t3 - t2),
            ]
        };
        let (wx, wz) = (weights(f.x), weights(f.y));
        let (x, z) = (base.x as isize, base.y as isize);
        let mut height = 0.;
        for (j, wz) in wz.iter().enumerate() {
            let mut row = 0.;
            for (i, wx) in wx.iter().enumerate() {
                row += wx * self.sample(x + i as isize - 1, z + j as isize - 1);
            }
            height += wz * row;
        }
        height
    }

    /// The first and last cells of the world: those holding samples, widened to whole blocks
    /// of up to 32 cells. The terrain hierarchy then closes into a few large roots, where a
    /// ragged border would need more small ones than the coarse cover allows.
    fn cells(&self) -> [CellCoord; 2] {
        let far = self.origin
            + Vec2::new(self.samples[0] as f32 - 1., self.samples[1] as f32 - 1.) * self.spacing;
        let cell = |p: Vec2| {
            [
                (p.x / DEFAULT_CELL_SIZE).floor() as i32,
                (p.y / DEFAULT_CELL_SIZE).floor() as i32,
            ]
        };
        let (first, last) = (cell(self.origin), cell(far));
        let span = (last[0] - first[0]).max(last[1] - first[1]) + 1;
        let block = ((span / 16) as u32).next_power_of_two().min(32) as i32;
        let widen = |a: i32, b: i32| {
            (
                a.div_euclid(block) * block,
                (b.div_euclid(block) + 1) * block - 1,
            )
        };
        let (x0, x1) = widen(first[0], last[0]);
        let (z0, z1) = widen(first[1], last[1]);
        [CellCoord { x: x0, z: z0 }, CellCoord { x: x1, z: z1 }]
    }

    fn centre(&self) -> Vec2 {
        self.origin
            + Vec2::new(self.samples[0] as f32 - 1., self.samples[1] as f32 - 1.)
                * self.spacing
                * 0.5
    }
}

#[derive(Debug, Clone)]
pub struct HeightfieldImportReport {
    pub stats: TerrainImportStats,
    /// Exactly flat cells, which store no heightfield.
    pub flat_cells: u64,
    pub bounds: [f32; 2],
    pub created: bool,
    pub seconds: f64,
    /// Imported masks that no layer reads.
    pub unused_masks: Vec<String>,
}

/// Imports `manifest` as the default world's terrain in `project`, creating the project if
/// it does not exist. The `start` view becomes where play starts by default; it and a
/// `summit` view are also written beside the project.
pub fn import_heightfield(manifest: &Path, project: &Path) -> Result<HeightfieldImportReport> {
    let start = std::time::Instant::now();
    let field = Heightfield::load(manifest)?;
    // World settings are part of every cell's cook inputs. Rounding keeps them, and the
    // incremental cook, stable while the terrain is reshaped.
    let bounds = [
        ((field.range[0].min(field.sea_level) - 20.) / 100.).floor() * 100.,
        ((field.range[1].max(field.sea_level) + 100.) / 100.).ceil() * 100.,
    ];
    let created = !project.exists();
    if created {
        write_project_database(
            project,
            &terrain_world::base_document("Island", bounds, field.sea_level),
        )
        .with_context(|| format!("failed to create {}", project.display()))?;
    }
    let mut writer = TerrainImportWriter::open(project, bounds, Some(field.sea_level))?;
    let field = &field;
    let samplers: Vec<_> = (0..field.masks.len())
        .map(|index| move |p: Vec2| field.mask(index, p))
        .collect();
    let masks: Vec<(&str, &terrain_world::MaskFn)> = field
        .masks
        .iter()
        .zip(&samplers)
        .map(|((name, _), sample)| (name.as_str(), sample as &terrain_world::MaskFn))
        .collect();
    let layers = terrain_world::imported_layers(writer.definition(), &masks)?;
    let unused_masks =
        terrain_world::unused_masks(writer.definition(), masks.iter().map(|(name, _)| *name));
    let height = |p: Vec2| field.height(p);
    let [first, last] = field.cells();
    let corner = |cell: CellCoord| Vec2::new(cell.x as f32, cell.z as f32) * DEFAULT_CELL_SIZE;
    let world = [
        corner(first),
        corner(CellCoord {
            x: last.x + 1,
            z: last.z + 1,
        }),
    ];
    let mut flat_cells = 0;
    // Rows of cells are built on every core and written here in order.
    let rows: Vec<_> = (first.z..=last.z).collect();
    for rows in rows.chunks(4) {
        let cells: Vec<_> = rows
            .iter()
            .flat_map(|&z| (first.x..=last.x).map(move |x| CellCoord { x, z }))
            .collect();
        for cell in parallel::map(&cells, |&cell| terrain_cell(cell, &height, &layers, world)) {
            flat_cells += u64::from(cell.heights.is_none());
            writer.put(&cell)?;
        }
    }
    let views = views(&height, field)?;
    writer.set_start_view(&views[0].1)?;
    let stats = writer.finish()?;
    terrain_world::write_views(project, &views)?;
    Ok(HeightfieldImportReport {
        stats,
        flat_cells,
        bounds,
        created,
        seconds: start.elapsed().as_secs_f64(),
        unused_masks,
    })
}

/// `start` looks from the start position towards the summit; `summit` looks back.
fn views(
    height: &HeightFn,
    field: &Heightfield,
) -> Result<Vec<(&'static str, world::WorldViewBookmark)>> {
    let centre = field.centre();
    let radius = (field.samples[0].max(field.samples[1]) as f32 - 1.) * field.spacing * 0.5;
    let summit = highest(height, centre, radius);
    let start = match field.start {
        Some(start) => start,
        None => nearest_shore(height, centre).context("the heightfield has no shore")?,
    };
    let towards = if summit.distance(start) > 1. {
        summit - start
    } else {
        centre - start
    };
    Ok(vec![
        ("start", view(height, start, towards, 8., 9.7)),
        ("summit", view(height, summit, start - summit, 6., 14.)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Output(PathBuf);
    impl Output {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "yarra-heightfield-import-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A 2 m grid over 256 m: a round island in flat sea, with an optional bump.
    fn write_field(dir: &Path, bump: f32) -> PathBuf {
        write_field_with_masks(dir, bump, &[])
    }

    type MaskRule = fn(Vec2) -> u8;

    /// `write_field` with masks, each a name and its value at a sample.
    fn write_field_with_masks(dir: &Path, bump: f32, masks: &[(&str, MaskRule)]) -> PathBuf {
        let n = 129;
        let origin = [-127.0_f32, -129.0];
        let points: Vec<_> = (0..n)
            .flat_map(|z| (0..n).map(move |x| (x, z)))
            .map(|(x, z)| Vec2::new(origin[0] + x as f32 * 2., origin[1] + z as f32 * 2.))
            .collect();
        let heights: Vec<u8> = points
            .iter()
            .flat_map(|&p| {
                let island = 40. * (1. - p.length() / 70.);
                let bumped = bump * (-(p - Vec2::new(20., 10.)).length_squared() / 36.).exp();
                (island.max(-30.) + bumped).to_le_bytes()
            })
            .collect();
        fs::write(dir.join("height.f32"), heights).unwrap();
        for (name, rule) in masks {
            let samples: Vec<u8> = points.iter().map(|&p| rule(p)).collect();
            fs::write(dir.join(format!("{name}.u8")), samples).unwrap();
        }
        let masks: Vec<_> = masks
            .iter()
            .map(|(name, _)| format!(r#"{{"name": "{name}", "file": "{name}.u8"}}"#))
            .collect();
        let manifest = dir.join("island.json");
        fs::write(
            &manifest,
            format!(
                r#"{{"format": "yarra-heightfield", "version": 2, "samples": [{n}, {n}],
                "spacing": 2.0, "origin": [{}, {}], "heights": "height.f32",
                "sea_level": 0.0, "masks": [{}], "start": [60.0, 0.0], "source": "test"}}"#,
                origin[0],
                origin[1],
                masks.join(", ")
            ),
        )
        .unwrap();
        manifest
    }

    #[test]
    fn heightfield_is_interpolated_through_its_samples() {
        let output = Output::new();
        let field = Heightfield::load(&write_field(&output.0, 0.)).unwrap();
        // Samples are reproduced exactly; between them the surface is smooth.
        let p = Vec2::new(-127. + 2. * 30., -129. + 2. * 70.);
        let expected = 40. * (1. - p.length() / 70.);
        assert!((field.height(p) - expected.max(-30.)).abs() < 1e-3);
        assert_eq!(field.height(Vec2::new(-5000., -5000.)), -30.);
        assert_eq!(
            field.cells(),
            [CellCoord { x: -4, z: -5 }, CellCoord { x: 4, z: 3 }]
        );
    }

    #[test]
    fn large_worlds_widen_to_whole_blocks_of_cells() {
        // Houdini's 10 km heightfield at 2 m: samples from -4999 to 4999 m cover cells -157 to
        // 156, which would leave a ragged hierarchy border.
        let field = Heightfield {
            samples: [5000, 5000],
            spacing: 2.,
            origin: Vec2::splat(-4999.),
            heights: Vec::new(),
            range: [-30., 1266.],
            sea_level: 0.,
            start: None,
            masks: Vec::new(),
        };
        assert_eq!(
            field.cells(),
            [CellCoord { x: -160, z: -160 }, CellCoord { x: 159, z: 159 }]
        );
    }

    #[test]
    fn import_creates_a_world_and_reimport_rewrites_only_changed_cells() {
        let output = Output::new();
        let project = output.0.join("island.project.sqlite");
        let runtime = output.0.join("island.runtime.sqlite");
        let manifest = write_field(&output.0, 0.);
        let first = import_heightfield(&manifest, &project).unwrap();
        assert!(first.created);
        assert_eq!(first.stats.cells, 9 * 9);
        assert_eq!(first.stats.added, first.stats.cells);
        // Corner cells lie in flat sea and store no heights.
        assert!(first.flat_cells > 0 && first.flat_cells < first.stats.cells);
        assert_eq!(first.bounds, [-100., 200.]);
        assert!(project.with_extension("views").join("start.ron").is_file());
        let cooked = cook_project_with_report(&project, &runtime).unwrap();
        assert_eq!(cooked.stats.terrain_cells, 81);
        let start = |runtime: &Path| {
            world_db::RuntimeReader::open_immutable(runtime)
                .unwrap()
                .manifest()
                .start_view
                .clone()
                .unwrap()
                .position
        };
        assert_eq!(start(&runtime)[0], 60.);

        // Moving the start alone changes no cell and recooks nothing.
        let text = fs::read_to_string(&manifest).unwrap();
        fs::write(&manifest, text.replace("[60.0, 0.0]", "[40.0, 0.0]")).unwrap();
        let same = import_heightfield(&manifest, &project).unwrap();
        assert!(!same.created);
        assert_eq!(
            (same.stats.added, same.stats.changed, same.stats.removed),
            (0, 0, 0)
        );
        let unchanged = cook_project_with_report(&project, &runtime).unwrap();
        assert!(unchanged.stats.incremental);
        assert_eq!(unchanged.stats.terrain_cells, 0);
        assert_eq!(start(&runtime)[0], 40.);
        assert_eq!(
            unchanged.manifest.content_hash,
            cooked.manifest.content_hash
        );

        // A 6 m bump inside one cell changes it and the neighbours its slopes reach.
        let manifest = write_field(&output.0, 6.);
        let bumped = import_heightfield(&manifest, &project).unwrap();
        assert!(
            bumped.stats.changed > 0 && bumped.stats.changed < 9,
            "{bumped:?}"
        );
        let incremental = cook_project_with_report(&project, &runtime).unwrap();
        assert!(incremental.stats.incremental);
        let fresh = cook_project_fresh(&project, &output.0.join("fresh.sqlite"), None).unwrap();
        assert_eq!(
            incremental.manifest.content_hash,
            fresh.manifest.content_hash
        );
    }

    #[test]
    fn masks_fill_their_layers_and_reimports_keep_painted_layers() {
        let output = Output::new();
        let sand: MaskRule = |p| if p.x > 10. { 255 } else { 0 };
        let manifest = write_field_with_masks(&output.0, 0., &[("sand", sand)]);
        let first = output.0.join("first.project.sqlite");
        let report = import_heightfield(&manifest, &first).unwrap();
        assert_eq!(report.unused_masks, ["sand"]);

        // A sand layer reads the mask; a forest layer is painted in cell (1, 0).
        let mut document = world_db::read_project_database(&first).unwrap();
        let definition = &mut document.environments[0];
        let template = definition.layers[0].clone();
        let layer = |name: &str, order, imported_mask: Option<&str>| environment::Layer {
            id: environment::LayerId(stable_id(name)),
            name: name.into(),
            order,
            imported_mask: imported_mask.map(Into::into),
            ..template.clone()
        };
        let (sand_layer, forest_layer) =
            (layer("sand", 10, Some("sand")), layer("forest", 11, None));
        definition
            .layers
            .extend([sand_layer.clone(), forest_layer.clone()]);
        let green = definition
            .layers
            .iter()
            .find(|l| l.imported_mask.as_deref() == Some("green"))
            .unwrap()
            .id;
        let cell = CellCoord { x: 1, z: 0 };
        // Empty along the border, which neighbouring cells share.
        let side = terrain_world::MASK_SIDE;
        let painted = environment::CoverageTile {
            layer: forest_layer.id,
            samples: (0..side * side)
                .map(|i| {
                    let inside = |v| (1..side - 1).contains(&v);
                    if inside(i % side) && inside(i / side) {
                        200
                    } else {
                        0
                    }
                })
                .collect(),
        };
        document
            .environment_cells
            .iter_mut()
            .find(|c| c.cell == cell)
            .unwrap()
            .tiles
            .push(painted.clone());
        let project = output.0.join("island.project.sqlite");
        world_db::write_project_database(&project, &document).unwrap();

        // An imported `green` replaces the generated one.
        let manifest = write_field_with_masks(&output.0, 0., &[("sand", sand), ("green", |_| 0)]);
        let report = import_heightfield(&manifest, &project).unwrap();
        assert!(report.unused_masks.is_empty(), "{:?}", report.unused_masks);
        assert!(report.stats.changed > 0);
        let tiles = |project: &Path, cell: CellCoord| {
            world_db::read_project_database(project)
                .unwrap()
                .environment_cells
                .into_iter()
                .find(|c| c.cell == cell)
                .map_or_else(Vec::new, |c| c.tiles)
        };
        let east = tiles(&project, cell);
        let tile = |layer| east.iter().find(|t| t.layer == layer);
        assert!(
            tile(sand_layer.id)
                .unwrap()
                .samples
                .iter()
                .all(|&s| s == 255)
        );
        assert_eq!(tile(forest_layer.id), Some(&painted));
        assert_eq!(tile(green), None);
        assert!(
            tiles(&project, CellCoord { x: -2, z: 0 })
                .iter()
                .all(|t| t.layer != sand_layer.id)
        );

        // Imported coverage agrees along shared cell borders, as the cook requires.
        cook_project_with_report(&project, &output.0.join("island.runtime.sqlite")).unwrap();

        // Importing the same terrain again changes nothing and keeps the painting.
        let again = import_heightfield(&manifest, &project).unwrap();
        assert_eq!((again.stats.added, again.stats.changed), (0, 0));
        assert_eq!(tiles(&project, cell), east);

        // A layer whose mask the import lacks is an error.
        let manifest = write_field_with_masks(&output.0, 0., &[("green", |_| 0)]);
        let error = import_heightfield(&manifest, &project).unwrap_err();
        assert!(format!("{error:#}").contains("\"sand\""), "{error:#}");
    }

    #[test]
    fn mask_names_and_sizes_are_checked() {
        let output = Output::new();
        let project = output.0.join("island.project.sqlite");
        for name in ["Sand", "sand", "wet ness"] {
            let manifest = write_field_with_masks(&output.0, 0., &[(name, |_| 0), ("sand", |_| 0)]);
            assert!(import_heightfield(&manifest, &project).is_err(), "{name}");
        }
        let manifest = write_field_with_masks(&output.0, 0., &[("sand", |_| 0)]);
        fs::write(output.0.join("sand.u8"), [0; 10]).unwrap();
        let error = import_heightfield(&manifest, &project).unwrap_err();
        assert!(format!("{error:#}").contains("sand.u8"), "{error:#}");
        assert!(!project.exists());
    }
}
