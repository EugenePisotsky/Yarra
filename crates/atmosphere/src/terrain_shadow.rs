//! Distant terrain shadows and sky occlusion. Past the shadow maps nothing cast shadows, so
//! mountains under a low sun turned into evenly shaded clay: ridges threw no shade into the
//! gullies and valleys beside them, and valley floors saw as much sky as summits. From the
//! world's terrain at one coarse level (read once with the valley mist's, `TerrainRelief`) a
//! map holds per texel the horizon towards the light's compass direction (the tangent of its
//! elevation and how far off it lies), the ground height and the share of the sky the terrain
//! around leaves open. The horizon depends only on the compass direction, so a light climbing or
//! sinking needs no new search: the map follows the sun, or the moon at night, when its compass
//! direction moves. Lit materials dim the light below the horizon past the shadow maps, and the
//! sky's light everywhere (`shaders/lighting/terrain_shadow.wesl`).
//!
//! The searches run on the GPU (`gpu`, `shaders/lighting/terrain_horizon.wesl`), a band of rows
//! a frame, into the map in place: the sky share once per world, the horizon whenever the
//! light's direction moves. The main thread only follows the light. (On the CPU, 54 ms on every
//! core every second or two of game time, and a 13 MB re-upload, dropped frames at that
//! interval.)
use crate::{WorldMoon, WorldSun};

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
};

use std::sync::Arc;

pub(crate) mod gpu;

/// Longest side of the map; coarser terrain is averaged down to it.
const MAX_SIDE: usize = 2048;

/// Horizon search: the first sample a texel away, each next this much further, up to
/// [`HORIZON_REACH`]. Mountains a few kilometres off still shade a valley at sunset. The
/// shader's constants must match (its GPU test compares it with the reference here).
#[cfg(test)]
const HORIZON_GROWTH: f32 = 1.08;

const HORIZON_REACH: f32 = 6000.0;

/// Sky occlusion: compass directions searched and how far. Further terrain hides little sky.
#[cfg(test)]
const SKY_DIRECTIONS: usize = 8;

#[cfg(test)]
const SKY_GROWTH: f32 = 1.15;

#[cfg(test)]
const SKY_REACH: f32 = 600.0;

/// Search again when the light's compass direction moves further than this: about every 0.8 s
/// of game time around noon at the default day length. A shadow a kilometre long then shifts
/// sideways by about 4 m, half a texel.
const REBUILD_DEGREES: f32 = 0.25;

/// The world's ground on one coarse grid, in world coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainRelief {
    /// World XZ of the first texel's outer corner.
    pub origin: [f64; 2],
    pub size: UVec2,
    pub metres_per_texel: f32,
    /// Row-major from `origin`.
    pub heights: Vec<f32>,
}

impl TerrainRelief {
    /// The relief of a height grid: `heights` row-major from `origin` (texel centres half a
    /// spacing in), `None` where the world has no ground, which then lies at the sea's level, or
    /// the lowest ground without a sea. Grids larger than [`MAX_SIDE`] are averaged down.
    pub fn from_heights(
        origin: [f64; 2],
        size: UVec2,
        metres_per_texel: f32,
        heights: &[Option<f32>],
        sea_level: Option<f32>,
    ) -> Self {
        let (mut w, mut h) = (size.x as usize, size.y as usize);
        assert_eq!(
            heights.len(),
            w * h,
            "terrain relief heights must fill the grid"
        );
        let lowest = heights
            .iter()
            .flatten()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let water = sea_level.unwrap_or(if lowest.is_finite() { lowest } else { 0.0 });
        let mut grid: Vec<f32> = heights.iter().map(|v| v.unwrap_or(water)).collect();
        let mut spacing = metres_per_texel;

        while w.max(h) > MAX_SIDE {
            let (half_w, half_h) = (w.div_ceil(2), h.div_ceil(2));
            let mut half = vec![0.0; half_w * half_h];

            for z in 0..half_h {
                for x in 0..half_w {
                    let mut sum = 0.0;

                    for (dz, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        sum += grid[(2 * z + dz).min(h - 1) * w + (2 * x + dx).min(w - 1)];
                    }

                    half[z * half_w + x] = sum * 0.25;
                }
            }

            (grid, w, h, spacing) = (half, half_w, half_h, spacing * 2.0);
        }

        Self {
            origin,
            size: UVec2::new(w as u32, h as u32),
            metres_per_texel: spacing,
            heights: grid,
        }
    }
}

/// The relief and the compass direction the map holds the horizon towards. Without a relief
/// shaders treat everything as open to the light and the sky.
#[derive(Resource, Default)]
pub struct TerrainShadow {
    relief: Option<Arc<TerrainRelief>>,
    azimuth: Option<Vec2>,
    /// Changes with the relief, whose images are then rewritten.
    revision: u64,
}

impl TerrainShadow {
    /// A new world's relief; the map follows the light from the next frame.
    pub fn set_relief(&mut self, relief: Arc<TerrainRelief>) {
        self.relief = Some(relief);
        self.azimuth = None;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn disable(&mut self) {
        if self.relief.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
        self.azimuth = None;
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Shader parameters for render coordinates whose origin lies at `render_origin` in the
    /// world: the map's first corner in render XZ, metres per texel (0 when off).
    pub fn parameters(&self, render_origin: [f64; 2]) -> [f32; 4] {
        match (&self.relief, self.azimuth) {
            (Some(relief), Some(_)) => [
                (relief.origin[0] - render_origin[0]) as f32,
                (relief.origin[1] - render_origin[1]) as f32,
                relief.metres_per_texel,
                1.0,
            ],
            _ => [0.0; 4],
        }
    }

    /// The compass direction the map holds the horizon towards, as unit XZ.
    pub fn light(&self) -> [f32; 4] {
        self.azimuth.map_or([0.0; 4], |a| [a.x, a.y, 0.0, 0.0])
    }

    /// The map lit materials sample, written by the GPU searches.
    pub(crate) fn image() -> Image {
        let mut image = Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            half_floats(&[OPEN]),
            TextureFormat::Rgba16Float,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::ClampToEdge,
            address_mode_v: ImageAddressMode::ClampToEdge,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        });
        image
    }

    /// The relief's heights for the GPU searches.
    pub(crate) fn heights_image() -> Image {
        Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            0.0_f32.to_le_bytes().to_vec(),
            TextureFormat::R32Float,
            RenderAssetUsages::RENDER_WORLD,
        )
    }

    /// Sizes an image to the relief, which makes Bevy create a new texture (once per world).
    fn sized(&self, image: &mut Image, data: impl FnOnce(&TerrainRelief) -> Vec<u8>) {
        let Some(relief) = &self.relief else {
            return;
        };
        image.texture_descriptor.size = Extent3d {
            width: relief.size.x,
            height: relief.size.y,
            depth_or_array_layers: 1,
        };
        image.data = Some(data(relief));
    }
}

/// A texel open to light and sky: horizon far below, sky all open.
const OPEN: [f32; 4] = [-4.0, 0.0, 1.0, HORIZON_REACH];

/// Follow the sun, or the moon while the sun is down: the map's direction moves once the
/// light's compass direction has moved more than [`REBUILD_DEGREES`] from it.
#[allow(clippy::type_complexity)] // The sun and moon queries.
pub(crate) fn follow_light(
    mut shadow: ResMut<TerrainShadow>,
    sun: Query<(&Transform, &DirectionalLight), (With<WorldSun>, Without<WorldMoon>)>,
    moon: Query<(&Transform, &DirectionalLight), (With<WorldMoon>, Without<WorldSun>)>,
) {
    if shadow.relief.is_none() {
        return;
    }

    let up = |light: Option<(&Transform, &DirectionalLight)>| {
        light
            .map(|(t, l)| (t.back().as_vec3(), l.illuminance))
            .filter(|(d, lux)| d.y > 0.0 && *lux > 0.0)
    };
    let Some((to_light, _)) = up(sun.single().ok()).or_else(|| up(moon.single().ok())) else {
        return;
    };
    let azimuth = Vec2::new(to_light.x, to_light.z);

    if azimuth.length_squared() < 1e-8 {
        return;
    }

    let azimuth = azimuth.normalize();
    let moved = |a: Vec2| a.dot(azimuth) < REBUILD_DEGREES.to_radians().cos();

    if shadow.azimuth.is_none_or(moved) {
        shadow.azimuth = Some(azimuth);
    }
}

/// Writes the relief into its images when a new world's arrives.
pub(crate) fn publish(
    shadow: Res<TerrainShadow>,
    assets: Option<Res<crate::environment::EnvironmentAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut published: Local<Option<u64>>,
) {
    let Some(assets) = assets else {
        return;
    };

    if *published == Some(shadow.revision) {
        return;
    }

    // The relief's heights, and a map open to light and sky until the searches fill it.
    if let Some(mut map) = images.get_mut(&assets.terrain_shadow) {
        shadow.sized(&mut map, |r| half_floats(&vec![OPEN; r.heights.len()]));
    }

    if let Some(mut heights) = images.get_mut(&assets.terrain_heights) {
        shadow.sized(&mut heights, |r| bytemuck::cast_slice(&r.heights).to_vec());
    }

    *published = Some(shadow.revision);
}

fn half_floats(texels: &[[f32; 4]]) -> Vec<u8> {
    let bits: Vec<u16> = texels
        .iter()
        .flatten()
        .map(|&v| half::f16::from_f32(v).to_bits())
        .collect();
    bytemuck::cast_slice(&bits).to_vec()
}

/// The searches on the CPU, as the shader runs them: the reference its GPU test checks.
#[cfg(test)]
pub(crate) mod reference {
    use super::*;

    /// Bilinear height at texel coordinates (texel centres at whole numbers), clamped.
    fn height(relief: &TerrainRelief, x: f32, z: f32) -> f32 {
        let (w, h) = (relief.size.x as usize, relief.size.y as usize);
        let x = x.clamp(0.0, (w - 1) as f32);
        let z = z.clamp(0.0, (h - 1) as f32);
        let (x0, z0) = (x as usize, z as usize);
        let (x1, z1) = ((x0 + 1).min(w - 1), (z0 + 1).min(h - 1));
        let (fx, fz) = (x - x0 as f32, z - z0 as f32);
        let row =
            |z: usize| relief.heights[z * w + x0] * (1.0 - fx) + relief.heights[z * w + x1] * fx;
        row(z0) * (1.0 - fz) + row(z1) * fz
    }

    /// The highest horizon from texel `(x, z)` towards `direction` (texel units, unit length),
    /// as the tangent of its elevation and how far off it lies in metres. Ground falling away
    /// on every side gives a negative tangent.
    fn horizon(
        relief: &TerrainRelief,
        x: usize,
        z: usize,
        direction: Vec2,
        growth: f32,
        reach: f32,
    ) -> (f32, f32) {
        let (w, h) = (relief.size.x as f32, relief.size.y as f32);
        let here = relief.heights[z * relief.size.x as usize + x];
        let (mut best, mut at) = (f32::NEG_INFINITY, reach);
        let mut r = 1.0_f32;

        while r * relief.metres_per_texel <= reach {
            let (sx, sz) = (x as f32 + direction.x * r, z as f32 + direction.y * r);

            // A thousandth of a texel of slack: an axis direction's near-zero component must
            // not leave the grid at its edge.
            if sx < -1e-3 || sz < -1e-3 || sx > w - 1.0 + 1e-3 || sz > h - 1.0 + 1e-3 {
                break;
            }

            let tangent = (height(relief, sx, sz) - here) / (r * relief.metres_per_texel);

            if tangent > best {
                (best, at) = (tangent, r * relief.metres_per_texel);
            }

            r = (r * growth).max(r + 1.0);
        }

        if best.is_finite() {
            (best, at)
        } else {
            (0.0, reach)
        }
    }

    /// Share of the sky above each texel the terrain around leaves open, for flat ground:
    /// 1 - mean of sin²(horizon elevation) over [`SKY_DIRECTIONS`].
    pub(crate) fn sky(relief: &TerrainRelief) -> Vec<f32> {
        let directions: Vec<Vec2> = (0..SKY_DIRECTIONS)
            .map(|i| Vec2::from_angle(std::f32::consts::TAU * i as f32 / SKY_DIRECTIONS as f32))
            .collect();
        texels(relief, |x, z| {
            let hidden: f32 = directions
                .iter()
                .map(|&d| {
                    let t = horizon(relief, x, z, d, SKY_GROWTH, SKY_REACH).0.max(0.0);
                    t * t / (1.0 + t * t)
                })
                .sum();
            1.0 - hidden / SKY_DIRECTIONS as f32
        })
    }

    /// The map: horizon tangent, ground height, sky share, horizon distance.
    pub(crate) fn map(relief: &TerrainRelief, azimuth: Vec2) -> Vec<[f32; 4]> {
        let direction = azimuth.normalize_or(Vec2::X);
        let sky = sky(relief);
        let w = relief.size.x as usize;
        texels(relief, |x, z| {
            let (tangent, distance) =
                horizon(relief, x, z, direction, HORIZON_GROWTH, HORIZON_REACH);
            [
                tangent.clamp(-4.0, 16.0),
                relief.heights[z * w + x],
                sky[z * w + x],
                distance,
            ]
        })
    }

    fn texels<T>(relief: &TerrainRelief, f: impl Fn(usize, usize) -> T) -> Vec<T> {
        let w = relief.size.x as usize;
        (0..relief.heights.len()).map(|i| f(i % w, i / w)).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 1 km grid at 8 m with a 200 m wall across its middle (x = 60..64 texels).
    pub(crate) fn wall() -> TerrainRelief {
        let size = UVec2::new(125, 125);
        let heights: Vec<Option<f32>> = (0..125 * 125)
            .map(|i| {
                Some(if (60..64).contains(&(i % 125)) {
                    200.0
                } else {
                    0.0
                })
            })
            .collect();
        TerrainRelief::from_heights([0.0, 0.0], size, 8.0, &heights, None)
    }

    #[test]
    fn a_wall_shades_the_ground_on_its_far_side_only() {
        let relief = wall();

        // Light from +X: the wall rises east of texels west of it.
        let map = reference::map(&relief, Vec2::X);
        let at = |x: usize| map[62 * 125 + x];
        let west = at(50);

        // Ten texels (80 m) from the wall's near face, 200 m up: tangent 2.5.
        assert!((west[0] - 200.0 / 80.0).abs() < 0.05, "{west:?}");
        assert!((west[3] - 80.0).abs() < 1.0);

        // East of the wall nothing rises towards the light.
        assert!(at(80)[0] <= 0.0);

        // Light from -X shades the east side instead.
        let back = reference::map(&relief, -Vec2::X);
        assert!(back[62 * 125 + 80][0] > 1.0);
        assert!(back[62 * 125 + 50][0] <= 0.0);
    }

    #[test]
    fn the_sky_closes_in_beside_the_wall_and_stays_open_away_from_it() {
        let sky = reference::sky(&wall());
        let near = sky[62 * 125 + 58];
        let open = sky[62 * 125 + 5];
        assert!(near < 0.8, "{near}");
        assert!(open > near && open > 0.95, "{open}");
        assert!(sky.iter().all(|s| (0.0..=1.0).contains(s)));
    }

    #[test]
    fn large_grids_are_averaged_down_to_the_longest_side() {
        let size = UVec2::new(3000, 10);
        let heights = vec![Some(1.0); 3000 * 10];
        let relief = TerrainRelief::from_heights([0.0, 0.0], size, 4.0, &heights, None);
        assert_eq!(relief.size, UVec2::new(1500, 5));
        assert_eq!(relief.metres_per_texel, 8.0);
        assert!(relief.heights.iter().all(|&h| h == 1.0));
    }

    #[test]
    fn parameters_place_the_map_in_render_coordinates_once_it_follows_a_light() {
        let mut shadow = TerrainShadow::default();
        assert_eq!(shadow.parameters([0.0, 0.0]), [0.0; 4]);
        shadow.set_relief(Arc::new(wall()));
        assert_eq!(shadow.parameters([0.0, 0.0]), [0.0; 4]);
        shadow.azimuth = Some(Vec2::X);
        assert_eq!(shadow.parameters([100.0, -50.0]), [-100.0, 50.0, 8.0, 1.0]);
        assert_eq!(shadow.light(), [1.0, 0.0, 0.0, 0.0]);
    }
}
