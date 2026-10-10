//! Distant forest shadows: a coarse top-down map of crowns around the camera. Past the
//! shadow maps, crowns, trunks and ground march towards the sun through it
//! (`shaders/clouds/forest_shadow.wesl`), so distant forests keep the shade their trees cast on
//! each other and on the ground instead of turning flat and pale where the cascades end. At
//! every distance the same map holds back the sky light under the crowns
//! ([`ForestSkyOcclusion`]). Callers supply the crowns; the engine rasterizes every far-object
//! tree.
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

/// Texels per edge and metres per texel: 4,096 m around the camera, the far-object range.
pub const SIZE: u32 = 1024;
pub const METRES_PER_TEXEL: f32 = 4.0;
/// Texels around each texel whose crowns its local canopy top covers: a shadow march starting
/// there ends above them, and a point with no crown this near above it skips the march.
const NEIGHBOURHOOD: usize = 24;
/// Crown heights stored where no crown reaches a texel.
const OPEN: f32 = -1.0e4;
/// Mip levels above the map (8, 16 and 32 m texels) for the sky light under the crowns: each
/// holds the highest crown top, lowest crown base, the average share of the sky level 0's
/// crowns let through (`exp(-density)`, not density) and the highest canopy top.
pub const SKY_LEVELS: u32 = 3;

/// One tree's crown as an upright ellipsoid of foliage, in render-local XZ.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ForestCrown {
    pub centre: Vec2,
    pub radius: f32,
    /// Absolute heights of the foliage base and top.
    pub bottom: f32,
    pub top: f32,
    /// Share of the crown's silhouette that foliage covers, 0..1.
    pub opacity: f32,
}

/// Rasterized crowns: per texel the highest crown top, lowest crown base, foliage density
/// (opaque crown share of a texel's area, adding up where crowns overlap) and the local canopy
/// top, the highest crown top within [`NEIGHBOURHOOD`] texels.
pub struct ForestShadowMap {
    origin: Vec2,
    texels: Vec<[f32; 4]>,
    /// [`SKY_LEVELS`] mip levels, finest first.
    levels: Vec<Vec<[f32; 4]>>,
    tallest: f32,
}

impl ForestShadowMap {
    /// Rasterize crowns into a square centred on `centre`, snapped to whole texels.
    pub fn rasterize(centre: Vec2, crowns: impl IntoIterator<Item = ForestCrown>) -> Self {
        let half = SIZE as f32 * METRES_PER_TEXEL * 0.5;
        let origin = ((centre - Vec2::splat(half)) / METRES_PER_TEXEL).floor() * METRES_PER_TEXEL;
        let mut texels = vec![[OPEN, -OPEN, 0.0, OPEN]; (SIZE * SIZE) as usize];
        let mut tallest = OPEN;
        for crown in crowns {
            if !(crown.radius > 0.0
                && crown.radius.is_finite()
                && crown.top > crown.bottom
                && crown.top.is_finite()
                && crown.bottom.is_finite()
                && crown.opacity > 0.0)
            {
                continue;
            }
            let local = (crown.centre - origin) / METRES_PER_TEXEL;
            let reach = crown.radius / METRES_PER_TEXEL + 1.0;
            let min = (local - reach).floor().max(Vec2::ZERO);
            let max = (local + reach).ceil().min(Vec2::splat(SIZE as f32 - 1.0));
            if min.x > max.x || min.y > max.y {
                continue;
            }
            let (middle, half_height) = (
                (crown.top + crown.bottom) * 0.5,
                (crown.top - crown.bottom) * 0.5,
            );
            let opacity = crown.opacity.min(1.0);
            for y in min.y as u32..=max.y as u32 {
                for x in min.x as u32..=max.x as u32 {
                    let texel_centre = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let d = (texel_centre - local).length() * METRES_PER_TEXEL / crown.radius;
                    if d >= 1.0 {
                        continue;
                    }
                    // The share of a texel a crown smaller than it covers, then an ellipsoid
                    // profile: full height at the trunk, thinning to the drip line.
                    let area = (std::f32::consts::PI * crown.radius * crown.radius
                        / (METRES_PER_TEXEL * METRES_PER_TEXEL))
                        .min(1.0);
                    let profile = (1.0 - d * d).sqrt();
                    let coverage = opacity * area * profile;
                    let texel = &mut texels[(y * SIZE + x) as usize];
                    texel[0] = texel[0].max(middle + half_height * profile);
                    texel[1] = texel[1].min(middle - half_height * profile);
                    // Overlapping crowns add their foliage: a dense stand dims more than one tree.
                    texel[2] += coverage;
                    tallest = tallest.max(texel[0]);
                }
            }
        }
        // Filtered lookups blend neighbouring texels: give empty texels next to crowns their
        // neighbours' heights (density stays 0), so an edge sample sees a sane crown layer.
        let source = texels.clone();
        for y in 0..SIZE as i32 {
            for x in 0..SIZE as i32 {
                let index = (y as u32 * SIZE + x as u32) as usize;
                if source[index][2] > 0.0 {
                    continue;
                }
                for (dx, dy) in [
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                ] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= SIZE as i32 || ny >= SIZE as i32 {
                        continue;
                    }
                    let neighbour = source[(ny as u32 * SIZE + nx as u32) as usize];
                    if neighbour[2] > 0.0 {
                        let texel = &mut texels[index];
                        texel[0] = texel[0].max(neighbour[0]);
                        texel[1] = texel[1].min(neighbour[1]);
                    }
                }
            }
        }
        // Local canopy top: a separable maximum over the neighbourhood.
        let size = SIZE as usize;
        let mut rows = vec![OPEN; size * size];
        for y in 0..size {
            let row = &texels[y * size..(y + 1) * size];
            for x in 0..size {
                let (low, high) = (
                    x.saturating_sub(NEIGHBOURHOOD),
                    (x + NEIGHBOURHOOD).min(size - 1),
                );
                rows[y * size + x] = row[low..=high]
                    .iter()
                    .filter(|t| t[2] > 0.0)
                    .fold(OPEN, |top, t| top.max(t[0]));
            }
        }
        for x in 0..size {
            for y in 0..size {
                let (low, high) = (
                    y.saturating_sub(NEIGHBOURHOOD),
                    (y + NEIGHBOURHOOD).min(size - 1),
                );
                texels[y * size + x][3] =
                    (low..=high).fold(OPEN, |top, v| top.max(rows[v * size + x]));
            }
        }
        let levels = sky_levels(&texels);
        Self {
            origin,
            texels,
            levels,
            tallest,
        }
    }

    /// Foliage depth in metres along a straight ray, at texel centres; matches the shader's
    /// march closely enough for tests and CPU queries.
    pub fn density_at(&self, point: Vec3) -> f32 {
        let texel = ((Vec2::new(point.x, point.z) - self.origin) / METRES_PER_TEXEL).floor();
        if texel.x < 0.0 || texel.y < 0.0 || texel.x >= SIZE as f32 || texel.y >= SIZE as f32 {
            return 0.0;
        }
        let [top, bottom, density, _] =
            self.texels[(texel.y as u32 * SIZE + texel.x as u32) as usize];
        if point.y > bottom && point.y < top {
            density
        } else {
            0.0
        }
    }
}

/// How much sky light the crowns above a point hold back from it
/// (`forest_sky_visibility` in the shader): 1 as their foliage does, 0 none.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct ForestSkyOcclusion(pub f32);
impl Default for ForestSkyOcclusion {
    fn default() -> Self {
        Self(1.0)
    }
}

/// The published map. Disabled until the first rebuild, so shaders treat everything as unshaded.
#[derive(Resource, Default)]
pub struct ForestShadow {
    map: Option<ForestShadowMap>,
    revision: u64,
}

impl ForestShadow {
    pub fn publish(&mut self, map: ForestShadowMap) {
        self.map = Some(map);
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn disable(&mut self) {
        if self.map.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Centre of the covered square, if published.
    pub fn centre(&self) -> Option<Vec2> {
        self.map
            .as_ref()
            .map(|map| map.origin + Vec2::splat(SIZE as f32 * METRES_PER_TEXEL * 0.5))
    }

    pub fn map(&self) -> Option<&ForestShadowMap> {
        self.map.as_ref()
    }

    /// Shader parameters: origin XZ, metres per texel (0 when off), tallest crown top.
    pub fn parameters(&self) -> [f32; 4] {
        match &self.map {
            Some(map) if map.tallest > OPEN => {
                [map.origin.x, map.origin.y, METRES_PER_TEXEL, map.tallest]
            }
            _ => [0.0; 4],
        }
    }

    pub(crate) fn image() -> Image {
        let empty = [OPEN, -OPEN, 0.0, OPEN];
        let open_sky = [OPEN, -OPEN, 1.0, OPEN];
        let texels: Vec<[f32; 4]> = (0..=SKY_LEVELS)
            .flat_map(|level| {
                let side = (SIZE >> level) as usize;
                std::iter::repeat_n(if level == 0 { empty } else { open_sky }, side * side)
            })
            .collect();
        let mut image = Image::new(
            Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            half_floats(&texels[..(SIZE * SIZE) as usize]),
            TextureFormat::Rgba16Float,
            RenderAssetUsages::RENDER_WORLD,
        );
        // Bevy checks the data against level 0; the mip levels follow it.
        image.texture_descriptor.mip_level_count = 1 + SKY_LEVELS;
        image.data = Some(half_floats(&texels));
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::ClampToEdge,
            address_mode_v: ImageAddressMode::ClampToEdge,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            mipmap_filter: ImageFilterMode::Linear,
            ..default()
        });
        image
    }

    /// The published map's sky levels as half floats, finest first. Bevy rewrites only level 0
    /// of a texture it already has, so these are uploaded separately (`clouds::render`).
    pub(crate) fn sky_level_bytes(&self) -> Vec<Vec<u8>> {
        self.map
            .as_ref()
            .map(|map| map.levels.iter().map(|level| half_floats(level)).collect())
            .unwrap_or_default()
    }

    pub(crate) fn write(&self, image: &mut Image) {
        if let Some(map) = &self.map {
            let texels: Vec<[f32; 4]> = map
                .texels
                .iter()
                .chain(map.levels.iter().flatten())
                .copied()
                .collect();
            image.data = Some(half_floats(&texels));
        }
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

/// [`SKY_LEVELS`] mip levels of the map: per 2×2 texels the highest tops, lowest base and the
/// average share of sky the crowns let through.
fn sky_levels(texels: &[[f32; 4]]) -> Vec<Vec<[f32; 4]>> {
    let mut levels: Vec<Vec<[f32; 4]>> = Vec::new();
    for level in 1..=SKY_LEVELS {
        let (side, finer) = ((SIZE >> level) as usize, (SIZE >> (level - 1)) as usize);
        let source = levels.last().map_or(texels, Vec::as_slice);
        let level_texels = (0..side * side)
            .map(|i| {
                let (x, y) = (i % side * 2, i / side * 2);
                let quad = [
                    source[y * finer + x],
                    source[y * finer + x + 1],
                    source[(y + 1) * finer + x],
                    source[(y + 1) * finer + x + 1],
                ];
                // Level 0 holds foliage density; finer sky levels already hold the sky share.
                let sky = |t: &[f32; 4]| if level == 1 { (-t[2]).exp() } else { t[2] };
                [
                    quad.iter().fold(OPEN, |v, t| v.max(t[0])),
                    quad.iter().fold(-OPEN, |v, t| v.min(t[1])),
                    quad.iter().map(sky).sum::<f32>() * 0.25,
                    quad.iter().fold(OPEN, |v, t| v.max(t[3])),
                ]
            })
            .collect();
        levels.push(level_texels);
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crown(x: f32, z: f32) -> ForestCrown {
        ForestCrown {
            centre: Vec2::new(x, z),
            radius: 3.0,
            bottom: 8.0,
            top: 18.0,
            opacity: 0.6,
        }
    }

    #[test]
    fn crowns_fill_their_layer_and_leave_the_trunk_and_sky_open() {
        let map = ForestShadowMap::rasterize(Vec2::ZERO, [crown(10.0, -6.0)]);
        assert!(map.density_at(Vec3::new(10.0, 13.0, -6.0)) > 0.3);
        assert_eq!(
            map.density_at(Vec3::new(10.0, 4.0, -6.0)),
            0.0,
            "below the crown"
        );
        assert_eq!(
            map.density_at(Vec3::new(10.0, 19.0, -6.0)),
            0.0,
            "above the crown"
        );
        assert_eq!(
            map.density_at(Vec3::new(30.0, 13.0, -6.0)),
            0.0,
            "beside it"
        );
        assert_eq!(
            map.density_at(Vec3::new(5000.0, 13.0, 0.0)),
            0.0,
            "outside the map"
        );
        let local_top = |x: f32| {
            let texel = ((Vec2::new(x, -6.0) - map.origin) / METRES_PER_TEXEL).floor();
            map.texels[(texel.y as u32 * SIZE + texel.x as u32) as usize][3]
        };
        assert_eq!(
            local_top(60.0),
            18.0,
            "a crown 50 m away is in the neighbourhood"
        );
        assert_eq!(local_top(200.0), OPEN, "one 190 m away is not");
    }

    #[test]
    fn sky_levels_hold_the_sky_share_around_the_crowns() {
        let map = ForestShadowMap::rasterize(Vec2::ZERO, [crown(10.0, -6.0)]);
        assert_eq!(map.levels.len(), SKY_LEVELS as usize);
        let at = |level: usize, x: f32| {
            let size = (SIZE >> (level + 1)) as f32;
            let texel = ((Vec2::new(x, -6.0) - map.origin)
                / (METRES_PER_TEXEL * 2.0 * (1 << level) as f32))
                .floor()
                .min(Vec2::splat(size - 1.0));
            map.levels[level][(texel.y * size + texel.x) as usize]
        };
        let [top, _, sky, canopy] = at(0, 10.0);
        assert!(
            sky < 0.95 && sky > 0.0,
            "the crown hides part of the sky: {sky}"
        );
        assert_eq!((top, canopy), (18.0, 18.0));
        assert_eq!(at(0, 600.0)[2], 1.0, "open sky far from it");
        assert!(
            at(2, 10.0)[2] > sky,
            "coarser levels average in the open sky around it"
        );
        let data = {
            let mut shadow = ForestShadow::default();
            shadow.publish(map);
            let mut image = ForestShadow::image();
            shadow.write(&mut image);
            image.data.unwrap().len()
        };
        let texels: usize = (0..=SKY_LEVELS)
            .map(|l| ((SIZE >> l) * (SIZE >> l)) as usize)
            .sum();
        assert_eq!(data, texels * 8, "every mip level is uploaded");
    }

    #[test]
    fn overlapping_crowns_combine_and_publication_enables_the_shader() {
        let one = ForestShadowMap::rasterize(Vec2::ZERO, [crown(10.0, -6.0)]);
        let two = ForestShadowMap::rasterize(Vec2::ZERO, [crown(10.0, -6.0), crown(10.5, -6.0)]);
        let p = Vec3::new(10.0, 13.0, -6.0);
        assert!(two.density_at(p) > one.density_at(p));
        let mut shadow = ForestShadow::default();
        assert_eq!(shadow.parameters()[2], 0.0, "off until published");
        shadow.publish(two);
        let [x, z, texel, tallest] = shadow.parameters();
        assert_eq!((x % METRES_PER_TEXEL, z % METRES_PER_TEXEL), (0.0, 0.0));
        assert_eq!(texel, METRES_PER_TEXEL);
        assert_eq!(tallest, 18.0);
        assert_eq!(shadow.centre().unwrap().round(), Vec2::ZERO);
    }

    #[test]
    fn invalid_crowns_are_skipped() {
        let map = ForestShadowMap::rasterize(
            Vec2::ZERO,
            [
                ForestCrown {
                    radius: f32::NAN,
                    ..crown(0.0, 0.0)
                },
                ForestCrown {
                    top: 2.0,
                    ..crown(0.0, 0.0)
                },
            ],
        );
        assert_eq!(map.tallest, OPEN);
    }
}
