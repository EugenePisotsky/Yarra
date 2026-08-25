mod renderer;

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::{
    asset::Asset,
    prelude::*,
    reflect::TypePath,
    render::{
        extract_component::ExtractComponent,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
    },
    transform::TransformSystems,
};
use world::{GroundCoverPage, GroundCoverSpecies, PageKey};

pub(crate) const MAX_GROUND_COVER_INTERACTION_STAMPS: usize = 16;
const GROUND_COVER_INTERACTION_HISTORY_LIMIT: usize = 64;
const GROUND_COVER_INTERACTION_SAMPLE_SECONDS: f32 = 0.12;
const GROUND_COVER_INTERACTION_TELEPORT_METERS: f32 = 5.0;

/// Installs the ground-cover asset and rendering systems.
///
/// Streaming only hands this plugin page assets. GPU extraction, visibility,
/// LOD, and drawing remain private so grass, flowers, and other decorative
/// fields can evolve without coupling the world streamer to their renderer.
pub struct GroundCoverPlugin;

impl Plugin for GroundCoverPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<GroundCoverPageAsset>()
            .init_resource::<GroundCoverWind>()
            .init_resource::<GroundCoverDebug>()
            .init_resource::<GroundCoverInteractionState>()
            .init_resource::<GroundCoverInteraction>()
            .add_plugins((
                ExtractResourcePlugin::<GroundCoverWind>::default(),
                ExtractResourcePlugin::<GroundCoverDebug>::default(),
                ExtractResourcePlugin::<GroundCoverInteraction>::default(),
            ))
            .add_systems(
                Update,
                (advance_ground_cover_wind, cycle_ground_cover_debug),
            )
            .add_systems(
                PostUpdate,
                update_ground_cover_interaction.after(TransformSystems::Propagate),
            )
            .add_plugins(renderer::GroundCoverRenderPlugin);
    }
}

/// Marks a presented actor that pushes decorative ground cover aside.
///
/// Interaction remains a bounded visual field. It does not create collision bodies or persistent
/// state for individual clumps, and therefore remains independent of streamed page lifetime.
#[derive(Component, Debug, Clone, Copy)]
pub struct GroundCoverInteractor {
    pub radius: f32,
    pub maximum_displacement: f32,
    pub recovery_seconds: f32,
    priority: u8,
}

impl GroundCoverInteractor {
    /// Default response for the locally controlled character.
    pub const fn character() -> Self {
        Self {
            radius: 0.72,
            maximum_displacement: 0.48,
            recovery_seconds: 1.2,
            priority: 2,
        }
    }

    /// Default response for another presented moving actor.
    pub const fn actor() -> Self {
        Self {
            priority: 1,
            ..Self::character()
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GroundCoverInteractionStamp {
    pub(crate) start: Vec2,
    pub(crate) end: Vec2,
    /// Negative values mark live contact; released stamps contain normalized recovery age.
    pub(crate) start_recovery: f32,
    pub(crate) end_recovery: f32,
    pub(crate) radius: f32,
    pub(crate) maximum_displacement: f32,
}

#[derive(Resource, ExtractResource, Clone, Debug, Default)]
pub(crate) struct GroundCoverInteraction {
    pub(crate) stamps: Vec<GroundCoverInteractionStamp>,
}

#[derive(Resource, Default)]
struct GroundCoverInteractionState {
    actors: HashMap<Entity, TrackedGroundCoverInteractor>,
    released: VecDeque<ReleasedGroundCoverInteraction>,
}

#[derive(Clone, Copy, Debug)]
struct TrackedGroundCoverInteractor {
    current: Vec2,
    sample_position: Vec2,
    sample_elapsed: f32,
    settings: GroundCoverInteractor,
}

#[derive(Clone, Copy, Debug)]
struct ReleasedGroundCoverInteraction {
    start: Vec2,
    end: Vec2,
    age: f32,
    span_age: f32,
    radius: f32,
    maximum_displacement: f32,
    recovery_seconds: f32,
}

#[derive(Clone, Copy, Debug)]
struct GroundCoverInteractorSample {
    entity: Entity,
    position: Vec2,
    settings: GroundCoverInteractor,
}

fn update_ground_cover_interaction(
    time: Res<Time>,
    interactors: Query<(Entity, &GlobalTransform, &GroundCoverInteractor)>,
    mut state: ResMut<GroundCoverInteractionState>,
    mut interaction: ResMut<GroundCoverInteraction>,
) {
    let mut samples = interactors
        .iter()
        .map(
            |(entity, transform, settings)| GroundCoverInteractorSample {
                entity,
                position: transform.translation().xz(),
                settings: *settings,
            },
        )
        .collect::<Vec<_>>();
    samples.sort_by_key(|sample| sample.entity.to_bits());
    state.advance(time.delta_secs(), &samples);
    interaction.stamps = state.stamps();
}

impl GroundCoverInteractionState {
    fn advance(&mut self, delta_seconds: f32, actors: &[GroundCoverInteractorSample]) {
        let delta_seconds = delta_seconds.max(0.0);
        for stamp in &mut self.released {
            stamp.age += delta_seconds;
        }
        self.released
            .retain(|stamp| stamp.age < stamp.recovery_seconds.max(0.05));

        let mut seen = HashSet::with_capacity(actors.len());
        let mut newly_released = Vec::new();
        for actor in actors {
            seen.insert(actor.entity);
            let Some(tracked) = self.actors.get_mut(&actor.entity) else {
                self.actors.insert(
                    actor.entity,
                    TrackedGroundCoverInteractor {
                        current: actor.position,
                        sample_position: actor.position,
                        sample_elapsed: 0.0,
                        settings: actor.settings,
                    },
                );
                continue;
            };
            tracked.settings = actor.settings;
            let frame_start = tracked.current;
            let frame_delta = actor.position - frame_start;
            let frame_distance = frame_delta.length();
            if frame_distance > GROUND_COVER_INTERACTION_TELEPORT_METERS {
                tracked.current = actor.position;
                tracked.sample_position = actor.position;
                tracked.sample_elapsed = 0.0;
                continue;
            }
            tracked.current = actor.position;

            if frame_distance > 0.000_001 && delta_seconds > 0.000_001 {
                let elapsed_before_frame = tracked.sample_elapsed;
                let total_elapsed = elapsed_before_frame + delta_seconds;
                let sample_count = ((total_elapsed / GROUND_COVER_INTERACTION_SAMPLE_SECONDS)
                    .floor() as usize)
                    .min(GROUND_COVER_INTERACTION_HISTORY_LIMIT);
                for sample_index in 0..sample_count {
                    let boundary_time = GROUND_COVER_INTERACTION_SAMPLE_SECONDS
                        * (sample_index + 1) as f32
                        - elapsed_before_frame;
                    let frame_fraction = (boundary_time / delta_seconds).clamp(0.0, 1.0);
                    let endpoint = frame_start.lerp(actor.position, frame_fraction);
                    if endpoint.distance_squared(tracked.sample_position) > 0.000_001 {
                        newly_released.push(ReleasedGroundCoverInteraction {
                            start: tracked.sample_position,
                            end: endpoint,
                            age: (delta_seconds - boundary_time).max(0.0),
                            span_age: GROUND_COVER_INTERACTION_SAMPLE_SECONDS,
                            radius: tracked.settings.radius.max(0.01),
                            maximum_displacement: tracked.settings.maximum_displacement.max(0.0),
                            recovery_seconds: tracked.settings.recovery_seconds.max(0.05),
                        });
                    }
                    tracked.sample_position = endpoint;
                }
                tracked.sample_elapsed = (total_elapsed
                    - sample_count as f32 * GROUND_COVER_INTERACTION_SAMPLE_SECONDS)
                    .max(0.0);
            } else if tracked.sample_position.distance_squared(tracked.current) > 0.000_001 {
                newly_released.push(ReleasedGroundCoverInteraction {
                    start: tracked.sample_position,
                    end: tracked.current,
                    age: 0.0,
                    span_age: tracked.sample_elapsed,
                    radius: tracked.settings.radius.max(0.01),
                    maximum_displacement: tracked.settings.maximum_displacement.max(0.0),
                    recovery_seconds: tracked.settings.recovery_seconds.max(0.05),
                });
                tracked.sample_position = tracked.current;
                tracked.sample_elapsed = 0.0;
            }
        }
        self.released.extend(newly_released);

        let mut removed = self
            .actors
            .keys()
            .copied()
            .filter(|entity| !seen.contains(entity))
            .collect::<Vec<_>>();
        removed.sort_by_key(|entity| entity.to_bits());
        for entity in removed {
            if let Some(tracked) = self.actors.remove(&entity) {
                self.released.push_back(ReleasedGroundCoverInteraction {
                    start: tracked.sample_position,
                    end: tracked.current,
                    age: 0.0,
                    span_age: tracked.sample_elapsed,
                    radius: tracked.settings.radius.max(0.01),
                    maximum_displacement: tracked.settings.maximum_displacement.max(0.0),
                    recovery_seconds: tracked.settings.recovery_seconds.max(0.05),
                });
            }
        }
        while self.released.len() > GROUND_COVER_INTERACTION_HISTORY_LIMIT {
            self.released.pop_front();
        }
    }

    fn stamps(&self) -> Vec<GroundCoverInteractionStamp> {
        let mut actors = self.actors.iter().collect::<Vec<_>>();
        actors.sort_by_key(|(entity, actor)| {
            (std::cmp::Reverse(actor.settings.priority), entity.to_bits())
        });
        let mut stamps = actors
            .into_iter()
            .take(MAX_GROUND_COVER_INTERACTION_STAMPS)
            .map(|(_, actor)| GroundCoverInteractionStamp {
                start: actor.sample_position,
                end: actor.current,
                start_recovery: -1.0,
                end_recovery: -1.0,
                radius: actor.settings.radius.max(0.01),
                maximum_displacement: actor.settings.maximum_displacement.max(0.0),
            })
            .collect::<Vec<_>>();
        let remaining = MAX_GROUND_COVER_INTERACTION_STAMPS.saturating_sub(stamps.len());
        stamps.extend(self.released.iter().rev().take(remaining).map(|stamp| {
            let recovery_seconds = stamp.recovery_seconds.max(0.05);
            GroundCoverInteractionStamp {
                start: stamp.start,
                end: stamp.end,
                start_recovery: (stamp.age + stamp.span_age) / recovery_seconds,
                end_recovery: stamp.age / recovery_seconds,
                radius: stamp.radius,
                maximum_displacement: stamp.maximum_displacement,
            }
        }));
        stamps
    }
}

/// One coherent wind field shared by every ground-cover species.
///
/// These are authoring-facing controls: species decide their maximum response,
/// while this resource describes the current world's direction and motion.
#[derive(Resource, ExtractResource, Debug, Clone)]
pub struct GroundCoverWind {
    pub direction: Vec2,
    pub base_strength: f32,
    pub gust_strength: f32,
    pub spatial_scale: f32,
    pub speed: f32,
    pub elapsed_seconds: f32,
}

impl Default for GroundCoverWind {
    fn default() -> Self {
        Self {
            direction: Vec2::new(0.88, 0.47).normalize(),
            base_strength: 0.35,
            gust_strength: 0.65,
            spatial_scale: 0.12,
            speed: 1.15,
            elapsed_seconds: 0.0,
        }
    }
}

fn advance_ground_cover_wind(time: Res<Time>, mut wind: ResMut<GroundCoverWind>) {
    wind.elapsed_seconds = (wind.elapsed_seconds + time.delta_secs()) % 4096.0;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroundCoverDebugMode {
    #[default]
    Normal,
    LodColors,
    FarOnly,
    FarDisabled,
}

impl GroundCoverDebugMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::LodColors => "LOD colors",
            Self::FarOnly => "far only",
            Self::FarDisabled => "far disabled",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Normal => Self::LodColors,
            Self::LodColors => Self::FarOnly,
            Self::FarOnly => Self::FarDisabled,
            Self::FarDisabled => Self::Normal,
        }
    }

    pub(crate) fn gpu_value(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::LodColors => 1,
            Self::FarOnly => 2,
            Self::FarDisabled => 3,
        }
    }
}

#[derive(Resource, ExtractResource, Debug, Clone, Copy, Default)]
pub struct GroundCoverDebug {
    pub mode: GroundCoverDebugMode,
}

fn cycle_ground_cover_debug(
    keys: Res<ButtonInput<KeyCode>>,
    mut ground_cover_debug: ResMut<GroundCoverDebug>,
) {
    if !keys.just_pressed(KeyCode::KeyG) {
        return;
    }
    ground_cover_debug.mode = ground_cover_debug.mode.next();
    warn!(
        "ground-cover debug mode: {}",
        ground_cover_debug.mode.label()
    );
}

/// A resolved runtime page ready for renderer upload.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct GroundCoverPageAsset {
    pub key: PageKey,
    pub cell_size: f32,
    pub page: GroundCoverPage,
    pub species: Vec<GroundCoverSpecies>,
}

/// Main-world attachment for one resident ground-cover page.
#[derive(Component, Debug, Clone, Deref, DerefMut)]
pub struct GroundCoverPage3d(pub Handle<GroundCoverPageAsset>);

/// Marks a camera as a view that should render ground cover and carries its resolved zoom.
#[derive(Component, ExtractComponent, Debug, Clone, Copy, Default)]
pub struct GroundCoverView {
    /// Zero is the closest third-person view and one is the original full-overhead reference.
    /// A game may cap its playable camera before reaching one.
    pub normalized_zoom: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(entity: Entity, position: Vec2) -> [GroundCoverInteractorSample; 1] {
        [GroundCoverInteractorSample {
            entity,
            position,
            settings: GroundCoverInteractor::character(),
        }]
    }

    #[test]
    fn interaction_keeps_live_contact_and_releases_a_decaying_sweep() {
        let entity = World::new().spawn_empty().id();
        let mut state = GroundCoverInteractionState::default();
        state.advance(0.0, &samples(entity, Vec2::ZERO));
        state.advance(0.24, &samples(entity, Vec2::X));

        let moving = state.stamps();
        assert!(moving[0].start_recovery < 0.0);
        assert!(moving.iter().skip(1).any(|stamp| {
            stamp.start_recovery >= 0.0 && stamp.start.distance_squared(stamp.end) > 0.0
        }));

        state.advance(1.21, &samples(entity, Vec2::X));
        assert_eq!(state.stamps().len(), 1, "only live contact should remain");
    }

    #[test]
    fn interaction_does_not_sweep_across_a_teleport() {
        let entity = World::new().spawn_empty().id();
        let mut state = GroundCoverInteractionState::default();
        state.advance(0.0, &samples(entity, Vec2::ZERO));
        state.advance(
            1.0 / 60.0,
            &samples(
                entity,
                Vec2::X * (GROUND_COVER_INTERACTION_TELEPORT_METERS + 1.0),
            ),
        );

        let stamps = state.stamps();
        assert_eq!(stamps.len(), 1);
        assert_eq!(stamps[0].start, stamps[0].end);
    }

    #[test]
    fn visible_instance_layout_matches_the_shader_storage_stride() {
        assert_eq!(size_of::<renderer::VisibleInstanceGpu>(), 80);
    }
}
