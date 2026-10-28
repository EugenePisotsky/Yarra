//! The character probe (`--character-probe ASSET`): a model under assets/ stands ahead of the
//! player at the start, facing the camera, so a character can be judged in the game's own light
//! next to the one it plays. It stands in its rest pose; `content/characters/scripts/export_glb.py`
//! makes one from an MPFB preset.
use crate::launch::LaunchOptions;

use bevy::{gltf::GltfAssetLabel, prelude::*};
use engine::{TerrainGrounded, WorldRenderRoot, WorldStartAdopted, WorldStartView};
use world::WorldViewBookmark;

/// How far ahead of the start's focus the model stands and how far to the camera's right,
/// metres: off the line of sight, so the player does not hide it.
const AHEAD_M: f32 = 2.4;

const ASIDE_M: f32 = 0.9;

#[derive(Resource)]
struct ProbeAsset(String);

#[derive(Component)]
struct CharacterProbe;

pub(crate) fn install(app: &mut App) {
    let Some(asset) = app
        .world()
        .resource::<LaunchOptions>()
        .character_probe
        .clone()
    else {
        return;
    };
    app.insert_resource(ProbeAsset(asset))
        .add_systems(Update, place_probe);
}

/// Stands the model at the start view, or moves it to the one the world adopts.
fn place_probe(
    mut commands: Commands,
    asset: Res<ProbeAsset>,
    asset_server: Res<AssetServer>,
    start: Res<WorldStartView>,
    mut adopted: MessageReader<WorldStartAdopted>,
    mut probe: Query<&mut Transform, With<CharacterProbe>>,
) {
    let view = match adopted.read().last() {
        Some(WorldStartAdopted(view)) => view,
        None if probe.is_empty() => match start.0.as_ref() {
            Some(view) => view,
            None => return,
        },
        None => return,
    };
    let stand = stand(view);

    if let Ok(mut transform) = probe.single_mut() {
        *transform = stand;
        return;
    }

    let scene = GltfAssetLabel::Scene(0).from_asset(asset.0.clone());
    commands.spawn((
        stand,
        Visibility::Inherited,
        WorldAssetRoot(asset_server.load(scene)),
        TerrainGrounded,
        WorldRenderRoot,
        CharacterProbe,
        Name::new("Character probe"),
    ));
}

/// Ahead of the focus along the camera's view and to its right, the model's front (glTF's +Z)
/// turned towards the camera.
fn stand(view: &WorldViewBookmark) -> Transform {
    let yaw = view.yaw_degrees.to_radians();
    let towards_camera = Vec3::new(yaw.sin(), 0.0, yaw.cos());
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let focus = Vec3::from_array(view.position);
    Transform::from_translation(focus - towards_camera * AHEAD_M + right * ASIDE_M)
        .with_rotation(Quat::from_rotation_y(yaw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(yaw_degrees: f32) -> WorldViewBookmark {
        WorldViewBookmark {
            position: [10.0, 5.0, -3.0],
            yaw_degrees,
            pitch_degrees: 10.0,
            distance: 4.0,
            fog_visibility: 1000.0,
            route: Vec::new(),
        }
    }

    #[test]
    fn the_model_stands_ahead_of_the_focus_and_faces_the_camera() {
        for yaw in [0.0, 45.0, 135.0, -90.0] {
            let view = view(yaw);
            let stand = stand(&view);
            let camera = engine::orbit_transform(
                Vec3::from_array(view.position),
                yaw.to_radians(),
                0.2,
                4.0,
            );
            let focus = Vec3::from_array(view.position);
            let ahead = (stand.translation - focus)
                .dot((focus - camera.translation).with_y(0.0).normalize());
            let facing = (stand.rotation * Vec3::Z).dot(
                (camera.translation - stand.translation)
                    .with_y(0.0)
                    .normalize(),
            );

            assert!((ahead - AHEAD_M).abs() < 1e-3, "yaw {yaw}: {ahead} m ahead");
            assert!(facing > 0.9, "yaw {yaw}: faces the camera by {facing}");
        }
    }
}
