//! Optional visual feedback for a movement destination.
use super::GameplaySystems;
use crate::{
    StreamedTerrainSurface, WorldOrigin, WorldRenderRoot,
    actor::{MoveIntent, PlayerControlled},
    sample_resident_terrain_surface,
};
use bevy::prelude::*;

/// Shows a terrain-aligned destination ring. Requires gameplay, Mesh and StandardMaterial assets.
/// Omission removes both its startup entity/assets and its update system.
pub struct MovementTargetPlugin;
impl Plugin for MovementTargetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_indicator).add_systems(
            Update,
            update_target_indicator.in_set(GameplaySystems::TargetIndicator),
        );
    }
}

const TARGET_INDICATOR_HEIGHT: f32 = 0.025;
#[derive(Component)]
pub(super) struct TargetIndicator;

type TargetIndicatorState<'w, 's> = Single<
    'w,
    's,
    (&'static mut Transform, &'static mut Visibility),
    (With<TargetIndicator>, Without<PlayerControlled>),
>;

fn setup_indicator(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(
            meshes.add(
                Torus::new(0.55, 0.68)
                    .mesh()
                    .minor_resolution(8)
                    .major_resolution(48),
            ),
        ),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.72, 0.12),
            unlit: true,
            ..default()
        })),
        Transform::from_xyz(0.0, TARGET_INDICATOR_HEIGHT, 0.0),
        Visibility::Hidden,
        TargetIndicator,
        WorldRenderRoot,
        Name::new("Movement target indicator"),
    ));
}
fn update_target_indicator(
    intent: Single<&MoveIntent, With<PlayerControlled>>,
    origin: Res<WorldOrigin>,
    terrain_pages: Query<&StreamedTerrainSurface>,
    mut indicator: TargetIndicatorState,
) {
    if let Some(target) = intent.destination() {
        let ground_height =
            sample_resident_terrain_surface(&origin, terrain_pages.iter(), [target.x, target.z])
                .map_or(target.y, |surface| surface.height);
        indicator.0.translation =
            Vec3::new(target.x, ground_height + TARGET_INDICATOR_HEIGHT, target.z);
        *indicator.1 = Visibility::Visible;
    } else {
        *indicator.1 = Visibility::Hidden;
    }
}
