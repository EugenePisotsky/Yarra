//! Render-only open sea at the current world space's sea level. There is no sea mesh: the sky
//! composite intersects each view ray with the sea's surface (`atmosphere::SeaSurface`) and
//! draws the water in front of whatever the main pass drew below it, so the seabed shows through
//! shallow water and the shore is wherever the terrain crosses the sea level. Lakes, rivers and
//! swimming are later work.
use crate::{ActiveWorldSpace, ActiveWorldView, WorldCatalog};
use bevy::prelude::*;

pub struct OceanPlugin;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<atmosphere::SeaSurface>()
            .add_systems(PostUpdate, publish_sea.before(atmosphere::ApplyAtmosphere));
    }
}

/// The sea of the active world space while a world view shows it; none without a sea level or
/// an active world view (other editor workspaces).
fn publish_sea(
    catalog: Res<WorldCatalog>,
    active: Res<ActiveWorldSpace>,
    view: ActiveWorldView,
    mut sea: ResMut<atmosphere::SeaSurface>,
) {
    let level = active
        .current()
        .and_then(|space| catalog.world_space(space))
        .and_then(|space| space.sea_level)
        .filter(|_| view.active().is_some());
    if sea.level != level {
        sea.level = level;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorldViewCamera;
    use world::{CellCoord, WorldSpaceId};

    fn ocean_app(sea_level: Option<f32>) -> App {
        let space = WorldSpaceId(1);
        let (mut catalog, _) =
            crate::world_streaming::test_world_resources(space, CellCoord::ZERO, None);
        catalog.world_spaces_mut()[0].sea_level = sea_level;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(ActiveWorldSpace::current_for_tests(space))
            .insert_resource(catalog)
            .configure_sets(PostUpdate, atmosphere::ApplyAtmosphere)
            .add_plugins(OceanPlugin);
        app.world_mut().spawn((
            Camera::default(),
            GlobalTransform::from_xyz(120., 35., -40.),
            WorldViewCamera,
        ));
        app
    }

    fn level(app: &App) -> Option<f32> {
        app.world().resource::<atmosphere::SeaSurface>().level
    }

    #[test]
    fn the_active_world_views_sea_is_published_for_the_sky_composite() {
        let mut app = ocean_app(Some(0.));
        app.update();
        assert_eq!(level(&app), Some(0.));
    }

    #[test]
    fn worlds_without_sea_or_an_active_view_have_none() {
        let mut app = ocean_app(None);
        app.update();
        assert_eq!(level(&app), None);
        let mut app = ocean_app(Some(0.));
        let world = app.world_mut();
        let mut cameras = world.query::<&mut Camera>();
        for mut camera in cameras.iter_mut(world) {
            camera.is_active = false;
        }
        app.update();
        assert_eq!(level(&app), None);
    }
}
