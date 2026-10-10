//! How waves come ashore, worked out once from a world's terrain. Per texel the map holds the
//! water's depth below the sea level (negative over land) and the time a wave crest takes to get
//! there from deep water, with its gradient. Wave crests are the lines of equal time: they turn
//! to lie along the shore, wrap into bays and close up as the water shallows and slows them, as
//! swell does over a sloping seabed. The sky composite turns the map into breaking waves, their
//! foam and the swash on the beach (`shaders/water/sea.wesl`).
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::{cmp::Reverse, collections::BinaryHeap, sync::Arc};

/// Period of the swell coming ashore, seconds. The wave clock wraps after whole periods
/// (`crate::environment::WAVE_PERIOD`).
pub(crate) const SURF_PERIOD: f32 = 9.0;
/// Depth taken for ground missing from the map: open sea.
const OPEN_SEA_DEPTH: f32 = 50.0;
/// Crests set out from water this deep, all at once: the surf is worked out from here inwards.
const DEEP: f32 = 20.0;
/// Below this depth a step integrates the shallow-water speed exactly.
const SHALLOW: f32 = 5.0;
/// Height of the broken waves' bore: it runs at the square root of g times the depth and its
/// own height, so crests keep coming ashore where linear waves would stall at the waterline.
const BORE: f32 = 0.4;
/// Arrival time of water no crest reaches (a basin below the sea level inland).
pub(crate) const UNREACHED: f32 = 600.0;
const GRAVITY: f32 = 9.81;

/// The shore map over a whole world space, in world coordinates, on the valley mist's grid.
#[derive(Clone, Debug, PartialEq)]
pub struct ShoreMap {
    /// World XZ of the first texel's outer corner.
    pub origin: [f64; 2],
    pub size: UVec2,
    pub metres_per_texel: f32,
    /// Row-major from `origin`: depth below the sea level in metres (negative over land),
    /// seconds a crest takes to get here from deep water (over land, to the waterline next to
    /// it), and that time's gradient along X and Z in seconds per metre.
    pub texels: Vec<[f32; 4]>,
}

/// Speed of a wave crest of [`SURF_PERIOD`] over `depth` metres of water: the linear dispersion
/// relation by Eckart's explicit form, within a few per cent; the square root of g times depth
/// in shallow water, the deep-water speed in deep.
pub(crate) fn crest_speed(depth: f32) -> f32 {
    let omega = std::f32::consts::TAU / SURF_PERIOD;
    let deep = omega * omega / GRAVITY;
    let k = deep / (deep * depth.max(1e-4)).tanh().sqrt();
    omega / k
}

impl ShoreMap {
    /// The map of a height grid laid out as for `crate::valley_mist::MistMap::from_heights`:
    /// `heights` row-major from `origin`, `None` where the world has no ground (open sea).
    pub fn from_heights(
        origin: [f64; 2],
        size: UVec2,
        metres_per_texel: f32,
        heights: &[Option<f32>],
        sea_level: f32,
    ) -> Self {
        let (w, h) = (size.x as usize, size.y as usize);
        assert_eq!(heights.len(), w * h, "shore map heights must fill the grid");
        let depth: Vec<f32> = heights
            .iter()
            .map(|g| g.map_or(OPEN_SEA_DEPTH, |g| sea_level - g))
            .collect();
        let time = travel_times(&depth, w, h, metres_per_texel);
        let at = |x: isize, z: isize| {
            time[z.clamp(0, h as isize - 1) as usize * w + x.clamp(0, w as isize - 1) as usize]
        };
        let texels = (0..w * h)
            .map(|i| {
                let (x, z) = ((i % w) as isize, (i / w) as isize);
                let span = 2.0 * metres_per_texel;
                [
                    depth[i],
                    time[i],
                    (at(x + 1, z) - at(x - 1, z)) / span,
                    (at(x, z + 1) - at(x, z - 1)) / span,
                ]
            })
            .collect();
        Self {
            origin,
            size,
            metres_per_texel,
            texels,
        }
    }

    #[cfg(test)]
    /// The texel holding a world XZ position, clamped to the map.
    pub(crate) fn texel_at(&self, world: [f64; 2]) -> [f32; 4] {
        let cell = |axis: usize, side: u32| {
            ((world[axis] - self.origin[axis]) / f64::from(self.metres_per_texel))
                .floor()
                .clamp(0.0, f64::from(side - 1)) as usize
        };
        self.texels[cell(1, self.size.y) * self.size.x as usize + cell(0, self.size.x)]
    }
}

/// Seconds a crest takes to reach each texel from deep water, by Dijkstra's search in from the
/// water at least [`DEEP`] over the 8-connected grid. Each step takes the crest speed's mean
/// slowness over its length; in shallow water it integrates the square root of g times the depth
/// and the bore's height along a seabed rising linearly between the texels, which a mean would
/// get badly wrong near the waterline. Land takes the time the crest reaches the waterline
/// next to it and passes nothing on.
fn travel_times(depth: &[f32], w: usize, h: usize, spacing: f32) -> Vec<f32> {
    let slowness = |d: f32| 1.0 / crest_speed(d);
    let mut time = vec![UNREACHED; w * h];
    let mut queue = BinaryHeap::new();
    for (i, &d) in depth.iter().enumerate() {
        if d >= DEEP {
            time[i] = 0.0;
            queue.push(Reverse((0_u32, i)));
        }
    }
    while let Some(Reverse((bits, i))) = queue.pop() {
        let here = f32::from_bits(bits);
        let a = depth[i];
        if here > time[i] || a <= 0.0 {
            continue;
        }
        let (x, z) = ((i % w) as isize, (i / w) as isize);
        for (dx, dz) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let (nx, nz) = (x + dx, z + dz);
            if nx < 0 || nz < 0 || nx >= w as isize || nz >= h as isize {
                continue;
            }
            let j = nz as usize * w + nx as usize;
            let b = depth[j];
            let length = spacing
                * if dx != 0 && dz != 0 {
                    2_f32.sqrt()
                } else {
                    1.0
                };
            let step = if b <= 0.0 {
                // To the waterline, where the depth reaches zero.
                let wet = length * a / (a - b);
                2.0 * wet / (GRAVITY.sqrt() * ((a + BORE).sqrt() + BORE.sqrt()))
            } else if a.max(b) < SHALLOW {
                2.0 * length / (GRAVITY.sqrt() * ((a + BORE).sqrt() + (b + BORE).sqrt()))
            } else {
                length * 0.5 * (slowness(a) + slowness(b))
            };
            let arrival = here + step;
            if arrival < time[j] {
                time[j] = arrival;
                queue.push(Reverse((arrival.to_bits(), j)));
            }
        }
    }
    time
}

/// The published map. Without one the sea has no surf.
#[derive(Resource, Default)]
pub struct Shore {
    map: Option<Arc<ShoreMap>>,
    revision: u64,
}

impl Shore {
    pub fn publish(&mut self, map: Arc<ShoreMap>) {
        self.map = Some(map);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn disable(&mut self) {
        if self.map.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn map(&self) -> Option<&Arc<ShoreMap>> {
        self.map.as_ref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Shader parameters for render coordinates whose origin lies at `render_origin` in the
    /// world: the map's first corner in render XZ, metres per texel and the swell's period (all
    /// 0 without a map).
    pub fn parameters(&self, render_origin: [f64; 2]) -> [f32; 4] {
        match &self.map {
            Some(map) => [
                (map.origin[0] - render_origin[0]) as f32,
                (map.origin[1] - render_origin[1]) as f32,
                map.metres_per_texel,
                SURF_PERIOD,
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

    /// Replaces the image with the published map.
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

/// Half floats hold travel times to 0.03 s within the surf zone and 0.06 s out to the map's
/// longest.
fn half_floats(texels: &[[f32; 4]]) -> Vec<u8> {
    let bits: Vec<u16> = texels
        .iter()
        .flatten()
        .map(|&v| half::f16::from_f32(v).to_bits())
        .collect();
    bytemuck::cast_slice(&bits).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight beach along Z: land west of x = 0, the seabed falling `slope` per metre
    /// eastwards to a 30 m floor.
    fn beach(slope: f32) -> ShoreMap {
        let (w, h, spacing) = (64_usize, 8_usize, 8.0_f32);
        let heights: Vec<_> = (0..w * h)
            .map(|i| {
                let x = ((i % w) as f32 + 0.5) * spacing - 64.0;
                Some((-x * slope).max(-30.0))
            })
            .collect();
        ShoreMap::from_heights(
            [0.0, 0.0],
            UVec2::new(w as u32, h as u32),
            spacing,
            &heights,
            0.0,
        )
    }

    #[test]
    fn crests_travel_at_the_square_root_of_g_times_depth_in_shallow_water() {
        assert!((crest_speed(1.0) - GRAVITY.sqrt()).abs() < 0.05);
        // Deep water: g T / 2 pi.
        let deep = GRAVITY * SURF_PERIOD / std::f32::consts::TAU;
        assert!((crest_speed(500.0) - deep).abs() / deep < 0.01);
        assert!(crest_speed(5.0) < crest_speed(10.0));
    }

    #[test]
    fn crests_come_in_from_deep_water_and_slow_down_as_it_shallows() {
        let slope = 0.08;
        let map = beach(slope);
        let at = |x: f32| map.texel_at([f64::from(x) + 64.0, 32.0]);
        // Out from the 20 m line, crests set out together.
        assert_eq!(at(300.0)[1], 0.0);
        let times: Vec<f32> = (0..9).map(|i| at(236.0 - 28.0 * i as f32)[1]).collect();
        assert!(times.windows(2).all(|t| t[1] > t[0]), "{times:?}");
        // The time follows the crest speed's integral from the 20 m line, within the grid's
        // error.
        let integral = |x: f32| {
            let steps = 2000;
            let deep = DEEP / slope;
            (0..steps)
                .map(|k| {
                    let s = x + (deep - x) * (k as f32 + 0.5) / steps as f32;
                    (deep - x) / steps as f32 / crest_speed(s * slope)
                })
                .sum::<f32>()
        };
        for x in [20.0, 100.0, 180.0] {
            let (found, expected) = (at(x)[1], integral(x));
            assert!(
                (found - expected).abs() < 0.08 * expected + 0.5,
                "{x}: {found} {expected}"
            );
        }
        // The gradient points to the shore, at the crest's slowness.
        let texel = at(100.0);
        assert!(
            (texel[2] + 1.0 / crest_speed(texel[0])).abs() < 0.03,
            "{texel:?}"
        );
        assert!(texel[3].abs() < 1e-3);
        // Land takes the time the crest reaches the waterline.
        let land = at(-4.0);
        assert!(
            land[0] < 0.0 && land[1] > at(4.0)[1] && land[1] < at(4.0)[1] + 5.0,
            "{land:?}"
        );
    }

    #[test]
    fn missing_ground_is_open_sea_and_basins_inland_are_not_reached() {
        let heights = vec![Some(-1.0), Some(5.0), Some(-1.0), None];
        let map = ShoreMap::from_heights([0.0, 0.0], UVec2::new(4, 1), 10.0, &heights, 0.0);
        assert_eq!(map.texels[3][0], OPEN_SEA_DEPTH);
        assert_eq!(map.texels[3][1], 0.0);
        assert!(map.texels[2][1] > 0.0 && map.texels[1][1] > map.texels[2][1]);
        assert_eq!(map.texels[0][1], UNREACHED);
    }
}
