//! The environment's maps (rain shelter, forest shadow, valley mist, shore) are rebuilt on the
//! CPU and uploaded into their images only when a rebuild changed them.
use super::EnvironmentAssets;
use crate::{
    forest_shadow::ForestShadow, shelter::RainShelter, shore::Shore, valley_mist::ValleyMist,
};
use bevy::{prelude::*, render::extract_resource::ExtractResource};

/// A map with a revision that changes on every rebuild, and the image it is drawn into.
pub(super) trait EnvironmentMap: Resource {
    fn revision(&self) -> u64;
    fn write(&self, image: &mut Image);
    fn image(assets: &EnvironmentAssets) -> &Handle<Image>;
}
impl EnvironmentMap for RainShelter {
    fn revision(&self) -> u64 {
        self.revision()
    }
    fn write(&self, image: &mut Image) {
        self.write(image)
    }
    fn image(assets: &EnvironmentAssets) -> &Handle<Image> {
        &assets.shelter
    }
}
impl EnvironmentMap for ForestShadow {
    fn revision(&self) -> u64 {
        self.revision()
    }
    fn write(&self, image: &mut Image) {
        self.write(image)
    }
    fn image(assets: &EnvironmentAssets) -> &Handle<Image> {
        &assets.forest_shadow
    }
}
impl EnvironmentMap for ValleyMist {
    fn revision(&self) -> u64 {
        self.revision()
    }
    fn write(&self, image: &mut Image) {
        self.write(image)
    }
    fn image(assets: &EnvironmentAssets) -> &Handle<Image> {
        &assets.mist
    }
}
impl EnvironmentMap for Shore {
    fn revision(&self) -> u64 {
        self.revision()
    }
    fn write(&self, image: &mut Image) {
        self.write(image)
    }
    fn image(assets: &EnvironmentAssets) -> &Handle<Image> {
        &assets.shore
    }
}

/// Writes the map into its image if a rebuild changed it since the last upload; true when it
/// did.
fn upload<M: EnvironmentMap>(
    map: &M,
    assets: Option<&EnvironmentAssets>,
    images: &mut Assets<Image>,
    published: &mut Option<u64>,
) -> bool {
    let Some(assets) = assets else {
        return false;
    };
    if *published == Some(map.revision()) {
        return false;
    }
    let Some(mut image) = images.get_mut(M::image(assets)) else {
        return false;
    };
    map.write(&mut image);
    *published = Some(map.revision());
    true
}

/// Upload a map only when a rebuild changed it.
pub(super) fn publish<M: EnvironmentMap>(
    map: Res<M>,
    assets: Option<Res<EnvironmentAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut published: Local<Option<u64>>,
) {
    upload(&*map, assets.as_deref(), &mut images, &mut published);
}

/// The forest map's sky levels (mip levels 1 and up) for the render world to upload, since
/// Bevy rewrites only level 0 of an existing texture.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct ForestSkyLevels {
    pub revision: u64,
    pub levels: std::sync::Arc<Vec<Vec<u8>>>,
}

/// Upload the forest shadow map only when a rebuild changed it, with its sky levels.
pub(super) fn publish_forest_shadow(
    forest: Res<ForestShadow>,
    assets: Option<Res<EnvironmentAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut levels: ResMut<ForestSkyLevels>,
    mut published: Local<Option<u64>>,
) {
    if upload(&*forest, assets.as_deref(), &mut images, &mut published) {
        *levels = ForestSkyLevels {
            revision: forest.revision(),
            levels: std::sync::Arc::new(forest.sky_level_bytes()),
        };
    }
}
