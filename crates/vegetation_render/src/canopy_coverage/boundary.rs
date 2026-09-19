//! Distance inside the authored grass area, independent of roots, wind and rendered LOD.
use bevy::prelude::*;
use vegetation::{VegetationCatalog, VegetationFieldPage};

pub const MAX_DEPTH: f32 = 4.0;
pub const MARGIN: f32 = MAX_DEPTH + 1.0;
const STEP: f32 = 0.25;

pub struct BoundaryField {
    pub minimum: Vec2,
    pub step: f32,
    pub size: UVec2,
    pub depth: Vec<f32>,
}

impl BoundaryField {
    pub fn for_scene(catalog: &VegetationCatalog, pages: &[VegetationFieldPage]) -> Self {
        let mut minimum = Vec2::splat(f32::INFINITY);
        let mut maximum = Vec2::splat(f32::NEG_INFINITY);
        for page in pages {
            minimum = minimum.min(Vec2::from_array(page.origin_xz));
            maximum = maximum.max(Vec2::from_array(page.origin_xz) + Vec2::splat(page.size));
        }
        if pages.is_empty() {
            minimum = Vec2::ZERO;
            maximum = Vec2::ZERO;
        }
        Self::bake(catalog, pages, minimum, maximum)
    }

    pub fn bake(
        catalog: &VegetationCatalog,
        pages: &[VegetationFieldPage],
        min: Vec2,
        max: Vec2,
    ) -> Self {
        // The normal resident world uses 25 cm texels. Bound unusual study extents as well.
        let step = STEP.max(((max - min).max_element() + MARGIN * 2.0) / 2048.0);
        let minimum = ((min - Vec2::splat(MARGIN)) / step).floor() * step;
        let size = ((max + Vec2::splat(MARGIN) - minimum) / step)
            .ceil()
            .as_uvec2()
            .max(UVec2::ONE);
        let mut depth = vec![0.0; size.x as usize * size.y as usize];
        for page in pages {
            let page_min = Vec2::from_array(page.origin_xz);
            let lo = ((page_min - minimum) / step)
                .floor()
                .max(Vec2::ZERO)
                .as_uvec2()
                .min(size);
            let hi = ((page_min + Vec2::splat(page.size) - minimum) / step)
                .ceil()
                .max(Vec2::ZERO)
                .as_uvec2()
                .min(size);
            for field in &page.fields {
                if !catalog
                    .population(field.population)
                    .is_some_and(|p| p.key == "short_split_fill")
                {
                    continue;
                }
                for y in lo.y..hi.y {
                    for x in lo.x..hi.x {
                        let p = minimum + (Vec2::new(x as f32, y as f32) + Vec2::splat(0.5)) * step;
                        if page.owns(p.to_array())
                            && field.sample_coverage(page, p.to_array()) >= 0.1
                            && page
                                .surface
                                .sample(page.origin_xz, page.size, p.to_array())
                                .validity
                                >= 0.5
                        {
                            depth[(y * size.x + x) as usize] = MAX_DEPTH + step * 0.5;
                        }
                    }
                }
            }
        }
        distance_transform(&mut depth, size, step);
        Self {
            minimum,
            step,
            size,
            depth,
        }
    }

    pub fn sample(&self, p: Vec2) -> f32 {
        let grid = ((p - self.minimum) / self.step - Vec2::splat(0.5))
            .clamp(Vec2::ZERO, (self.size - UVec2::ONE).as_vec2());
        let lo = grid.floor().as_uvec2();
        let hi = (lo + UVec2::ONE).min(self.size - UVec2::ONE);
        let t = grid.fract();
        let at = |x, y| self.depth[(y * self.size.x + x) as usize];
        let a = at(lo.x, lo.y) * (1.0 - t.x) + at(hi.x, lo.y) * t.x;
        let b = at(lo.x, hi.y) * (1.0 - t.x) + at(hi.x, hi.y) * t.x;
        a * (1.0 - t.y) + b * t.y
    }

    /// Header and scalar grid for the separate canopy buffer.
    pub fn gpu_values(&self) -> Vec<f32> {
        let mut data = vec![
            self.minimum.x,
            self.minimum.y,
            self.step,
            self.size.x as f32,
            self.size.y as f32,
            0.0,
            0.0,
            0.0,
        ];
        data.extend_from_slice(&self.depth);
        data
    }
}

fn distance_transform(depth: &mut [f32], size: UVec2, step: f32) {
    let w = size.x as usize;
    let h = size.y as usize;
    let diagonal = step * std::f32::consts::SQRT_2;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if x > 0 {
                depth[i] = depth[i].min(depth[i - 1] + step);
            }
            if y > 0 {
                depth[i] = depth[i].min(depth[i - w] + step);
                if x > 0 {
                    depth[i] = depth[i].min(depth[i - w - 1] + diagonal);
                }
                if x + 1 < w {
                    depth[i] = depth[i].min(depth[i - w + 1] + diagonal);
                }
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if x + 1 < w {
                depth[i] = depth[i].min(depth[i + 1] + step);
            }
            if y + 1 < h {
                depth[i] = depth[i].min(depth[i + w] + step);
                if x > 0 {
                    depth[i] = depth[i].min(depth[i + w - 1] + diagonal);
                }
                if x + 1 < w {
                    depth[i] = depth[i].min(depth[i + w + 1] + diagonal);
                }
            }
        }
    }
    for value in depth {
        *value = (*value - step * 0.5).clamp(0.0, MAX_DEPTH);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grass_boundary_and_clear_ground_match_in_scene_and_tile_fields() {
        let catalog: VegetationCatalog = ron::from_str(include_str!(
            "../../../../content/vegetation/field-current.ron"
        ))
        .unwrap();
        let population = catalog
            .populations
            .iter()
            .find(|p| p.key == "short_split_fill")
            .unwrap()
            .id;
        let page = |x, coverage| VegetationFieldPage {
            origin_xz: [x, -8.0],
            size: 16.0,
            surface: vegetation::VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
            fields: vec![vegetation::VegetationPopulationField {
                population,
                resolution: 1,
                coverage: vec![coverage],
                flow_direction: [0.0, 1.0],
            }],
        };
        let pages = [page(-16.0, 255), page(0.0, 255), page(16.0, 0)];
        let scene = BoundaryField::for_scene(&catalog, &pages);
        let tile =
            BoundaryField::bake(&catalog, &pages, Vec2::new(0.0, -8.0), Vec2::new(16.0, 8.0));
        // An internal page border is not a vegetation edge.
        assert_eq!(scene.sample(Vec2::ZERO), MAX_DEPTH);
        assert_eq!(tile.sample(Vec2::ZERO), MAX_DEPTH);
        assert_eq!(scene.sample(Vec2::new(18.0, 0.0)), 0.0);
        assert_eq!(scene.sample(Vec2::new(11.5, 0.0)), MAX_DEPTH);
        let mut previous = MAX_DEPTH;
        for i in 0..=40 {
            let p = Vec2::new(12.0 + i as f32 * 0.125, 0.0);
            let depth = scene.sample(p);
            assert!((depth - tile.sample(p)).abs() < 1e-6);
            assert!(depth <= previous);
            previous = depth;
        }
        assert_eq!(previous, 0.0);
    }
}
