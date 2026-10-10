//! Authoritative player root, motor and terrain contact.
use crate::{
    DEFAULT_CHARACTER_PRESENTATION_ID, StreamedTerrainSurface, TerrainContactReadiness,
    TerrainHierarchy, WorldOrigin, WorldRenderRoot, WorldStartView,
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

/// A grounded, presented actor that stands where it is placed until something gives it a
/// movement intent. For NPCs placed by a gameplay adapter.
pub fn standing_character(translation: Vec3, name: &'static str) -> impl Bundle {
    (
        Transform::from_translation(translation),
        Visibility::Inherited,
        MoveIntent::default(),
        CharacterMotor::default(),
        CharacterMotion::default(),
        CharacterPresentationRef::new(DEFAULT_CHARACTER_PRESENTATION_ID),
        TerrainGrounded,
        WorldRenderRoot,
        Name::new(name),
    )
}

/// While anything holds the player, they stand still whatever input says: a conversation, a
/// menu or a scripted moment has their attention. Each holder gives its own reason and lets
/// go of it alone, so one letting go does not free a player another still holds. Camera
/// controls are unaffected.
#[derive(Resource, Default, Clone, PartialEq, Eq, Debug)]
pub struct PlayerMovementSuspended(std::collections::BTreeSet<&'static str>);
impl PlayerMovementSuspended {
    /// Holds the player for `reason`, or lets go of it.
    pub fn hold(&mut self, reason: &'static str, held: bool) {
        if held {
            self.0.insert(reason);
        } else {
            self.0.remove(reason);
        }
    }
    pub fn held(&self) -> bool {
        !self.0.is_empty()
    }
}

pub(super) fn hold_suspended_player(
    suspended: Res<PlayerMovementSuspended>,
    mut player: Query<&mut MoveIntent, With<PlayerControlled>>,
) {
    if suspended.held() {
        for mut intent in &mut player {
            *intent = MoveIntent::default();
        }
    }
}

/// Walks the player through world-space points in order, steered as input would: a scripted
/// route for stress tests and profiling. Input is overridden while points remain.
#[derive(Resource, Clone, Debug)]
pub struct PlayerRoute {
    points: Vec<[f32; 3]>,
    next: usize,
    /// Overrides the character's jog speed, so long routes finish in reasonable time. Steps
    /// onto ground that is not yet certified still wait, as they do at normal speed.
    speed_mps: Option<f32>,
}
impl PlayerRoute {
    pub fn new(points: Vec<[f32; 3]>) -> Self {
        Self {
            points,
            next: 0,
            speed_mps: None,
        }
    }
    pub fn with_speed(self, speed_mps: f32) -> Self {
        Self {
            speed_mps: Some(speed_mps),
            ..self
        }
    }
    /// Points not reached yet.
    pub fn remaining(&self) -> usize {
        self.points.len() - self.next
    }
}

pub(super) fn steer_player_along_route(
    route: Option<ResMut<PlayerRoute>>,
    origin: Option<Res<crate::WorldOrigin>>,
    catalog: Option<Res<crate::WorldCatalog>>,
    mut player: Query<
        (
            &Transform,
            &mut MoveIntent,
            Option<&mut crate::actor::CharacterMotorConfig>,
        ),
        With<PlayerControlled>,
    >,
) {
    let (Some(mut route), Some(origin), Some(catalog)) = (route, origin, catalog) else {
        return;
    };
    let size = origin
        .space()
        .and_then(|space| catalog.world_space(space))
        .map_or(world::DEFAULT_CELL_SIZE, |space| space.cell_size);
    let offset = origin.cell().origin(size);
    for (transform, mut intent, config) in &mut player {
        // Re-applied each frame: the character's presentation may replace its config.
        if let (Some(speed), Some(mut config)) = (route.speed_mps, config) {
            config.jog_speed_mps = speed;
        }
        let here = Vec2::new(
            (f64::from(transform.translation.x) + offset[0]) as f32,
            (f64::from(transform.translation.z) + offset[1]) as f32,
        );
        let target =
            |route: &PlayerRoute| route.points.get(route.next).map(|p| Vec2::new(p[0], p[2]));
        while target(&route).is_some_and(|p| here.distance(p) < 3.) {
            route.next += 1;
        }
        match target(&route) {
            Some(p) => intent.set_direct(p - here, 1., Some(crate::actor::CharacterGait::Jog)),
            None => intent.set_direct(Vec2::ZERO, 0., None),
        }
    }
}

pub(super) fn move_player_to_adopted_start(
    mut adopted: MessageReader<crate::WorldStartAdopted>,
    mut player: Query<(&mut Transform, &mut MoveIntent), With<PlayerControlled>>,
) {
    let Some(crate::WorldStartAdopted(view)) = adopted.read().last() else {
        return;
    };
    for (mut transform, mut intent) in &mut player {
        // The origin is still the zero cell: the manifest has only just opened.
        transform.translation = Vec3::from_array(view.position);
        *intent = MoveIntent::default();
    }
}

pub(crate) fn ground_characters_to_streamed_terrain(
    origin: Res<WorldOrigin>,
    terrain_pages: Query<&StreamedTerrainSurface>,
    mut actors: Query<&mut Transform, With<TerrainGrounded>>,
    lod: Res<world_streaming::terrain_lod::TerrainLodStream>,
    lod_config: Res<TerrainHierarchy>,
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

#[cfg(test)]
mod suspension_tests {
    use super::*;
    #[test]
    fn the_player_stays_held_while_any_reason_remains() {
        let mut suspended = PlayerMovementSuspended::default();
        assert!(!suspended.held());
        suspended.hold("conversation", true);
        suspended.hold("menu", true);
        suspended.hold("conversation", false);
        assert!(suspended.held());
        suspended.hold("menu", false);
        assert!(!suspended.held());
    }
}
