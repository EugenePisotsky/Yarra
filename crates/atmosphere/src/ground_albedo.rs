//! The ground around the camera, seen from above: its albedo, and how squarely the sun's beam
//! meets it. What lies below a surface reflects the light it gets in its own colour
//! (`shaders/lighting/ground.wesl`), so sunlit snow lights a character or a trunk from below, a
//! meadow barely does, and snow on a slope turned from the sun only by the sky's light. A terrain
//! renderer fills the map each frame from the ground it draws (`terrain_render`); ground no layer
//! holds, or no renderer fills, is taken for open, sunlit grass and soil. Two layers, the near one
//! finer, each snapped to its texels in world space so a texel holds the same ground while the
//! camera moves.
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
        TextureViewDimension,
    },
};

/// Texels per side of each layer.
pub const SIZE: u32 = 128;

/// Metres per texel of the near layer (256 m across) and the far one (4 km): the ground a point
/// a metre or two up sees lies mostly within a few metres of it.
pub const TEXELS: [f32; 2] = [2.0, 32.0];

pub const FORMAT: TextureFormat = TextureFormat::Rgba16Float;

#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct GroundAlbedo {
    /// Render-local XZ of each layer's first corner.
    pub corners: [Vec2; 2],
    /// The open sea's level, under which the ground is water.
    pub sea: Option<f32>,
    /// Whether the map holds the ground, as its renderer last filled it.
    pub filled: bool,
}

impl GroundAlbedo {
    /// Centres both layers on render XZ `centre` of a world whose render origin lies at `origin`.
    pub fn follow(&mut self, centre: Vec2, origin: [f64; 2]) {
        for (corner, texel) in self.corners.iter_mut().zip(TEXELS.map(f64::from)) {
            let half = f64::from(SIZE) * texel * 0.5;
            let first = |c: f32, o: f64| ((f64::from(c) + o - half) / texel).floor() * texel - o;
            *corner = Vec2::new(
                first(centre.x, origin[0]) as f32,
                first(centre.y, origin[1]) as f32,
            );
        }
    }

    /// Each layer's first corner and metres per texel; none while nothing fills the map.
    pub(crate) fn parameters(&self) -> [[f32; 4]; 2] {
        if !self.filled {
            return [[0.0; 4]; 2];
        }

        [0, 1].map(|l| [self.corners[l].x, self.corners[l].y, TEXELS[l], 0.0])
    }

    /// The map, written on the GPU alone.
    pub fn image() -> Image {
        let mut image = Image::new_uninit(
            Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 2,
            },
            TextureDimension::D2,
            FORMAT,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_descriptor.usage |= TextureUsages::STORAGE_BINDING;
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..default()
        });
        image.sampler = bevy::image::ImageSampler::linear();
        image
    }
}

/// The map follows the camera that sees the world.
pub(crate) fn follow_view(
    mut albedo: ResMut<GroundAlbedo>,
    origin: Res<crate::environment::EnvironmentOrigin>,
    sea: Res<crate::SeaSurface>,
    views: Query<&GlobalTransform, With<crate::WorldEnvironmentView>>,
) {
    if let Some(view) = views.iter().next() {
        albedo.follow(view.translation().xz(), origin.0);
    }

    albedo.sea = sea.level;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_centre_on_the_camera_on_texels_fixed_in_the_world() {
        let mut albedo = GroundAlbedo::default();
        albedo.follow(Vec2::new(10.5, -3.2), [64.0, 32.0]);

        // World (74.5, 28.8): the near layer's corner 128 m back, on even metres of the world.
        assert_eq!(albedo.corners[0], Vec2::new(-118.0, -132.0));

        // The far layer's 2048 m back, on whole 32 m texels.
        assert_eq!(albedo.corners[1], Vec2::new(-2048.0, -2080.0));

        // Moving within a texel keeps it; the same world point from another origin lands on the
        // same world texels.
        let before = albedo.corners;
        albedo.follow(Vec2::new(11.4, -3.9), [64.0, 32.0]);
        assert_eq!(albedo.corners, before);
        albedo.follow(Vec2::new(-21.5, 28.8), [96.0, 0.0]);
        assert_eq!(
            albedo.corners[0] + Vec2::new(96.0, 0.0),
            before[0] + Vec2::new(64.0, 32.0)
        );
    }

    #[test]
    fn an_unfilled_map_is_not_published() {
        let mut albedo = GroundAlbedo::default();
        assert_eq!(albedo.parameters(), [[0.0; 4]; 2]);
        albedo.filled = true;
        albedo.follow(Vec2::ZERO, [0.0; 2]);
        assert_eq!(albedo.parameters()[0], [-128.0, -128.0, 2.0, 0.0]);
        assert_eq!(albedo.parameters()[1], [-2048.0, -2048.0, 32.0, 0.0]);
    }
}
