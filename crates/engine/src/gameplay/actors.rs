//! Authoritative player root, motor and terrain contact.
use crate::{
    DEFAULT_CHARACTER_PRESENTATION_ID, WorldOrigin, WorldRenderRoot, WorldStartView,
    actor::{
        CameraTarget, CharacterMotion, CharacterMotor, MoveIntent, PlayerControlled,
        TerrainGrounded, WorldStreamFocus,
    },
    character::CharacterPresentationRef,
    world_streaming,
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

/// A grounded actor presented as a character catalog profile, standing where it is placed and
/// facing the way it is turned until something gives it a movement intent. For NPCs placed by a
/// gameplay adapter and the game's character probe.
pub fn standing_character(
    place: Transform,
    presentation: impl Into<String>,
    name: &'static str,
) -> impl Bundle {
    (
        place,
        Visibility::Inherited,
        MoveIntent::default(),
        CharacterMotor::facing((place.rotation * Vec3::Z).xz()),
        CharacterMotion::default(),
        CharacterPresentationRef::new(presentation),
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

/// Where a render-space translation lies in world XZ.
fn world_xz(origin: &crate::WorldOrigin, catalog: &crate::WorldCatalog, translation: Vec3) -> Vec2 {
    let offset = origin.world_offset(Some(catalog));
    Vec2::new(
        (f64::from(translation.x) + offset[0]) as f32,
        (f64::from(translation.z) + offset[1]) as f32,
    )
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

    for (transform, mut intent, config) in &mut player {
        // Re-applied each frame: the character's presentation may replace its config.
        if let (Some(speed), Some(mut config)) = (route.speed_mps, config) {
            config.jog_speed_mps = speed;
        }

        let here = world_xz(&origin, &catalog, transform.translation);
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

/// Route points ahead the camera looks toward, about 30 m along a road: it turns with the road's
/// bends rather than at every point.
const CAMERA_LOOK_AHEAD_POINTS: usize = 3;

/// Turns the camera behind the player to look along the route ahead, as a player keeps it on the
/// road; the rig eases there as it does after input. Where the route ends it stays.
pub(super) fn turn_camera_along_route(
    route: Option<Res<PlayerRoute>>,
    origin: Option<Res<crate::WorldOrigin>>,
    catalog: Option<Res<crate::WorldCatalog>>,
    player: Single<&Transform, With<PlayerControlled>>,
    mut rig: Single<&mut super::camera::CameraRig, With<super::camera::MainCamera>>,
) {
    let (Some(route), Some(origin), Some(catalog)) = (route, origin, catalog) else {
        return;
    };
    let ahead = (route.next + CAMERA_LOOK_AHEAD_POINTS).min(route.points.len().saturating_sub(1));
    let Some(point) = route.points.get(ahead).filter(|_| route.remaining() > 0) else {
        return;
    };
    let way = Vec2::new(point[0], point[2]) - world_xz(&origin, &catalog, player.translation);

    if way.length_squared() < 1.0 {
        return;
    }

    // The orbit's yaw puts the camera on the side opposite the way, the nearest turn from where
    // the rig aims now.
    let yaw = (-way.x).atan2(-way.y);
    let tau = std::f32::consts::TAU;
    rig.target_yaw = yaw + ((rig.target_yaw - yaw) / tau).round() * tau;
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

/// Actors stand on terrain v2's resident leaf surface, the triangles drawn near the camera,
/// never on ground not yet resident.
pub(crate) fn ground_characters(
    origin: Res<WorldOrigin>,
    mut actors: Query<&mut Transform, With<TerrainGrounded>>,
    terrain: Res<world_streaming::ClipmapTerrain>,
) {
    for mut transform in &mut actors {
        if let Some(height) = terrain.ground_height(transform.translation, &origin) {
            transform.translation.y = height;
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
