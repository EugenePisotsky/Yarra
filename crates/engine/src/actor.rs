//! Reusable actor intent, locomotion, and observable motion state.
//!
//! Input, follower steering, and future NPC logic all write `MoveIntent`.
//! The motor owns world movement; character presentation only observes the
//! resulting `CharacterMotion` and never moves the actor root.

use bevy::prelude::*;

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct PlayerControlled;

/// Marks the one actor followed by the gameplay camera.
///
/// Party members and NPCs deliberately do not receive this role.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct CameraTarget;

/// Marks the actor whose position drives world-page preload and residency.
///
/// This is separate from `CameraTarget` so neither policy is implied merely by
/// being a presented or moving actor.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct WorldStreamFocus;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CharacterGait {
    #[default]
    Walk,
    Jog,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CharacterMotionPhase {
    #[default]
    Idle,
    Moving,
    Releasing,
    Arriving,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MovementDestination {
    position: Vec3,
    gait: CharacterGait,
    revision: u64,
}

/// One frame's desired movement, independent of who produced it.
///
/// Direct input temporarily overrides and cancels a destination. A future
/// follower or NPC can use the same component without depending on player
/// input, the camera, or character presentation.
#[derive(Component, Clone, Copy, Debug, Default)]
pub(crate) struct MoveIntent {
    direction: Vec2,
    strength: f32,
    requested_gait: Option<CharacterGait>,
    destination: Option<MovementDestination>,
    next_destination_revision: u64,
}

impl MoveIntent {
    pub(crate) fn set_direct(
        &mut self,
        direction: Vec2,
        strength: f32,
        requested_gait: Option<CharacterGait>,
    ) {
        self.direction = direction.normalize_or_zero();
        self.strength = strength.clamp(0.0, 1.0);
        self.requested_gait = requested_gait;
        if self.strength > 0.0 && self.direction != Vec2::ZERO {
            self.destination = None;
        }
    }

    pub(crate) fn set_destination(&mut self, position: Vec3, gait: CharacterGait) {
        self.next_destination_revision = self.next_destination_revision.wrapping_add(1).max(1);
        self.destination = Some(MovementDestination {
            position,
            gait,
            revision: self.next_destination_revision,
        });
        self.direction = Vec2::ZERO;
        self.strength = 0.0;
        self.requested_gait = None;
    }

    pub(crate) fn destination(&self) -> Option<Vec3> {
        self.destination.map(|destination| destination.position)
    }

    pub(crate) fn clear(&mut self) {
        self.direction = Vec2::ZERO;
        self.strength = 0.0;
        self.requested_gait = None;
        self.destination = None;
    }
}

/// Authored locomotion values for one compatible model and animation set.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct CharacterMotorConfig {
    pub(crate) walk_speed_mps: f32,
    pub(crate) jog_speed_mps: f32,
    pub(crate) input_dead_zone: f32,
    pub(crate) input_response_exponent: f32,
    pub(crate) walk_to_jog_threshold: f32,
    pub(crate) jog_to_walk_threshold: f32,
    pub(crate) walk_acceleration_seconds: f32,
    pub(crate) jog_acceleration_seconds: f32,
    pub(crate) release_seconds: f32,
    pub(crate) gait_blend_seconds: f32,
    pub(crate) arrival_radius_m: f32,
    pub(crate) walk_turn_rate_radians: f32,
    pub(crate) jog_turn_rate_radians: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DestinationBraking {
    revision: u64,
    elapsed_seconds: f32,
    duration_seconds: f32,
    start_speed_mps: f32,
}

/// Stateful locomotion controller shared by controlled, party, and NPC actors.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct CharacterMotor {
    gait: CharacterGait,
    phase: CharacterMotionPhase,
    facing: Vec2,
    speed_mps: f32,
    destination_revision: Option<u64>,
    destination_braking: Option<DestinationBraking>,
}

impl Default for CharacterMotor {
    fn default() -> Self {
        Self {
            gait: CharacterGait::Walk,
            phase: CharacterMotionPhase::Idle,
            facing: Vec2::Y,
            speed_mps: 0.0,
            destination_revision: None,
            destination_braking: None,
        }
    }
}

/// Actual motion produced by the motor during the latest update.
///
/// Presentation consumes this output rather than raw input. When collision is
/// introduced, its resolved displacement should become the value published
/// here, leaving the animation contract unchanged.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct CharacterMotion {
    pub(crate) displacement: Vec2,
    pub(crate) speed_mps: f32,
    pub(crate) playback_rate: f32,
    pub(crate) gait: CharacterGait,
    pub(crate) phase: CharacterMotionPhase,
}

impl Default for CharacterMotion {
    fn default() -> Self {
        Self {
            displacement: Vec2::ZERO,
            speed_mps: 0.0,
            playback_rate: 1.0,
            gait: CharacterGait::Walk,
            phase: CharacterMotionPhase::Idle,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MotorOutput {
    displacement: Vec2,
    speed_mps: f32,
    playback_rate: f32,
    gait: CharacterGait,
    phase: CharacterMotionPhase,
    arrived: bool,
}

pub(crate) fn advance_character_motors(
    time: Res<Time>,
    mut actors: Query<(
        &mut Transform,
        &mut MoveIntent,
        &CharacterMotorConfig,
        &mut CharacterMotor,
        &mut CharacterMotion,
    )>,
) {
    let delta_seconds = time.delta_secs();
    for (mut transform, mut intent, config, mut motor, mut motion) in &mut actors {
        let output = if let Some(destination) = intent.destination {
            let offset = destination.position - transform.translation;
            motor.update_destination(
                Vec2::new(offset.x, offset.z),
                destination.gait,
                destination.revision,
                config,
                delta_seconds,
            )
        } else {
            motor.update_direct(*intent, config, delta_seconds)
        };

        transform.translation.x += output.displacement.x;
        transform.translation.z += output.displacement.y;
        if motor.facing.length_squared() > f32::EPSILON {
            // The validated character asset faces local +Z.
            transform.rotation = Quat::from_rotation_y(motor.facing.x.atan2(motor.facing.y));
        }
        if output.arrived {
            intent.clear();
        }
        *motion = CharacterMotion {
            displacement: output.displacement,
            speed_mps: output.speed_mps,
            playback_rate: output.playback_rate,
            gait: output.gait,
            phase: output.phase,
        };
    }
}

impl CharacterMotor {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    fn update_direct(
        &mut self,
        intent: MoveIntent,
        config: &CharacterMotorConfig,
        delta_seconds: f32,
    ) -> MotorOutput {
        self.destination_revision = None;
        self.destination_braking = None;
        let strength = remap_input_strength(
            intent.strength,
            config.input_dead_zone,
            config.input_response_exponent,
        );

        let displacement = if strength > 0.0 && intent.direction != Vec2::ZERO {
            self.gait = intent.requested_gait.unwrap_or(match self.gait {
                CharacterGait::Walk if strength >= config.walk_to_jog_threshold => {
                    CharacterGait::Jog
                }
                CharacterGait::Jog if strength <= config.jog_to_walk_threshold => {
                    CharacterGait::Walk
                }
                gait => gait,
            });
            let desired_speed = desired_speed(config, self.gait, strength);
            let acceleration_seconds = match self.gait {
                CharacterGait::Walk => config.walk_acceleration_seconds,
                CharacterGait::Jog if self.speed_mps <= config.walk_speed_mps => {
                    config.jog_acceleration_seconds
                }
                CharacterGait::Jog => config.gait_blend_seconds,
            };
            let maximum_delta =
                maximum_speed(config, self.gait) / acceleration_seconds * delta_seconds;
            self.speed_mps = approach(self.speed_mps, desired_speed, maximum_delta);
            self.phase = CharacterMotionPhase::Moving;
            let turn_rate = match self.gait {
                CharacterGait::Walk => config.walk_turn_rate_radians,
                CharacterGait::Jog => config.jog_turn_rate_radians,
            };
            self.facing = rotate_towards(self.facing, intent.direction, turn_rate * delta_seconds);
            self.facing * self.speed_mps * delta_seconds
        } else if self.speed_mps > 0.001 {
            let maximum_delta =
                maximum_speed(config, self.gait) / config.release_seconds * delta_seconds;
            self.speed_mps = approach(self.speed_mps, 0.0, maximum_delta);
            self.phase = if self.speed_mps > 0.001 {
                CharacterMotionPhase::Releasing
            } else {
                CharacterMotionPhase::Idle
            };
            self.facing * self.speed_mps * delta_seconds
        } else {
            self.speed_mps = 0.0;
            self.phase = CharacterMotionPhase::Idle;
            self.gait = CharacterGait::Walk;
            Vec2::ZERO
        };

        self.output(displacement, false, config)
    }

    fn update_destination(
        &mut self,
        offset: Vec2,
        gait: CharacterGait,
        revision: u64,
        config: &CharacterMotorConfig,
        delta_seconds: f32,
    ) -> MotorOutput {
        let remaining_distance = if offset.is_finite() {
            offset.length()
        } else {
            0.0
        };
        let direction = offset.normalize_or_zero();
        if self.destination_revision != Some(revision) {
            self.destination_revision = Some(revision);
            self.destination_braking = None;
        }
        if remaining_distance <= config.arrival_radius_m || direction == Vec2::ZERO {
            return self.finish_destination(direction * remaining_distance, config);
        }

        self.gait = gait;
        let turn_rate = match gait {
            CharacterGait::Walk => config.walk_turn_rate_radians,
            CharacterGait::Jog => config.jog_turn_rate_radians,
        };
        self.facing = rotate_towards(self.facing, direction, turn_rate * delta_seconds);

        if self
            .destination_braking
            .is_some_and(|braking| braking.revision == revision)
        {
            return self.advance_destination_braking(
                direction,
                remaining_distance,
                revision,
                config,
                delta_seconds,
            );
        }

        if gait == CharacterGait::Walk && self.speed_mps > 0.05 {
            let braking_duration = 2.0 * remaining_distance / self.speed_mps;
            if braking_duration <= config.release_seconds {
                self.destination_braking = Some(DestinationBraking {
                    revision,
                    elapsed_seconds: 0.0,
                    duration_seconds: braking_duration.max(f32::EPSILON),
                    start_speed_mps: self.speed_mps,
                });
                return self.advance_destination_braking(
                    direction,
                    remaining_distance,
                    revision,
                    config,
                    delta_seconds,
                );
            }
        }

        let desired_speed = maximum_speed(config, gait);
        let acceleration_seconds = match gait {
            CharacterGait::Walk => config.walk_acceleration_seconds,
            CharacterGait::Jog if self.speed_mps <= config.walk_speed_mps => {
                config.jog_acceleration_seconds
            }
            CharacterGait::Jog => config.gait_blend_seconds,
        };
        let maximum_delta = desired_speed / acceleration_seconds * delta_seconds;
        self.speed_mps = approach(self.speed_mps, desired_speed, maximum_delta);
        self.phase = CharacterMotionPhase::Moving;

        let maximum_step = self.speed_mps * delta_seconds;
        let (displacement, arrived) =
            destination_displacement(self.facing, direction, remaining_distance, maximum_step);
        if arrived {
            self.finish_destination(displacement, config)
        } else {
            self.output(displacement, false, config)
        }
    }

    fn advance_destination_braking(
        &mut self,
        direction: Vec2,
        remaining_distance: f32,
        revision: u64,
        config: &CharacterMotorConfig,
        delta_seconds: f32,
    ) -> MotorOutput {
        let braking = self
            .destination_braking
            .as_mut()
            .expect("destination braking was checked before it advanced");
        let duration = braking.duration_seconds.max(f32::EPSILON);
        let previous_time = braking.elapsed_seconds.clamp(0.0, duration);
        let next_time = (previous_time + delta_seconds).min(duration);
        let distance_at = |time: f32| {
            let progress = (time / duration).clamp(0.0, 1.0);
            braking.start_speed_mps * duration * (progress - 0.5 * progress * progress)
        };
        let maximum_step = (distance_at(next_time) - distance_at(previous_time)).max(0.0);
        braking.elapsed_seconds = next_time;
        self.speed_mps = maximum_step / delta_seconds.max(f32::EPSILON);
        self.phase = CharacterMotionPhase::Arriving;

        let (displacement, arrived) =
            destination_displacement(self.facing, direction, remaining_distance, maximum_step);
        if arrived {
            return self.finish_destination(displacement, config);
        }
        if next_time + f32::EPSILON >= duration {
            self.destination_braking = None;
            self.speed_mps = 0.0;
        }
        self.gait = CharacterGait::Walk;
        self.destination_revision = Some(revision);
        self.output(displacement, false, config)
    }

    fn finish_destination(
        &mut self,
        displacement: Vec2,
        config: &CharacterMotorConfig,
    ) -> MotorOutput {
        self.destination_braking = None;
        self.speed_mps = 0.0;
        self.phase = CharacterMotionPhase::Idle;
        self.gait = CharacterGait::Walk;
        self.output(displacement, true, config)
    }

    fn output(
        &self,
        displacement: Vec2,
        arrived: bool,
        config: &CharacterMotorConfig,
    ) -> MotorOutput {
        let authored_speed = maximum_speed(config, self.gait);
        let playback_rate = if self.speed_mps <= 0.001 {
            1.0
        } else {
            (self.speed_mps / authored_speed).clamp(0.75, 1.12)
        };
        MotorOutput {
            displacement,
            speed_mps: self.speed_mps,
            playback_rate,
            gait: self.gait,
            phase: self.phase,
            arrived,
        }
    }
}

fn destination_displacement(
    facing: Vec2,
    direction: Vec2,
    remaining_distance: f32,
    maximum_step: f32,
) -> (Vec2, bool) {
    if remaining_distance <= maximum_step + f32::EPSILON {
        (direction * remaining_distance, true)
    } else {
        (facing * maximum_step, false)
    }
}

fn remap_input_strength(raw: f32, dead_zone: f32, exponent: f32) -> f32 {
    let normalized = ((raw.clamp(0.0, 1.0) - dead_zone) / (1.0 - dead_zone)).clamp(0.0, 1.0);
    normalized.powf(exponent)
}

fn desired_speed(config: &CharacterMotorConfig, gait: CharacterGait, strength: f32) -> f32 {
    match gait {
        CharacterGait::Walk => {
            let fraction = (strength / config.walk_to_jog_threshold).clamp(0.0, 1.0);
            config.walk_speed_mps * fraction.max(0.75)
        }
        CharacterGait::Jog => {
            let span = (1.0 - config.jog_to_walk_threshold).max(f32::EPSILON);
            let alpha = ((strength - config.jog_to_walk_threshold) / span).clamp(0.0, 1.0);
            config.walk_speed_mps + (config.jog_speed_mps - config.walk_speed_mps) * alpha
        }
    }
}

fn maximum_speed(config: &CharacterMotorConfig, gait: CharacterGait) -> f32 {
    match gait {
        CharacterGait::Walk => config.walk_speed_mps,
        CharacterGait::Jog => config.jog_speed_mps,
    }
}

fn approach(current: f32, target: f32, maximum_delta: f32) -> f32 {
    current + (target - current).clamp(-maximum_delta.max(0.0), maximum_delta.max(0.0))
}

fn rotate_towards(current: Vec2, target: Vec2, maximum_angle: f32) -> Vec2 {
    let current_angle = current.y.atan2(current.x);
    let target_angle = target.y.atan2(target.x);
    let difference = (target_angle - current_angle + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    let next = current_angle + difference.clamp(-maximum_angle, maximum_angle);
    Vec2::new(next.cos(), next.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELTA_SECONDS: f32 = 1.0 / 60.0;

    fn test_config() -> CharacterMotorConfig {
        CharacterMotorConfig {
            walk_speed_mps: 1.968_607_5,
            jog_speed_mps: 2.941_954,
            input_dead_zone: 0.18,
            input_response_exponent: 1.0,
            walk_to_jog_threshold: 0.72,
            jog_to_walk_threshold: 0.60,
            walk_acceleration_seconds: 0.22,
            jog_acceleration_seconds: 0.22,
            release_seconds: 0.16,
            gait_blend_seconds: 0.24,
            arrival_radius_m: 0.01,
            walk_turn_rate_radians: 7.068_583_5,
            jog_turn_rate_radians: 5.497_787,
        }
    }

    fn advance_destination(
        motor: &mut CharacterMotor,
        position: &mut Vec2,
        target: Vec2,
        revision: u64,
    ) -> MotorOutput {
        let output = motor.update_destination(
            target - *position,
            CharacterGait::Walk,
            revision,
            &test_config(),
            DELTA_SECONDS,
        );
        *position += output.displacement;
        output
    }

    #[test]
    fn analog_strength_selects_walk_and_jog_with_hysteresis() {
        let config = test_config();
        let mut motor = CharacterMotor::default();
        let intent = |strength| MoveIntent {
            direction: Vec2::Y,
            strength,
            ..default()
        };

        motor.update_direct(intent(0.6), &config, 0.1);
        assert_eq!(motor.gait, CharacterGait::Walk);
        motor.update_direct(intent(1.0), &config, 0.1);
        assert_eq!(motor.gait, CharacterGait::Jog);
        motor.update_direct(intent(0.7), &config, 0.1);
        assert_eq!(motor.gait, CharacterGait::Jog);
        motor.update_direct(intent(0.5), &config, 0.1);
        assert_eq!(motor.gait, CharacterGait::Walk);
    }

    #[test]
    fn release_decelerates_and_reaches_idle() {
        let config = test_config();
        let mut motor = CharacterMotor::default();
        motor.update_direct(
            MoveIntent {
                direction: Vec2::Y,
                strength: 1.0,
                ..default()
            },
            &config,
            config.jog_acceleration_seconds,
        );
        assert!(motor.speed_mps > 0.0);

        motor.update_direct(MoveIntent::default(), &config, config.release_seconds);

        assert_eq!(motor.phase, CharacterMotionPhase::Idle);
        assert_eq!(motor.speed_mps, 0.0);
    }

    #[test]
    fn destination_brakes_and_arrives_exactly() {
        let target = Vec2::new(0.0, 3.0);
        let mut position = Vec2::ZERO;
        let mut motor = CharacterMotor::default();
        let mut saw_arriving = false;
        let mut arrived = false;

        for _ in 0..240 {
            let output = advance_destination(&mut motor, &mut position, target, 7);
            saw_arriving |= output.phase == CharacterMotionPhase::Arriving;
            if output.arrived {
                arrived = true;
                break;
            }
        }

        assert!(saw_arriving);
        assert!(arrived);
        assert_eq!(position, target);
        assert_eq!(motor.phase, CharacterMotionPhase::Idle);
    }

    #[test]
    fn a_new_destination_revision_preserves_motion() {
        let mut position = Vec2::ZERO;
        let mut motor = CharacterMotor::default();
        let first_target = Vec2::Y;
        for _ in 0..120 {
            let output = advance_destination(&mut motor, &mut position, first_target, 10);
            if output.phase == CharacterMotionPhase::Arriving {
                break;
            }
        }
        assert_eq!(motor.phase, CharacterMotionPhase::Arriving);
        let speed_before = motor.speed_mps;
        let second_target = position + Vec2::X * 2.0;

        let output = advance_destination(&mut motor, &mut position, second_target, 11);

        assert_eq!(output.phase, CharacterMotionPhase::Moving);
        assert!(!output.arrived);
        assert!(output.speed_mps >= speed_before);
        assert!(output.displacement.length() > 0.0);
    }

    #[test]
    fn role_markers_are_independent_components() {
        let mut world = World::new();
        let follower = world
            .spawn((
                MoveIntent::default(),
                CharacterMotor::default(),
                CharacterMotion::default(),
            ))
            .id();
        let leader = world
            .spawn((
                MoveIntent::default(),
                CharacterMotor::default(),
                CharacterMotion::default(),
                CameraTarget,
                WorldStreamFocus,
            ))
            .id();

        assert!(world.get::<CameraTarget>(follower).is_none());
        assert!(world.get::<WorldStreamFocus>(follower).is_none());
        assert!(world.get::<CameraTarget>(leader).is_some());
        assert!(world.get::<WorldStreamFocus>(leader).is_some());
    }
}
