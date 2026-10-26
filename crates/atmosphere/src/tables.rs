//! Bevy's atmosphere pass (Bevy 0.20 `atmosphere_luts`) rebuilds its four tables every frame. The
//! transmittance and multiple-scattering tables depend on the atmosphere and its medium alone, not
//! on the view or the lights, and are most of the pass's work, so they are rebuilt only when those
//! change or their textures are new. On other frames their sizes in the render world's
//! `GpuAtmosphereSettings` read zero, set after every system that sizes textures or uploads the
//! uniforms from them: Bevy's pass dispatches no work for them, and the shaders keep the uniform's
//! sizes. Like the sky pass swap in `sky`, the adapter targets Bevy 0.20.
use bevy::{
    ecs::entity::EntityHashMap,
    pbr::{
        ExtractedAtmosphere, GpuAtmosphereSettings, GpuScatteringMedium,
        resources::AtmosphereTextures,
    },
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        render_asset::RenderAssets,
        render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor, TextureId},
    },
};

pub(crate) struct StaticTablesPlugin;

impl Plugin for StaticTablesPlugin {
    fn build(&self, app: &mut App) {
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.add_systems(Render, keep.in_set(RenderSystems::PrepareBindGroups));
        }
    }
}

/// Bevy's table pipelines, by label: its pass builds nothing until all four have compiled.
const PIPELINES: [&str; 4] = [
    "transmittance_lut_pipeline",
    "multi_scattering_lut_pipeline",
    "sky_view_lut_pipeline",
    "aerial_view_lut_pipeline",
];

/// What a view's two tables are built from.
#[derive(Clone, Copy, PartialEq)]
struct Inputs {
    /// The tables' own textures, then the medium's density and scattering tables.
    textures: [TextureId; 4],
    /// The planet's radii and ground albedo.
    planet: [f32; 5],
    /// The tables' sizes and samples.
    settings: [u32; 7],
}

fn keep(
    mut views: Query<(
        Entity,
        &mut GpuAtmosphereSettings,
        &ExtractedAtmosphere,
        &AtmosphereTextures,
    )>,
    media: Res<RenderAssets<GpuScatteringMedium>>,
    cache: Res<PipelineCache>,
    mut compiled: Local<bool>,
    mut built: Local<Built<Inputs>>,
) {
    *compiled = *compiled
        || PIPELINES
            .iter()
            .all(|label| compiled_pipeline(&cache, label));

    if !*compiled {
        return;
    }

    built.retain(|entity| views.contains(entity));

    for (entity, mut settings, atmosphere, textures) in &mut views {
        let Some(medium) = media.get(atmosphere.medium) else {
            continue;
        };
        let inputs = Inputs {
            textures: [
                textures.transmittance_lut.texture.id(),
                textures.multiscattering_lut.texture.id(),
                medium.density_lut.id(),
                medium.scattering_lut.id(),
            ],
            planet: [
                atmosphere.inner_radius,
                atmosphere.outer_radius,
                atmosphere.ground_albedo.x,
                atmosphere.ground_albedo.y,
                atmosphere.ground_albedo.z,
            ],
            settings: [
                settings.transmittance_lut_size.x,
                settings.transmittance_lut_size.y,
                settings.transmittance_lut_samples,
                settings.multiscattering_lut_size.x,
                settings.multiscattering_lut_size.y,
                settings.multiscattering_lut_dirs,
                settings.multiscattering_lut_samples,
            ],
        };

        if built.unchanged(entity, inputs) {
            settings.transmittance_lut_size = UVec2::ZERO;
            settings.multiscattering_lut_size = UVec2::ZERO;
        }
    }
}

fn compiled_pipeline(cache: &PipelineCache, label: &str) -> bool {
    cache.pipelines().any(|pipeline| {
        matches!(pipeline.state, CachedPipelineState::Ok(_))
            && matches!(&pipeline.descriptor, PipelineDescriptor::ComputePipelineDescriptor(d)
                if d.label.as_deref() == Some(label))
    })
}

/// The inputs each view's tables were last built from.
struct Built<K>(EntityHashMap<K>);

impl<K> Default for Built<K> {
    fn default() -> Self {
        Self(EntityHashMap::default())
    }
}

impl<K: PartialEq> Built<K> {
    /// Whether `entity`'s tables were built from `inputs`; if not, they are this frame.
    fn unchanged(&mut self, entity: Entity, inputs: K) -> bool {
        if self.0.get(&entity) == Some(&inputs) {
            return true;
        }

        self.0.insert(entity, inputs);
        false
    }

    fn retain(&mut self, live: impl Fn(Entity) -> bool) {
        self.0.retain(|entity, _| live(*entity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_rebuild_on_first_sight_and_on_change_only() {
        let mut built = Built::default();
        let (a, b) = (
            Entity::from_raw_u32(1).unwrap(),
            Entity::from_raw_u32(2).unwrap(),
        );
        assert!(!built.unchanged(a, 1));
        assert!(built.unchanged(a, 1));
        assert!(!built.unchanged(b, 1), "each view's textures are its own");
        assert!(!built.unchanged(a, 2));
        assert!(built.unchanged(a, 2));

        // A view that comes back is built again.
        built.retain(|entity| entity == b);
        assert!(!built.unchanged(a, 2));
    }
}
