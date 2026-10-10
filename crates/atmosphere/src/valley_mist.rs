//! Valley mist: where mist pools over a world, worked out once from its terrain. Mist settles
//! in low ground under a level top, so ridges and hills rise out of it, and it is thickest in
//! valleys enclosed by higher ground. Per texel the map holds the mist floor (the lowest ground
//! of the neighbourhood, smoothed, so the floor and the top above it reach level across a
//! valley), the valley share (how far the ground lies below its surroundings) and the land share
//! (mist stays over land and near shores, not out at sea). The amount
//! and depth of mist come from the atmosphere profile (`world::atmosphere::FogSettings`); the
//! sky composite integrates it along each view ray (`shaders/sky/composite.wesl`).
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::sync::Arc;

/// Radius of the neighbourhood whose lowest ground is the mist floor: about half a valley's
/// width, so a floor reaches from the valley bottom up both sides.
const FLOOR_RADIUS: f32 = 256.0;
/// Smoothing of the floor, so its top has no steps where the lowest point changes.
const FLOOR_BLUR: f32 = 160.0;
/// Ground is a valley where it lies below the average of its surroundings within this
/// distance, fully so at [`VALLEY_DEPTH`] below.
const SURROUNDINGS: f32 = 384.0;
const VALLEY_DEPTH: f32 = 30.0;
const VALLEY_BLUR: f32 = 96.0;
/// Mist thins out over the sea within about this distance of the shore.
const SHORE_BLUR: f32 = 192.0;

/// Mist floor and valley share over a whole world space, in world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct MistMap {
    /// World XZ of the first texel's outer corner.
    pub origin: [f64; 2],
    pub size: UVec2,
    pub metres_per_texel: f32,
    /// Row-major from `origin`: mist floor height, valley share 0..1, ground height, land share
    /// 0..1.
    pub texels: Vec<[f32; 4]>,
}

impl MistMap {
    /// The map of a height grid: `heights` row-major from `origin` (texel centres half a
    /// spacing in), `None` where the world has no ground there. Water and missing ground count
    /// as the sea's surface, or the lowest ground without a sea.
    pub fn from_heights(
        origin: [f64; 2],
        size: UVec2,
        metres_per_texel: f32,
        heights: &[Option<f32>],
        sea_level: Option<f32>,
    ) -> Self {
        let (w, h) = (size.x as usize, size.y as usize);
        assert_eq!(heights.len(), w * h, "mist map heights must fill the grid");
        let lowest = heights
            .iter()
            .flatten()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let water = sea_level.unwrap_or(if lowest.is_finite() { lowest } else { 0.0 });
        let ground: Vec<f32> = heights
            .iter()
            .map(|h| h.map_or(water, |h| h.max(water)))
            .collect();
        let texels = |metres: f32| (metres / metres_per_texel).round().max(1.0) as usize;

        let lowest_around = minimum(&ground, w, h, texels(FLOOR_RADIUS));
        let floor = blur(&lowest_around, w, h, texels(FLOOR_BLUR));
        let surroundings = blur(&ground, w, h, texels(SURROUNDINGS));
        let below: Vec<f32> = ground
            .iter()
            .zip(&surroundings)
            .map(|(g, s)| smoothstep((s - g) / VALLEY_DEPTH))
            .collect();
        let valley = blur(&below, w, h, texels(VALLEY_BLUR));
        let dry: Vec<f32> = heights
            .iter()
            .map(|h| {
                if h.is_some_and(|h| h > water) {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let land = blur(&dry, w, h, texels(SHORE_BLUR));
        Self {
            origin,
            size,
            metres_per_texel,
            texels: (0..w * h)
                .map(|i| {
                    [
                        floor[i].min(ground[i]),
                        valley[i],
                        ground[i],
                        smoothstep(2.0 * land[i]),
                    ]
                })
                .collect(),
        }
    }

    /// The texel holding a world XZ position, clamped to the map.
    pub fn texel_at(&self, world: [f64; 2]) -> [f32; 4] {
        let cell = |axis: usize, side: u32| {
            ((world[axis] - self.origin[axis]) / f64::from(self.metres_per_texel))
                .floor()
                .clamp(0.0, f64::from(side - 1)) as usize
        };
        self.texels[cell(1, self.size.y) * self.size.x as usize + cell(0, self.size.x)]
    }
}

/// The published map. Without one the sky composite draws no valley mist.
#[derive(Resource, Default)]
pub struct ValleyMist {
    map: Option<Arc<MistMap>>,
    revision: u64,
    sea_level: f32,
}

impl ValleyMist {
    /// The world's sea level, where the ground haze is thickest; 0 without a sea.
    pub fn set_sea_level(&mut self, sea_level: Option<f32>) {
        self.sea_level = sea_level.unwrap_or(0.0);
    }

    pub fn sea_level(&self) -> f32 {
        self.sea_level
    }

    pub fn publish(&mut self, map: Arc<MistMap>) {
        self.map = Some(map);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn disable(&mut self) {
        if self.map.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn map(&self) -> Option<&Arc<MistMap>> {
        self.map.as_ref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Shader parameters for render coordinates whose origin lies at `render_origin` in the
    /// world: the map's first corner in render XZ, metres per texel (0 when off).
    pub fn parameters(&self, render_origin: [f64; 2]) -> [f32; 4] {
        match &self.map {
            Some(map) => [
                (map.origin[0] - render_origin[0]) as f32,
                (map.origin[1] - render_origin[1]) as f32,
                map.metres_per_texel,
                1.0,
            ],
            None => [0.0; 4],
        }
    }

    pub(crate) fn image() -> Image {
        let mut image = Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            half_floats(&[[0.0; 4]]),
            TextureFormat::Rgba16Float,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::ClampToEdge,
            address_mode_v: ImageAddressMode::ClampToEdge,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        });
        image
    }

    /// Replaces the image with the published map. Its size changes with the world, so Bevy
    /// makes a new texture.
    pub(crate) fn write(&self, image: &mut Image) {
        let Some(map) = &self.map else {
            return;
        };
        image.texture_descriptor.size = Extent3d {
            width: map.size.x,
            height: map.size.y,
            depth_or_array_layers: 1,
        };
        image.data = Some(half_floats(&map.texels));
    }
}

fn half_floats(texels: &[[f32; 4]]) -> Vec<u8> {
    let bits: Vec<u16> = texels
        .iter()
        .flatten()
        .map(|&v| half::f16::from_f32(v).to_bits())
        .collect();
    bytemuck::cast_slice(&bits).to_vec()
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The lowest value within `radius` texels along each axis (a square neighbourhood).
fn minimum(values: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut rows = vec![0.0; w * h];
    for y in 0..h {
        let row = &values[y * w..(y + 1) * w];
        for x in 0..w {
            let (low, high) = (x.saturating_sub(radius), (x + radius).min(w - 1));
            rows[y * w + x] = row[low..=high]
                .iter()
                .copied()
                .fold(f32::INFINITY, f32::min);
        }
    }
    let mut out = vec![0.0; w * h];
    for x in 0..w {
        for y in 0..h {
            let (low, high) = (y.saturating_sub(radius), (y + radius).min(h - 1));
            out[y * w + x] = (low..=high).fold(f32::INFINITY, |m, v| m.min(rows[v * w + x]));
        }
    }
    out
}

/// Two passes of a box filter `radius` texels wide each way, close to a Gaussian. Edges
/// repeat the outermost texel.
fn blur(values: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut out = values.to_vec();
    for _ in 0..2 {
        out = box_pass(&out, w, h, radius, true);
        out = box_pass(&out, w, h, radius, false);
    }
    out
}

fn box_pass(values: &[f32], w: usize, h: usize, radius: usize, along_x: bool) -> Vec<f32> {
    let (lines, length) = if along_x { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| {
        if along_x { line * w + i } else { i * w + line }
    };
    let mut out = vec![0.0; w * h];
    let span = (2 * radius + 1) as f64;
    for line in 0..lines {
        let sample = |i: isize| values[at(line, i.clamp(0, length as isize - 1) as usize)];
        let mut sum: f64 = (-(radius as isize)..=radius as isize)
            .map(|i| f64::from(sample(i)))
            .sum();
        for i in 0..length {
            out[at(line, i)] = (sum / span) as f32;
            let i = i as isize;
            sum +=
                f64::from(sample(i + radius as isize + 1)) - f64::from(sample(i - radius as isize));
        }
    }
    out
}

/// Mean share of full mist density along a straight segment through the mist's soft top, as
/// `mist_share` in the shader. `ua` and `ub` are the segment's ends in fade widths below the
/// top: density rises linearly from 0 at the top to full one fade width below it.
pub fn mist_share(ua: f32, ub: f32) -> f32 {
    // Integral of the density profile clamp(u, 0, 1).
    let integral = |u: f32| {
        if u <= 0.0 {
            0.0
        } else if u < 1.0 {
            0.5 * u * u
        } else {
            u - 0.5
        }
    };
    if (ua - ub).abs() < 1e-4 {
        return (0.5 * (ua + ub)).clamp(0.0, 1.0);
    }
    (integral(ua) - integral(ub)) / (ua - ub)
}

/// Optical depth of exponential ground haze along a ray, as `haze_depth` in the shader:
/// extinction `density` per metre at `base` and below, thinning by e every `height` metres
/// above it. The ray starts at height `y`, climbs `dy` per metre and runs `length` metres.
pub fn haze_depth(density: f32, base: f32, height: f32, y: f32, dy: f32, length: f32) -> f32 {
    let start = density * (-(y - base).max(0.0) / height).exp();
    // Climb in thinning heights, stopping where the ray would sink below the base.
    let climb = (dy * length / height).max(-(y - base).max(0.0) / height);
    if climb.abs() < 1e-4 {
        return start * length * (1.0 - 0.5 * climb);
    }
    start * length * (1.0 - (-climb).exp()) / climb
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight valley along Z: ground rising 0.25 m per metre to either side of x = 1,000 m
    /// over a 2 km square at 16 m, with its floor at 40 m, and a sea below 0 m beyond x = 1,800.
    fn valley() -> MistMap {
        let size = UVec2::new(128, 128);
        let heights: Vec<Option<f32>> = (0..size.x * size.y)
            .map(|i| {
                let x = (i % size.x) as f32 * 16.0 + 8.0;
                Some(if x > 1800.0 {
                    -5.0
                } else {
                    40.0 + 0.25 * (x - 1000.0).abs().min(500.0)
                })
            })
            .collect();
        MistMap::from_heights([0.0, 0.0], size, 16.0, &heights, Some(0.0))
    }

    #[test]
    fn mist_lies_level_across_a_valley_and_thickest_in_it() {
        let map = valley();
        let at = |x: f64| map.texel_at([x, 1000.0]);
        let bottom = at(1000.0);
        assert!((bottom[0] - 40.0).abs() < 1.0, "floor {}", bottom[0]);
        assert!(bottom[1] > 0.9, "valley share {}", bottom[1]);
        // Halfway up a side the floor still lies near the valley bottom: the top is level.
        let side = at(1160.0);
        assert!(side[2] > 75.0);
        assert!(side[0] < 52.0, "floor {}", side[0]);
        // Ridges are not valleys.
        assert!(at(1500.0)[1] < 0.2, "ridge share {}", at(1500.0)[1]);
        // The floor never lies above the ground.
        assert!(map.texels.iter().all(|t| t[0] <= t[2]));
        // The sea counts at its surface, and holds mist only near the shore.
        assert_eq!(at(1950.0)[2], 0.0);
        assert_eq!(at(1000.0)[3], 1.0);
        assert!(at(1830.0)[3] > 0.5);
        assert!(at(2000.0)[3] < 0.2, "{}", at(2000.0)[3]);
    }

    #[test]
    fn missing_ground_is_water() {
        let heights = [Some(10.0), None, Some(30.0), Some(20.0)];
        let map = MistMap::from_heights([0.0, 0.0], UVec2::new(2, 2), 16.0, &heights, None);
        assert_eq!(map.texels[1][2], 10.0);
        let map = MistMap::from_heights([0.0, 0.0], UVec2::new(2, 2), 16.0, &heights, Some(15.0));
        assert_eq!(map.texels[1][2], 15.0);
        assert_eq!(map.texels[0][2], 15.0);
    }

    /// Numerical reference: the mean of `profile(u)` over a straight segment.
    fn mean(profile: impl Fn(f32) -> f32, a: f32, b: f32) -> f32 {
        let n = 20_000;
        (0..n)
            .map(|i| profile(a + (b - a) * (i as f32 + 0.5) / n as f32))
            .sum::<f32>()
            / n as f32
    }

    #[test]
    fn mist_share_matches_the_soft_top_profile() {
        let profile = |u: f32| u.clamp(0.0, 1.0);
        for (a, b) in [
            (-2.0, 3.0),
            (0.2, 0.7),
            (4.0, 2.0),
            (-1.0, -0.5),
            (0.5, 0.5),
            (3.0, -3.0),
        ] {
            let expected = mean(profile, a, b);
            assert!(
                (mist_share(a, b) - expected).abs() < 1e-3,
                "{a} {b}: {} vs {expected}",
                mist_share(a, b)
            );
        }
    }

    #[test]
    fn haze_depth_matches_its_density_along_the_ray() {
        let (density, base, height) = (4e-4, 0.0, 150.0);
        let at = |y: f32| density * (-(y - base).max(0.0) / height).exp();
        for (y, dy, length) in [
            (2.0, 0.0, 5000.0),
            (2.0, 0.05, 8000.0),
            (500.0, -0.1, 4000.0),
            (300.0, -0.2, 1000.0),
        ] {
            let expected = mean(|t| at(y + dy * t), 0.0, length) * length;
            let got = haze_depth(density, base, height, y, dy, length);
            assert!(
                (got - expected).abs() < 1e-3 * expected.max(1e-3),
                "{y} {dy} {length}: {got} vs {expected}"
            );
        }
    }
}
