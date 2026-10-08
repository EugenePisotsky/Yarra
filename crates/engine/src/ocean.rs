//! Render-only open sea at the current world space's sea level. One surface follows the active
//! world view, and the terrain beneath it forms the shore. It is a placeholder: waves, shore
//! foam, transparency, lakes and rivers are later work.
use crate::{ActiveWorldSpace, WorldCatalog, WorldViewCamera};
use bevy::{
    asset::RenderAssetUsages, light::NotShadowCaster, mesh::Indices, prelude::*,
    render::render_resource::PrimitiveTopology, transform::TransformSystems,
};

/// The sky's planet (Bevy's Earth atmosphere).
const PLANET_RADIUS: f32 = 6_360_000.;
/// Flat around the view, so shores meet the uncurved terrain exactly.
const FLAT_RADIUS: f32 = 10_000.;
/// Beyond the horizon from the highest island summits (about 130 km at 1,300 m).
const OUTER_RADIUS: f32 = 200_000.;

pub struct OceanPlugin;

#[derive(Component)]
pub struct OceanSurface;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<atmosphere::SeaSurface>()
            .add_systems(Startup, spawn_ocean)
            .add_systems(
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
    commands.spawn((
        OceanSurface,
        Mesh3d(meshes.add(sea_disc())),
        // The light scattered up from inside deep water; its waves, reflection and glitter are
        // shaded by the sky composite (`atmosphere::SeaSurface`), so the surface itself has no
        // specular. The shared PBR conversion adds cloud shadows.
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.03, 0.09, 0.10),
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        })),
        NotShadowCaster,
        Transform::default(),
        Visibility::Hidden,
        Name::new("Ocean surface"),
    ));
}

/// Sinks the sea below its level by the planet's curvature beyond `FLAT_RADIUS`.
fn drop(r: f32) -> f32 {
    let beyond = (r - FLAT_RADIUS).max(0.);
    beyond * beyond / (2. * PLANET_RADIUS)
}

/// A disc out to `OUTER_RADIUS`, flat near the view and then bending with the planet, so the
/// sea reaches the sky's horizon from any height. A flat 20 km plane ended below that horizon
/// seen from hills and summits, leaving a grey band of the sky's planet ground above it.
fn sea_disc() -> Mesh {
    const SEGMENTS: u32 = 96;
    const RINGS: u32 = 96;
    // Rings grow geometrically from 20 m, so nearby vertices stay dense.
    let growth = (OUTER_RADIUS / 20.).powf(1. / (RINGS - 1) as f32);
    let radii: Vec<f32> = std::iter::once(0.)
        .chain((0..RINGS).map(|i| 20. * growth.powi(i as i32)))
        .collect();
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    for &r in &radii {
        let slope = (r - FLAT_RADIUS).max(0.) / PLANET_RADIUS;
        let count = if r == 0. { 1 } else { SEGMENTS };
        for s in 0..count {
            let angle = s as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            positions.push([r * cos, -drop(r), r * sin]);
            normals.push(
                Vec3::new(cos * slope, 1., sin * slope)
                    .normalize()
                    .to_array(),
            );
        }
    }
    let ring = |i: u32, s: u32| 1 + (i - 1) * SEGMENTS + s % SEGMENTS;
    let mut indices = Vec::new();
    for s in 0..SEGMENTS {
        indices.extend([0, ring(1, s + 1), ring(1, s)]);
    }
    for i in 1..RINGS {
        for s in 0..SEGMENTS {
            let (a, b, c, d) = (
                ring(i, s),
                ring(i, s + 1),
                ring(i + 1, s),
                ring(i + 1, s + 1),
            );
            indices.extend([a, b, c, b, d, c]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_indices(Indices::U32(indices))
}

/// Render-space Y is absolute, so only X/Z follow the view; floating-origin rebases need
/// nothing else. Hidden without a sea level or an active world view (editor studies).
fn follow_world_view(
    catalog: Res<WorldCatalog>,
    active: Res<ActiveWorldSpace>,
    views: Query<(&Camera, &GlobalTransform), With<WorldViewCamera>>,
    mut ocean: Single<(&mut Transform, &mut Visibility), With<OceanSurface>>,
    mut sea: ResMut<atmosphere::SeaSurface>,
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
    let shown = match (level, view) {
        (Some(level), Some(view)) => {
            transform.translation = Vec3::new(view.x, level, view.z);
            **visibility = Visibility::Inherited;
            Some(level)
        }
        _ => {
            **visibility = Visibility::Hidden;
            None
        }
    };
    if sea.level != shown {
        sea.level = shown;
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
        // The sky composite shades the water at this level.
        assert_eq!(
            app.world().resource::<atmosphere::SeaSurface>().level,
            Some(0.)
        );
    }

    #[test]
    fn sea_reaches_the_planet_horizon_and_faces_up() {
        // The sky's horizon lies sqrt(2hR) away; the curved sea covers it from summit height,
        // and within the island it is flat at sea level.
        let horizon = (2. * 1300. * PLANET_RADIUS).sqrt();
        assert!(OUTER_RADIUS > horizon);
        assert_eq!(drop(FLAT_RADIUS), 0.);
        let mesh = sea_disc();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!()
        };
        let far = positions
            .iter()
            .map(|p| Vec2::new(p[0], p[2]).length())
            .fold(0., f32::max);
        assert!((far - OUTER_RADIUS).abs() < 1.);
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!()
        };
        // Counter-clockwise seen from above: every triangle faces up.
        for t in indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from_array(positions[i as usize]));
            assert!((b - a).cross(c - a).y > 0., "{t:?}");
        }
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
        assert_eq!(app.world().resource::<atmosphere::SeaSurface>().level, None);
    }
}
