//! Terrain materials bind the environment's inputs at slots 120–125, as every lit material does
//! (`shaders/lighting/surface.wesl`): they start on the fallback buffer and take the
//! environment's own once it exists.
use atmosphere::environment::EnvironmentAssets;
use bevy::{prelude::*, render::storage::ShaderBuffer};

pub(crate) trait EnvironmentInputs: Asset {
    fn parameters(&self) -> &Handle<ShaderBuffer>;
    fn bind_environment(&mut self, environment: &EnvironmentAssets);
}
impl EnvironmentInputs for crate::TerrainMaterial {
    fn parameters(&self) -> &Handle<ShaderBuffer> {
        &self.environment
    }
    fn bind_environment(&mut self, environment: &EnvironmentAssets) {
        self.environment = environment.parameters.clone();
        self.cloud_shadows = Some(environment.shadows.clone());
        self.rain_shelter = Some(environment.shelter.clone());
        self.forest_shadow = Some(environment.forest_shadow.clone());
    }
}
impl EnvironmentInputs for crate::TerrainCompositeMaterial {
    fn parameters(&self) -> &Handle<ShaderBuffer> {
        &self.environment
    }
    fn bind_environment(&mut self, environment: &EnvironmentAssets) {
        self.environment = environment.parameters.clone();
        self.cloud_shadows = Some(environment.shadows.clone());
        self.rain_shelter = Some(environment.shelter.clone());
        self.forest_shadow = Some(environment.forest_shadow.clone());
    }
}

/// Binds the environment into materials still on another buffer; only those are marked changed.
pub(crate) fn sync<M: EnvironmentInputs>(
    environment: Option<Res<EnvironmentAssets>>,
    mut materials: ResMut<Assets<M>>,
) {
    let Some(environment) = environment else {
        return;
    };
    let ids: Vec<_> = materials
        .iter()
        .filter(|(_, m)| *m.parameters() != environment.parameters)
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        materials
            .get_mut(id)
            .unwrap()
            .bind_environment(&environment);
    }
}
