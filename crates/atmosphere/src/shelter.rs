//! Top-down rain shelter around the camera: for each texel, the highest object top above it and
//! how much of the texel that object covers. Rain, splashes, puddles and surface wetness all read
//! the same map, so what stays dry agrees everywhere. Callers supply object footprints; any
//! placed object shelters the ground below its top, so trees need no special tagging.
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

/// Texels per edge and metres per texel: 128 m around the camera at 0.5 m.
pub const SIZE: u32 = 256;
pub const METRES_PER_TEXEL: f32 = 0.5;
/// Height stored where nothing shelters a texel.
const OPEN: f32 = -1.0e6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShelterDisc {
    /// Render-local XZ.
    pub centre: Vec2,
    pub radius: f32,
    /// Absolute height of the object's top.
    pub top: f32,
}

/// CPU copy of the published map. `enabled` is false until the first rebuild and whenever no
/// weather needs shelter, so shaders then treat everything as exposed.
#[derive(Resource, Clone)]
pub struct RainShelter {
    /// Render-local XZ of texel (0, 0)'s corner.
    pub origin: Vec2,
    pub enabled: bool,
    /// Per texel: top height, coverage 0..1.
    texels: Vec<[f32; 2]>,
    revision: u64,
}
impl Default for RainShelter {
    fn default() -> Self {
        Self {
            origin: Vec2::ZERO,
            enabled: false,
            texels: vec![[OPEN, 0.0]; (SIZE * SIZE) as usize],
            revision: 0,
        }
    }
}
impl RainShelter {
    /// Centre of the covered square.
    pub fn centre(&self) -> Vec2 {
        self.origin + Vec2::splat(SIZE as f32 * METRES_PER_TEXEL * 0.5)
    }

    /// Changes whenever the published texels do.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Rasterize footprints into a square centred on `centre`, snapped to whole texels so a
    /// rebuild does not shift unchanged shelter by a fraction of a texel.
    pub fn rebuild(&mut self, centre: Vec2, discs: impl IntoIterator<Item = ShelterDisc>) {
        let half = SIZE as f32 * METRES_PER_TEXEL * 0.5;
        self.origin = ((centre - Vec2::splat(half)) / METRES_PER_TEXEL).floor() * METRES_PER_TEXEL;
        self.texels.fill([OPEN, 0.0]);
        for disc in discs {
            if !(disc.radius > 0.0 && disc.radius.is_finite() && disc.top.is_finite()) {
                continue;
            }
            let local = (disc.centre - self.origin) / METRES_PER_TEXEL;
            let reach = disc.radius / METRES_PER_TEXEL + 1.0;
            let min = (local - reach).floor().max(Vec2::ZERO);
            let max = (local + reach).ceil().min(Vec2::splat(SIZE as f32 - 1.0));
            if min.x > max.x || min.y > max.y {
                continue;
            }
            for y in min.y as u32..=max.y as u32 {
                for x in min.x as u32..=max.x as u32 {
                    let texel_centre = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let distance = (texel_centre - local).length() * METRES_PER_TEXEL;
                    // Crowns thin out towards the edge; leaves let some rain through.
                    let t = ((distance - disc.radius * 0.6) / (disc.radius * 0.4)).clamp(0.0, 1.0);
                    let coverage = 0.95 * (1.0 - t * t * (3.0 - 2.0 * t));
                    if coverage <= 0.01 {
                        continue;
                    }
                    let texel = &mut self.texels[(y * SIZE + x) as usize];
                    texel[0] = texel[0].max(disc.top);
                    texel[1] = texel[1].max(coverage);
                }
            }
        }
        self.enabled = true;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn disable(&mut self) {
        if self.enabled {
            self.enabled = false;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// 1 where rain reaches `point`, 0 under full cover. Matches the shader lookup.
    pub fn exposure(&self, point: Vec3) -> f32 {
        if !self.enabled {
            return 1.0;
        }
        let texel = (Vec2::new(point.x, point.z) - self.origin) / METRES_PER_TEXEL - 0.5;
        let base = texel.floor();
        let f = texel - base;
        let mut cover = 0.0;
        for (dx, dy, w) in [
            (0, 0, (1.0 - f.x) * (1.0 - f.y)),
            (1, 0, f.x * (1.0 - f.y)),
            (0, 1, (1.0 - f.x) * f.y),
            (1, 1, f.x * f.y),
        ] {
            let (x, y) = (base.x as i32 + dx, base.y as i32 + dy);
            if x < 0 || y < 0 || x >= SIZE as i32 || y >= SIZE as i32 {
                continue;
            }
            let [top, coverage] = self.texels[(y as u32 * SIZE + x as u32) as usize];
            if point.y < top {
                cover += coverage * w;
            }
        }
        1.0 - cover
    }

    /// Shader parameters: origin XZ, metres per texel, enabled.
    pub fn parameters(&self) -> [f32; 4] {
        [
            self.origin.x,
            self.origin.y,
            METRES_PER_TEXEL,
            if self.enabled { 1.0 } else { 0.0 },
        ]
    }

    pub(crate) fn image() -> Image {
        let mut image = Image::new_fill(
            Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            bytemuck::cast_slice(&[OPEN, 0.0]),
            TextureFormat::Rg32Float,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = bevy::image::ImageSampler::nearest();
        image
    }

    pub(crate) fn write(&self, image: &mut Image) {
        image.data = Some(bytemuck::cast_slice(&self.texels).to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_shelter_below_their_top_with_soft_edges() {
        let mut shelter = RainShelter::default();
        assert_eq!(shelter.exposure(Vec3::ZERO), 1.0, "disabled until built");
        shelter.rebuild(
            Vec2::ZERO,
            [ShelterDisc {
                centre: Vec2::new(10.0, -5.0),
                radius: 4.0,
                top: 12.0,
            }],
        );
        assert!(shelter.exposure(Vec3::new(10.0, 0.0, -5.0)) < 0.1);
        assert_eq!(
            shelter.exposure(Vec3::new(10.0, 13.0, -5.0)),
            1.0,
            "above the top"
        );
        assert_eq!(
            shelter.exposure(Vec3::new(20.0, 0.0, -5.0)),
            1.0,
            "outside the crown"
        );
        let edge = shelter.exposure(Vec3::new(13.2, 0.0, -5.0));
        assert!(edge > 0.1 && edge < 0.9, "soft drip line: {edge}");
        assert_eq!(
            shelter.exposure(Vec3::new(500.0, 0.0, 0.0)),
            1.0,
            "outside the map"
        );
        assert_eq!(shelter.centre().round(), Vec2::ZERO);
    }

    #[test]
    fn rebuilds_snap_to_texels_and_ignore_invalid_discs() {
        let mut shelter = RainShelter::default();
        shelter.rebuild(
            Vec2::new(3.3, -7.7),
            [
                ShelterDisc {
                    centre: Vec2::ZERO,
                    radius: f32::NAN,
                    top: 1.0,
                },
                ShelterDisc {
                    centre: Vec2::ZERO,
                    radius: 2.0,
                    top: f32::INFINITY,
                },
            ],
        );
        assert_eq!(shelter.origin % METRES_PER_TEXEL, Vec2::ZERO);
        assert_eq!(shelter.exposure(Vec3::ZERO), 1.0);
        let revision = shelter.revision();
        shelter.disable();
        assert!(!shelter.enabled);
        assert_ne!(shelter.revision(), revision);
        assert_eq!(shelter.parameters()[3], 0.0);
    }
}
