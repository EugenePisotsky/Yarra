//! Static source-root coverage, using the same footprint as the full-field study.
//! Each tile includes neighboring roots and a one-texel border. Neither camera,
//! wind nor emitted LOD density changes the result.
mod boundary;
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
pub use boundary::{BoundaryField, MARGIN, MAX_DEPTH};
use vegetation::{
    VegetationCatalog, VegetationFieldPage, candidate_density_retention, candidate_domain,
    random01, sample_candidate,
};

const TEXELS_PER_METRE: f32 = 16.0;
const RADIUS: f32 = 0.65;
const AREA_PER_ROOT: f32 = 0.075;

pub struct Bake {
    pub image: Image,
    pub bounds: Vec4,
}

pub fn bake(
    catalog: &VegetationCatalog,
    neighbors: &[VegetationFieldPage],
    origin: [f32; 2],
    extent: f32,
) -> Bake {
    let inner = (extent * TEXELS_PER_METRE).round().clamp(1.0, 1024.0) as usize;
    let step = extent / inner as f32;
    let radius = (RADIUS / step).round().max(1.0) as usize;
    // Border texels make adjacent terrain tiles sample the same filtered field.
    let output_size = inner + 2;
    let count_size = output_size + radius * 2;
    let output_min = Vec2::from_array(origin) - Vec2::splat(step);
    let count_min = output_min - Vec2::splat(radius as f32 * step);
    let count_max = count_min + Vec2::splat(count_size as f32 * step);
    let mut counts = vec![0.0_f32; count_size * count_size];
    for page in neighbors {
        let minimum = Vec2::from_array(page.origin_xz).max(count_min);
        let maximum = (Vec2::from_array(page.origin_xz) + Vec2::splat(page.size)).min(count_max);
        if (maximum - minimum).min_element() <= 0.0 {
            continue;
        }
        for field in &page.fields {
            let Some(population) = catalog.population(field.population) else {
                continue;
            };
            // The treatment belongs to this grass population, not to unrelated plants.
            if population.key != "short_split_fill" {
                continue;
            }
            let mut domain = candidate_domain(page, population);
            // Preserve the placement margin from the source domain while restricting
            // work to the overlap strip instead of resampling all neighboring pages.
            let margin_x = (page.origin_xz[0] / domain.spacing).floor() as i32 - domain.cell_min[0];
            let margin_z = (page.origin_xz[1] / domain.spacing).floor() as i32 - domain.cell_min[1];
            let lo = [
                (minimum.x / domain.spacing).floor() as i32 - margin_x,
                (minimum.y / domain.spacing).floor() as i32 - margin_z,
            ];
            let hi = [
                (maximum.x / domain.spacing).ceil() as i32 + margin_x + 1,
                (maximum.y / domain.spacing).ceil() as i32 + margin_z + 1,
            ];
            domain.cell_min = lo;
            domain.cell_count = [(hi[0] - lo[0]) as u32, (hi[1] - lo[1]) as u32];
            for index in 0..domain.candidate_count() {
                let c = sample_candidate(population, domain, index, field.flow_direction).unwrap();
                let root = Vec2::from_array(c.root_xz);
                if !page.owns(c.root_xz)
                    || root.cmplt(count_min).any()
                    || root.cmpge(count_max).any()
                    || c.stable_rank >= candidate_density_retention(population)
                    || random01(c.seed ^ 0x4cf5_ad43)
                        >= field.sample_coverage(page, c.root_xz) * c.group.density_retention
                    || page
                        .surface
                        .sample(page.origin_xz, page.size, c.root_xz)
                        .validity
                        < 0.5
                {
                    continue;
                }
                // At nonzero page coordinates, subtraction can round a point just
                // inside the upper bound onto count_size. Keep that point in the
                // final texel after the world-space ownership test above.
                let xy = ((root - count_min) / step)
                    .as_uvec2()
                    .min(UVec2::splat(count_size as u32 - 1));
                counts[xy.y as usize * count_size + xy.x as usize] += 1.0;
            }
        }
    }
    let values = filter(&counts, count_size, radius, step * step);
    let boundary = BoundaryField::bake(
        catalog,
        neighbors,
        Vec2::from_array(origin),
        Vec2::from_array(origin) + Vec2::splat(extent),
    );
    let mut channels = Vec::with_capacity(values.len() * 2);
    for (i, cover) in values.into_iter().enumerate() {
        let p = output_min
            + (Vec2::new((i % output_size) as f32, (i / output_size) as f32) + Vec2::splat(0.5))
                * step;
        channels.push(cover);
        channels.push((boundary.sample(p) / MAX_DEPTH * 255.0).round() as u8);
    }
    let mut image = Image::new(
        Extent3d {
            width: output_size as u32,
            height: output_size as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        channels,
        TextureFormat::Rg8Unorm,
        RenderAssetUsages::default(),
    );
    // This mask is already filtered over 1.3 m; linear sampling of 6.25 cm texels
    // suffices without rebuilding a large field-wide mip pyramid on streaming.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        min_filter: ImageFilterMode::Linear,
        mag_filter: ImageFilterMode::Linear,
        ..default()
    });
    Bake {
        image,
        bounds: Vec4::new(
            output_min.x,
            output_min.y,
            (output_size as f32 * step).recip(),
            (output_size as f32 * step).recip(),
        ),
    }
}

fn filter(counts: &[f32], size: usize, radius: usize, pixel_area: f32) -> Vec<u8> {
    let pitch = size + 1;
    let mut integral = vec![0.0_f32; pitch * pitch];
    for y in 0..size {
        let mut row = 0.0;
        for x in 0..size {
            row += counts[y * size + x];
            integral[(y + 1) * pitch + x + 1] = integral[y * pitch + x + 1] + row;
        }
    }
    let output = size - radius * 2;
    let area = (2 * radius + 1).pow(2) as f32 * pixel_area;
    let mut result = Vec::with_capacity(output * output);
    for y in 0..output {
        for x in 0..output {
            let x1 = x + 2 * radius + 1;
            let y1 = y + 2 * radius + 1;
            let count =
                (integral[y1 * pitch + x1] - integral[y * pitch + x1] - integral[y1 * pitch + x]
                    + integral[y * pitch + x])
                    .max(0.0);
            result.push(((1.0 - (-count / area * AREA_PER_ROOT).exp()) * 255.0).round() as u8);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_ground_stays_clear_and_coverage_increases_with_density() {
        assert!(filter(&[0.0; 64], 8, 1, 0.01).iter().all(|&v| v == 0));
        let sparse = filter(&[0.1; 64], 8, 1, 0.01);
        let dense = filter(&[0.72; 64], 8, 1, 0.01);
        assert!(sparse.iter().all(|&v| v > 0 && v < dense[0]));
        assert!(dense.iter().all(|&v| v == dense[0]));
    }
    #[test]
    fn translated_streamed_pages_do_not_index_past_the_mask() {
        let catalog: VegetationCatalog = ron::from_str(include_str!(
            "../../../content/vegetation/field-current.ron"
        ))
        .unwrap();
        let population = catalog
            .populations
            .iter()
            .find(|p| p.key == "short_split_fill")
            .unwrap()
            .id;
        for origin in [[-96.0, 64.0], [0.0, 0.0], [96.0, 128.0], [288.0, 320.0]] {
            let page = VegetationFieldPage {
                origin_xz: origin,
                size: 32.0,
                surface: vegetation::VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
                fields: vec![vegetation::VegetationPopulationField {
                    population,
                    resolution: 1,
                    coverage: vec![255],
                    flow_direction: [0.0, 1.0],
                }],
            };
            let result = bake(&catalog, &[page], origin, 32.0);
            assert_eq!(result.image.data.unwrap().len(), 514 * 514 * 2);
        }
    }
    #[test]
    fn independently_baked_tiles_agree_across_their_shared_border() {
        let catalog: VegetationCatalog = ron::from_str(include_str!(
            "../../../content/vegetation/field-current.ron"
        ))
        .unwrap();
        let population = catalog
            .populations
            .iter()
            .find(|p| p.key == "short_split_fill")
            .unwrap()
            .id;
        let neighbors = [-2.0, 0.0].map(|x| VegetationFieldPage {
            origin_xz: [x, 0.0],
            size: 2.0,
            surface: vegetation::VegetationSurfaceField {
                resolution: 2,
                heights: vec![0.0; 4],
                normals_oct: vec![[0, 0]; 4],
                validity: vec![255; 4],
            },
            fields: vec![vegetation::VegetationPopulationField {
                population,
                resolution: 1,
                coverage: vec![255],
                flow_direction: [0.0, 1.0],
            }],
        });
        let a = bake(&catalog, &neighbors, [-2.0, 0.0], 2.0);
        let b = bake(&catalog, &neighbors, [0.0, 0.0], 2.0);
        let a = a.image.data.unwrap();
        let b = b.image.data.unwrap();
        for y in 0..34 {
            assert_eq!(
                &a[(y * 34 + 32) * 2..(y * 34 + 34) * 2],
                &b[y * 34 * 2..(y * 34 + 2) * 2]
            );
        }
    }
}
