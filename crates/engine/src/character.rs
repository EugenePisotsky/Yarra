//! Catalog-backed character scene loading and locomotion presentation.
//!
//! Character visuals are children of authoritative actor roots. They observe
//! `CharacterMotion`; neither scene loading nor animation can move gameplay,
//! camera, or streaming state.

use std::{collections::HashMap, time::Duration};

use bevy::{gltf::GltfAssetLabel, prelude::*};

use crate::{
    actor::{
        CharacterGait, CharacterMotion, CharacterMotionPhase, CharacterMotorConfig,
        advance_character_motors,
    },
    character_catalog::{
        AnimationBankDefinition, CharacterModelDefinition, CharacterPresentationCatalog,
        MovementSetDefinition,
    },
};

pub(crate) struct CharacterPresentationPlugin;

impl Plugin for CharacterPresentationPlugin {
    fn build(&self, app: &mut App) {
        let catalog = CharacterPresentationCatalog::load()
            .unwrap_or_else(|error| panic!("invalid character presentation catalog: {error}"));
        app.insert_resource(CharacterCatalogResource(catalog))
            .init_resource::<CharacterAssetCache>()
            .add_systems(
                Update,
                (
                    resolve_character_presentations.in_set(CharacterPresentationResolveSet),
                    prepare_character_animation_graphs,
                    attach_character_animators,
                    drive_character_animation.after(advance_character_motors),
                )
                    .chain(),
            );
    }
}

#[derive(SystemSet, Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CharacterPresentationResolveSet;

/// Selects the complete visual and default movement presentation for an actor.
///
/// Persistent character identity, player control, camera focus, and streaming
/// roles are deliberately separate components.
#[derive(Component, Clone, Debug, Eq, PartialEq)]
pub(crate) struct CharacterPresentationRef {
    profile_id: String,
}

impl CharacterPresentationRef {
    pub(crate) fn new(profile_id: impl Into<String>) -> Self {
        Self {
            profile_id: profile_id.into(),
        }
    }
}

#[derive(Resource)]
struct CharacterCatalogResource(CharacterPresentationCatalog);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LocomotionVisual {
    Idle,
    Walk,
    Jog,
}

#[derive(Clone, Copy)]
struct CharacterAnimationNodes {
    idle: AnimationNodeIndex,
    walk: AnimationNodeIndex,
    jog: AnimationNodeIndex,
}

impl CharacterAnimationNodes {
    fn get(self, visual: LocomotionVisual) -> AnimationNodeIndex {
        match visual {
            LocomotionVisual::Idle => self.idle,
            LocomotionVisual::Walk => self.walk,
            LocomotionVisual::Jog => self.jog,
        }
    }
}

#[derive(Clone)]
struct CharacterAnimationClips {
    idle: Handle<AnimationClip>,
    walk: Handle<AnimationClip>,
    jog: Handle<AnimationClip>,
}

impl CharacterAnimationClips {
    fn get(&self, visual: LocomotionVisual) -> &Handle<AnimationClip> {
        match visual {
            LocomotionVisual::Idle => &self.idle,
            LocomotionVisual::Walk => &self.walk,
            LocomotionVisual::Jog => &self.jog,
        }
    }
}

#[derive(Clone, Copy)]
struct CharacterBlendTimings {
    start_seconds: f32,
    stop_seconds: f32,
    gait_seconds: f32,
}

struct PreparedMovementAnimations {
    graph: Handle<AnimationGraph>,
    nodes: CharacterAnimationNodes,
    clips: CharacterAnimationClips,
    blends: CharacterBlendTimings,
}

struct CharacterAnimationBank {
    source: Handle<Gltf>,
}

struct CharacterMovementAnimationCache {
    bank_id: String,
    prepared: Option<PreparedMovementAnimations>,
    failed: bool,
}

#[derive(Resource, Default)]
struct CharacterAssetCache {
    models: HashMap<String, Handle<WorldAsset>>,
    animation_banks: HashMap<String, CharacterAnimationBank>,
    movement_sets: HashMap<String, CharacterMovementAnimationCache>,
}

impl CharacterAssetCache {
    fn model_scene(
        &mut self,
        model: &CharacterModelDefinition,
        asset_server: &AssetServer,
    ) -> Handle<WorldAsset> {
        self.models
            .entry(model.id.clone())
            .or_insert_with(|| {
                asset_server.load(
                    GltfAssetLabel::Scene(model.visual.scene)
                        .from_asset(model.visual.asset.clone()),
                )
            })
            .clone()
    }

    fn request_movement_set(
        &mut self,
        set: &MovementSetDefinition,
        bank: &AnimationBankDefinition,
        asset_server: &AssetServer,
    ) {
        self.animation_banks
            .entry(bank.id.clone())
            .or_insert_with(|| CharacterAnimationBank {
                source: asset_server.load(bank.asset.clone()),
            });
        self.movement_sets.entry(set.id.clone()).or_insert_with(|| {
            CharacterMovementAnimationCache {
                bank_id: bank.id.clone(),
                prepared: None,
                failed: false,
            }
        });
    }

    fn prepared_movement_set(&self, id: &str) -> Option<&PreparedMovementAnimations> {
        self.movement_sets
            .get(id)
            .and_then(|cached| cached.prepared.as_ref())
    }
}

/// Marks the scene root below an actor. The imported model may own any number
/// of descendants; only the actor parent owns gameplay state.
#[derive(Component)]
struct CharacterVisual;

#[derive(Component)]
struct ResolvedCharacterPresentation {
    movement_set_id: String,
}

/// Added to an actor root after its imported scene has an animation graph and
/// a running Idle controller.
#[derive(Component)]
pub(crate) struct CharacterPresentationReady;

/// Editor-facing locomotion preview selector.
///
/// This component deliberately carries no gameplay, camera, or streaming role. The preview plugin
/// translates it into the same private presentation components used by runtime actors.
#[derive(Component, Clone, Debug)]
pub struct CharacterPresentationPreview {
    profile_id: String,
    clip: CharacterPreviewClip,
    playing: bool,
    speed: f32,
    restart_generation: u64,
}

impl CharacterPresentationPreview {
    pub fn new(profile_id: impl Into<String>) -> Self {
        Self {
            profile_id: profile_id.into(),
            clip: CharacterPreviewClip::Idle,
            playing: false,
            speed: 1.0,
            restart_generation: 0,
        }
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn set_profile(&mut self, profile_id: impl Into<String>) {
        self.profile_id = profile_id.into();
    }

    pub fn clip(&self) -> CharacterPreviewClip {
        self.clip
    }

    pub fn set_clip(&mut self, clip: CharacterPreviewClip) {
        self.clip = clip;
    }

    pub fn set_transport(&mut self, playing: bool, speed: f32) {
        self.playing = playing;
        self.speed = speed.clamp(0.05, 4.0);
    }

    pub fn restart(&mut self) {
        self.restart_generation = self.restart_generation.wrapping_add(1).max(1);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CharacterPreviewClip {
    #[default]
    Idle,
    Walk,
    Jog,
}

#[derive(Component, Debug, Default)]
struct CharacterPreviewRestartState(u64);

/// Loads and drives real catalog-backed models and clips for isolated editor preview actors.
pub struct CharacterPresentationPreviewPlugin;

impl Plugin for CharacterPresentationPreviewPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CharacterPresentationPlugin).add_systems(
            Update,
            (
                apply_character_preview.before(CharacterPresentationResolveSet),
                restart_character_preview.after(drive_character_animation),
            ),
        );
    }
}

fn apply_character_preview(
    mut commands: Commands,
    mut previews: Query<(
        Entity,
        &CharacterPresentationPreview,
        Option<&CharacterPresentationRef>,
        Option<&mut CharacterMotion>,
    )>,
) {
    for (entity, preview, presentation, motion) in &mut previews {
        if presentation.is_none_or(|presentation| presentation.profile_id != preview.profile_id) {
            commands
                .entity(entity)
                .insert(CharacterPresentationRef::new(preview.profile_id.clone()));
        }
        let mut desired = CharacterMotion {
            playback_rate: if preview.playing { preview.speed } else { 0.0 },
            ..default()
        };
        match preview.clip {
            CharacterPreviewClip::Idle => {}
            CharacterPreviewClip::Walk => {
                desired.displacement = Vec2::Y * 0.01;
                desired.speed_mps = 1.0;
                desired.gait = CharacterGait::Walk;
                desired.phase = CharacterMotionPhase::Moving;
            }
            CharacterPreviewClip::Jog => {
                desired.displacement = Vec2::Y * 0.01;
                desired.speed_mps = 3.0;
                desired.gait = CharacterGait::Jog;
                desired.phase = CharacterMotionPhase::Moving;
            }
        }
        if let Some(mut motion) = motion {
            *motion = desired;
        } else {
            commands
                .entity(entity)
                .insert((desired, CharacterPreviewRestartState::default()));
        }
    }
}

fn restart_character_preview(
    assets: Res<CharacterAssetCache>,
    previews: Query<&CharacterPresentationPreview>,
    mut restart_states: Query<&mut CharacterPreviewRestartState>,
    mut players: Query<(
        &mut AnimationPlayer,
        &mut AnimationTransitions,
        &CharacterAnimator,
    )>,
) {
    for (mut player, mut transitions, animator) in &mut players {
        let Ok(preview) = previews.get(animator.actor) else {
            continue;
        };
        let Ok(mut restart_state) = restart_states.get_mut(animator.actor) else {
            continue;
        };
        if restart_state.0 == preview.restart_generation {
            continue;
        }
        let Some(animations) = assets.prepared_movement_set(&animator.movement_set_id) else {
            continue;
        };
        let visual = match preview.clip {
            CharacterPreviewClip::Idle => LocomotionVisual::Idle,
            CharacterPreviewClip::Walk => LocomotionVisual::Walk,
            CharacterPreviewClip::Jog => LocomotionVisual::Jog,
        };
        transitions
            .play(&mut player, animations.nodes.get(visual), Duration::ZERO)
            .set_speed(if preview.playing { preview.speed } else { 0.0 })
            .set_seek_time(0.0)
            .repeat();
        restart_state.0 = preview.restart_generation;
    }
}

#[derive(Component)]
struct CharacterAnimator {
    actor: Entity,
    movement_set_id: String,
    visual: LocomotionVisual,
}

type ChangedCharacterPresentations<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static CharacterPresentationRef),
    (
        With<CharacterMotion>,
        Or<(
            Added<CharacterPresentationRef>,
            Changed<CharacterPresentationRef>,
        )>,
    ),
>;

fn resolve_character_presentations(
    mut commands: Commands,
    catalog: Res<CharacterCatalogResource>,
    mut assets: ResMut<CharacterAssetCache>,
    asset_server: Res<AssetServer>,
    actors: ChangedCharacterPresentations,
    visuals: Query<(Entity, &ChildOf), With<CharacterVisual>>,
) {
    for (actor, presentation_ref) in &actors {
        let resolved = match catalog.0.resolve_default(&presentation_ref.profile_id) {
            Ok(resolved) => resolved,
            Err(error) => {
                error!("actor {actor} has an invalid character presentation: {error}");
                continue;
            }
        };
        let scene = assets.model_scene(resolved.model, &asset_server);
        assets.request_movement_set(
            resolved.movement_set,
            resolved.animation_bank,
            &asset_server,
        );

        for (visual, parent) in &visuals {
            if parent.parent() == actor {
                commands.entity(visual).despawn();
            }
        }
        commands.entity(actor).remove::<(
            ResolvedCharacterPresentation,
            CharacterMotorConfig,
            CharacterPresentationReady,
        )>();
        commands.entity(actor).insert((
            ResolvedCharacterPresentation {
                movement_set_id: resolved.movement_set.id.clone(),
            },
            resolved.movement_set.motor_config(),
        ));

        let visual = &resolved.model.visual;
        commands.spawn((
            WorldAssetRoot(scene),
            Transform {
                translation: Vec3::from_array(visual.translation_m),
                rotation: Quat::from_rotation_y(visual.yaw_degrees.to_radians()),
                scale: Vec3::splat(visual.uniform_scale),
            },
            Visibility::Inherited,
            ChildOf(actor),
            CharacterVisual,
            Name::new(format!(
                "Character visual ({})",
                presentation_ref.profile_id
            )),
        ));
    }
}

fn prepare_character_animation_graphs(
    catalog: Res<CharacterCatalogResource>,
    mut assets: ResMut<CharacterAssetCache>,
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    let pending = assets
        .movement_sets
        .iter()
        .filter(|(_, cached)| cached.prepared.is_none() && !cached.failed)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();

    for set_id in pending {
        let set = catalog
            .0
            .movement_set(&set_id)
            .expect("requested movement sets come from the validated catalog");
        let bank_id = assets.movement_sets[&set_id].bank_id.clone();
        let source = assets.animation_banks[&bank_id].source.clone();
        let Some(gltf) = gltfs.get(&source) else {
            continue;
        };

        let clip_definitions = [
            catalog
                .0
                .clip(&set.idle_clip)
                .expect("validated Idle clip must exist"),
            catalog
                .0
                .clip(&set.walk.clip)
                .expect("validated Walk clip must exist"),
            catalog
                .0
                .clip(&set.jog.clip)
                .expect("validated Jog clip must exist"),
        ];
        let mut resolved_clips = Vec::with_capacity(clip_definitions.len());
        for definition in clip_definitions {
            let Some(handle) = gltf
                .named_animations
                .get(definition.animation.as_str())
                .cloned()
            else {
                error!(
                    "animation bank {bank_id:?} has no GLTF animation {:?} for stable clip {:?}",
                    definition.animation, definition.id
                );
                assets
                    .movement_sets
                    .get_mut(&set_id)
                    .expect("pending movement set must remain cached")
                    .failed = true;
                resolved_clips.clear();
                break;
            };
            resolved_clips.push(handle);
        }
        if resolved_clips.len() != 3 {
            continue;
        }

        let clips = CharacterAnimationClips {
            idle: resolved_clips[0].clone(),
            walk: resolved_clips[1].clone(),
            jog: resolved_clips[2].clone(),
        };
        let (graph, node_indices) = AnimationGraph::from_clips(resolved_clips);
        let prepared = PreparedMovementAnimations {
            graph: graphs.add(graph),
            nodes: CharacterAnimationNodes {
                idle: node_indices[0],
                walk: node_indices[1],
                jog: node_indices[2],
            },
            clips,
            blends: CharacterBlendTimings {
                start_seconds: set.tuning.start_blend_seconds,
                stop_seconds: set.tuning.stop_blend_seconds,
                gait_seconds: set.tuning.gait_blend_seconds,
            },
        };
        assets
            .movement_sets
            .get_mut(&set_id)
            .expect("pending movement set must remain cached")
            .prepared = Some(prepared);
    }
}

fn attach_character_animators(
    mut commands: Commands,
    assets: Res<CharacterAssetCache>,
    parents: Query<&ChildOf>,
    actor_roots: Query<(), (With<CharacterMotion>, With<ResolvedCharacterPresentation>)>,
    presentations: Query<&ResolvedCharacterPresentation>,
    mut players: Query<(Entity, &mut AnimationPlayer), Without<CharacterAnimator>>,
) {
    for (entity, mut player) in &mut players {
        let Some(actor) = find_actor_root(entity, &parents, &actor_roots) else {
            continue;
        };
        let presentation = presentations
            .get(actor)
            .expect("resolved actor root was found above");
        let Some(animations) = assets.prepared_movement_set(&presentation.movement_set_id) else {
            continue;
        };

        let mut transitions = AnimationTransitions::new();
        transitions
            .play(&mut player, animations.nodes.idle, Duration::ZERO)
            .repeat();
        commands.entity(entity).insert((
            AnimationGraphHandle(animations.graph.clone()),
            transitions,
            CharacterAnimator {
                actor,
                movement_set_id: presentation.movement_set_id.clone(),
                visual: LocomotionVisual::Idle,
            },
        ));
        commands.entity(actor).insert(CharacterPresentationReady);
    }
}

fn drive_character_animation(
    assets: Res<CharacterAssetCache>,
    animation_clips: Res<Assets<AnimationClip>>,
    motions: Query<&CharacterMotion>,
    mut players: Query<(
        &mut AnimationPlayer,
        &mut AnimationTransitions,
        &mut CharacterAnimator,
    )>,
) {
    for (mut player, mut transitions, mut animator) in &mut players {
        let Ok(motion) = motions.get(animator.actor) else {
            continue;
        };
        let Some(animations) = assets.prepared_movement_set(&animator.movement_set_id) else {
            continue;
        };
        let desired = locomotion_visual(motion);
        if desired != animator.visual {
            let phase = locomotion_phase(
                animator.visual,
                desired,
                animations.nodes,
                &animations.clips,
                &animation_clips,
                &player,
            );
            let blend_seconds = transition_seconds(animator.visual, desired, animations.blends);
            let target_duration = clip_duration(desired, &animations.clips, &animation_clips);
            let active = transitions.play(
                &mut player,
                animations.nodes.get(desired),
                Duration::from_secs_f32(blend_seconds),
            );
            active
                .set_speed(motion.playback_rate)
                .set_seek_time(phase * target_duration)
                .repeat();
            animator.visual = desired;
        } else if let Some(active) = player.animation_mut(animations.nodes.get(animator.visual)) {
            active.set_speed(motion.playback_rate);
        }
    }
}

fn locomotion_visual(motion: &CharacterMotion) -> LocomotionVisual {
    if motion.phase == CharacterMotionPhase::Idle
        && motion.displacement.length_squared() <= f32::EPSILON
    {
        LocomotionVisual::Idle
    } else {
        match motion.gait {
            CharacterGait::Walk => LocomotionVisual::Walk,
            CharacterGait::Jog => LocomotionVisual::Jog,
        }
    }
}

fn locomotion_phase(
    source: LocomotionVisual,
    target: LocomotionVisual,
    nodes: CharacterAnimationNodes,
    clips: &CharacterAnimationClips,
    animation_clips: &Assets<AnimationClip>,
    player: &AnimationPlayer,
) -> f32 {
    if !matches!(source, LocomotionVisual::Walk | LocomotionVisual::Jog)
        || !matches!(target, LocomotionVisual::Walk | LocomotionVisual::Jog)
    {
        return 0.0;
    }
    let duration = clip_duration(source, clips, animation_clips);
    player.animation(nodes.get(source)).map_or(0.0, |active| {
        (active.seek_time() / duration).rem_euclid(1.0)
    })
}

fn clip_duration(
    visual: LocomotionVisual,
    clips: &CharacterAnimationClips,
    animation_clips: &Assets<AnimationClip>,
) -> f32 {
    animation_clips
        .get(clips.get(visual))
        .map_or(1.0, |clip| clip.duration().max(f32::EPSILON))
}

fn transition_seconds(
    source: LocomotionVisual,
    target: LocomotionVisual,
    blends: CharacterBlendTimings,
) -> f32 {
    match (source, target) {
        (_, LocomotionVisual::Idle) => blends.stop_seconds,
        (LocomotionVisual::Idle, _) => blends.start_seconds,
        _ => blends.gait_seconds,
    }
}

fn find_actor_root(
    mut entity: Entity,
    parents: &Query<&ChildOf>,
    actor_roots: &Query<(), (With<CharacterMotion>, With<ResolvedCharacterPresentation>)>,
) -> Option<Entity> {
    loop {
        if actor_roots.contains(entity) {
            return Some(entity);
        }
        entity = parents.get(entity).ok()?.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BLENDS: CharacterBlendTimings = CharacterBlendTimings {
        start_seconds: 0.22,
        stop_seconds: 0.26,
        gait_seconds: 0.24,
    };

    #[test]
    fn only_idle_motion_uses_idle_presentation() {
        let mut motion = CharacterMotion::default();
        assert_eq!(locomotion_visual(&motion), LocomotionVisual::Idle);

        motion.phase = CharacterMotionPhase::Releasing;
        motion.gait = CharacterGait::Walk;
        assert_eq!(locomotion_visual(&motion), LocomotionVisual::Walk);

        motion.gait = CharacterGait::Jog;
        assert_eq!(locomotion_visual(&motion), LocomotionVisual::Jog);
    }

    #[test]
    fn gait_changes_preserve_phase_but_idle_transitions_restart() {
        assert_eq!(
            transition_seconds(LocomotionVisual::Walk, LocomotionVisual::Jog, TEST_BLENDS),
            TEST_BLENDS.gait_seconds
        );
        assert_eq!(
            transition_seconds(LocomotionVisual::Jog, LocomotionVisual::Idle, TEST_BLENDS),
            TEST_BLENDS.stop_seconds
        );
        assert_eq!(
            transition_seconds(LocomotionVisual::Idle, LocomotionVisual::Walk, TEST_BLENDS),
            TEST_BLENDS.start_seconds
        );
    }

    #[test]
    fn presentation_reference_is_independent_from_actor_roles() {
        let mut world = World::new();
        let actor = world
            .spawn(CharacterPresentationRef::new(
                "presentations/female/default",
            ))
            .id();

        assert!(world.get::<CharacterPresentationRef>(actor).is_some());
        assert!(world.get::<crate::actor::CameraTarget>(actor).is_none());
        assert!(world.get::<crate::actor::WorldStreamFocus>(actor).is_none());
    }
}
