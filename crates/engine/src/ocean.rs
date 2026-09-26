//! Render-only open sea at the current world space's sea level. One flat surface follows the
//! active world view out to its culling distance, and the terrain beneath it forms the shore.
//! It is a placeholder: waves, shore foam, transparency, lakes and rivers are later work.
use crate::{ActiveWorldSpace, WORLD_VIEW_DISTANCE, WorldCatalog, WorldViewCamera};
use bevy::{light::NotShadowCaster, prelude::*, transform::TransformSystems};

pub struct OceanPlugin;

#[derive(Component)]
pub struct OceanSurface;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_ocean).add_systems(
            PostUpdate,
            follow_world_view.before(TransformSystems::Propagate),
        );
    }
}

fn spawn_ocean(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let size = 2. * WORLD_VIEW_DISTANCE;
    commands.spawn((
        OceanSurface,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(size, size))),
        // Deep, glossy water. The shared PBR conversion adds cloud shadows and rain.
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.015, 0.055, 0.075),
            perceptual_roughness: 0.08,
            reflectance: 0.5,
            ..default()
        })),
        NotShadowCaster,
        Transform::default(),
        Visibility::Hidden,
        Name::new("Ocean surface"),
    ));
}

/// Render-space Y is absolute, so only X/Z follow the view; floating-origin rebases need
/// nothing else. Hidden without a sea level or an active world view (editor studies).
fn follow_world_view(
    catalog: Res<WorldCatalog>,
    active: Res<ActiveWorldSpace>,
    views: Query<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mut ocean: Single<(&mut Transform, &mut Visibility), With<OceanSurface>>,
) {
    let level = active
        .current()
        .and_then(|space| catalog.world_space(space))
        .and_then(|space| space.sea_level);
    let view = views
        .iter()
        .find(|(camera, _)| camera.is_active)
        .map(|(_, transform)| transform.translation());
    let (transform, visibility) = &mut *ocean;
    match (level, view) {
        (Some(level), Some(view)) => {
            transform.translation = Vec3::new(view.x, level, view.z);
            **visibility = Visibility::Inherited;
        }
        _ => **visibility = Visibility::Hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::{CellCoord, WorldSpaceId};

    fn ocean_app(sea_level: Option<f32>) -> App {
        let space = WorldSpaceId(1);
        let (mut catalog, _) =
            crate::world_streaming::test_world_resources(space, CellCoord::ZERO, None);
        catalog.world_spaces_mut()[0].sea_level = sea_level;
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .insert_resource(ActiveWorldSpace::current_for_tests(space))
            .insert_resource(catalog)
            .add_plugins(OceanPlugin);
        app.world_mut().spawn((
            Camera::default(),
            GlobalTransform::from_xyz(120., 35., -40.),
            WorldViewCamera,
        ));
        app
    }

    fn surface(app: &mut App) -> (Vec3, Visibility) {
        let world = app.world_mut();
        let (transform, visibility) = world
            .query_filtered::<(&Transform, &Visibility), With<OceanSurface>>()
            .single(world)
            .unwrap();
        (transform.translation, *visibility)
    }

    #[test]
    fn sea_follows_the_world_view_at_sea_level() {
        let mut app = ocean_app(Some(0.));
        app.update();
        assert_eq!(
            surface(&mut app),
            (Vec3::new(120., 0., -40.), Visibility::Inherited)
        );
    }

    #[test]
    fn worlds_without_sea_or_an_active_view_hide_it() {
        let mut app = ocean_app(None);
        app.update();
        assert_eq!(surface(&mut app).1, Visibility::Hidden);
        let mut app = ocean_app(Some(0.));
        let world = app.world_mut();
        let mut cameras = world.query::<&mut Camera>();
        for mut camera in cameras.iter_mut(world) {
            camera.is_active = false;
        }
        app.update();
        assert_eq!(surface(&mut app).1, Visibility::Hidden);
    }
}
