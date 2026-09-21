//! Authoritative player root, motor and terrain contact.
use crate::{
    DEFAULT_CHARACTER_PRESENTATION_ID, StreamedTerrainSurface, TerrainContactReadiness,
    TerrainLodPreview, WorldOrigin, WorldRenderRoot, WorldStartView,
    actor::{
        CameraTarget, CharacterMotion, CharacterMotor, MoveIntent, PlayerControlled,
        TerrainGrounded, WorldStreamFocus,
    },
    character::CharacterPresentationRef,
    sample_resident_terrain_surface, world_streaming,
};
use bevy::prelude::*;

pub(super) fn spawn_player(mut commands: Commands, start_view: Res<WorldStartView>) {
    let start = start_view
        .0
        .as_ref()
        .map_or(Vec3::ZERO, |v| Vec3::from_array(v.position));
    commands.spawn((
        Transform::from_translation(start),
        Visibility::Inherited,
        MoveIntent::default(),
        CharacterMotor::default(),
        CharacterMotion::default(),
        CharacterPresentationRef::new(DEFAULT_CHARACTER_PRESENTATION_ID),
        PlayerControlled,
        TerrainGrounded,
        CameraTarget,
        WorldStreamFocus,
        WorldRenderRoot,
        Name::new("Player actor root"),
    ));
}

pub(crate) fn ground_characters_to_streamed_terrain(
    origin: Res<WorldOrigin>,
    terrain_pages: Query<&StreamedTerrainSurface>,
    mut actors: Query<&mut Transform, With<TerrainGrounded>>,
    lod: Res<world_streaming::terrain_lod::TerrainLodStream>,
    lod_config: Res<TerrainLodPreview>,
    readiness: Res<TerrainContactReadiness>,
) {
    for mut transform in &mut actors {
        if lod_config.enabled {
            if let Some(height) =
                lod.sample_contact_height(transform.translation, &origin, &readiness)
            {
                transform.translation.y = height;
            }
            continue;
        }
        if let Some(surface) = sample_resident_terrain_surface(
            &origin,
            terrain_pages.iter(),
            [transform.translation.x, transform.translation.z],
        ) {
            transform.translation.y = surface.height;
        }
    }
}
