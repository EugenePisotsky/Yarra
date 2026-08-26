//! Checked-in semantic character and animation definitions.
//!
//! Pack manifests own provenance and licensing. This catalog owns only the
//! stable runtime composition between models, skeletons, animation clips, and
//! movement sets.

use std::collections::HashSet;

use serde::Deserialize;

use crate::actor::CharacterMotorConfig;

pub(crate) const DEFAULT_CHARACTER_PRESENTATION_ID: &str = "presentations/female/default";

const CHARACTER_PRESENTATION_CATALOG_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/catalogs/character_presentations.catalog.ron"
));

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct SkeletonDefinition {
    pub(crate) id: String,
    name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CharacterVisualDefinition {
    pub(crate) asset: String,
    pub(crate) scene: usize,
    pub(crate) translation_m: [f32; 3],
    pub(crate) yaw_degrees: f32,
    pub(crate) uniform_scale: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CharacterModelDefinition {
    pub(crate) id: String,
    name: String,
    pub(crate) skeleton: String,
    pub(crate) visual: CharacterVisualDefinition,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AnimationBankDefinition {
    pub(crate) id: String,
    name: String,
    pub(crate) skeleton: String,
    pub(crate) asset: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub(crate) enum ClipPlayback {
    Loop,
    Once,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AnimationClipDefinition {
    pub(crate) id: String,
    name: String,
    pub(crate) bank: String,
    pub(crate) animation: String,
    playback: ClipPlayback,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MovementGaitDefinition {
    pub(crate) clip: String,
    pub(crate) speed_mps: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct MovementTuningDefinition {
    pub(crate) input_dead_zone: f32,
    pub(crate) input_response_exponent: f32,
    pub(crate) walk_to_jog_threshold: f32,
    pub(crate) jog_to_walk_threshold: f32,
    pub(crate) walk_acceleration_seconds: f32,
    pub(crate) jog_acceleration_seconds: f32,
    pub(crate) release_seconds: f32,
    pub(crate) gait_blend_seconds: f32,
    pub(crate) arrival_radius_m: f32,
    pub(crate) start_blend_seconds: f32,
    pub(crate) stop_blend_seconds: f32,
    pub(crate) walk_turn_rate_radians: f32,
    pub(crate) jog_turn_rate_radians: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MovementSetDefinition {
    pub(crate) id: String,
    name: String,
    pub(crate) skeleton: String,
    pub(crate) idle_clip: String,
    pub(crate) walk: MovementGaitDefinition,
    pub(crate) jog: MovementGaitDefinition,
    pub(crate) tuning: MovementTuningDefinition,
}

impl MovementSetDefinition {
    pub(crate) fn motor_config(&self) -> CharacterMotorConfig {
        CharacterMotorConfig {
            walk_speed_mps: self.walk.speed_mps,
            jog_speed_mps: self.jog.speed_mps,
            input_dead_zone: self.tuning.input_dead_zone,
            input_response_exponent: self.tuning.input_response_exponent,
            walk_to_jog_threshold: self.tuning.walk_to_jog_threshold,
            jog_to_walk_threshold: self.tuning.jog_to_walk_threshold,
            walk_acceleration_seconds: self.tuning.walk_acceleration_seconds,
            jog_acceleration_seconds: self.tuning.jog_acceleration_seconds,
            release_seconds: self.tuning.release_seconds,
            gait_blend_seconds: self.tuning.gait_blend_seconds,
            arrival_radius_m: self.tuning.arrival_radius_m,
            walk_turn_rate_radians: self.tuning.walk_turn_rate_radians,
            jog_turn_rate_radians: self.tuning.jog_turn_rate_radians,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct MovementSetBindingDefinition {
    context: String,
    movement_set: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CharacterPresentationProfileDefinition {
    pub(crate) id: String,
    name: String,
    pub(crate) model: String,
    default_movement_context: String,
    movement_sets: Vec<MovementSetBindingDefinition>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CharacterPresentationCatalog {
    schema_version: u32,
    skeletons: Vec<SkeletonDefinition>,
    models: Vec<CharacterModelDefinition>,
    animation_banks: Vec<AnimationBankDefinition>,
    clips: Vec<AnimationClipDefinition>,
    movement_sets: Vec<MovementSetDefinition>,
    profiles: Vec<CharacterPresentationProfileDefinition>,
}

pub(crate) struct ResolvedCharacterPresentationDefinition<'a> {
    pub(crate) model: &'a CharacterModelDefinition,
    pub(crate) movement_set: &'a MovementSetDefinition,
    pub(crate) animation_bank: &'a AnimationBankDefinition,
}

impl CharacterPresentationCatalog {
    pub(crate) fn load() -> Result<Self, String> {
        let catalog =
            ron::de::from_str::<Self>(CHARACTER_PRESENTATION_CATALOG_SOURCE).map_err(|error| {
                format!("character presentation catalog could not be parsed: {error}")
            })?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub(crate) fn resolve_default(
        &self,
        profile_id: &str,
    ) -> Result<ResolvedCharacterPresentationDefinition<'_>, String> {
        let profile = self
            .profile(profile_id)
            .ok_or_else(|| format!("unknown character presentation profile {profile_id:?}"))?;
        let binding = profile
            .movement_sets
            .iter()
            .find(|binding| binding.context == profile.default_movement_context)
            .expect("validated presentation default movement context must exist");
        let model = self
            .model(&profile.model)
            .expect("validated presentation model must exist");
        let movement_set = self
            .movement_set(&binding.movement_set)
            .expect("validated presentation movement set must exist");
        let animation_bank = self
            .animation_bank_for_set(movement_set)
            .expect("validated movement set animation bank must exist");
        Ok(ResolvedCharacterPresentationDefinition {
            model,
            movement_set,
            animation_bank,
        })
    }

    pub(crate) fn movement_set(&self, id: &str) -> Option<&MovementSetDefinition> {
        self.movement_sets.iter().find(|set| set.id == id)
    }

    pub(crate) fn clip(&self, id: &str) -> Option<&AnimationClipDefinition> {
        self.clips.iter().find(|clip| clip.id == id)
    }

    fn model(&self, id: &str) -> Option<&CharacterModelDefinition> {
        self.models.iter().find(|model| model.id == id)
    }

    fn bank(&self, id: &str) -> Option<&AnimationBankDefinition> {
        self.animation_banks.iter().find(|bank| bank.id == id)
    }

    fn profile(&self, id: &str) -> Option<&CharacterPresentationProfileDefinition> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    fn animation_bank_for_set(
        &self,
        set: &MovementSetDefinition,
    ) -> Option<&AnimationBankDefinition> {
        let bank_id = &self.clip(&set.idle_clip)?.bank;
        self.bank(bank_id)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "unsupported character presentation catalog schema {}",
                self.schema_version
            ));
        }
        if self.skeletons.is_empty()
            || self.models.is_empty()
            || self.animation_banks.is_empty()
            || self.clips.is_empty()
            || self.movement_sets.is_empty()
            || self.profiles.is_empty()
        {
            return Err("character presentation catalog contains an empty definition group".into());
        }

        validate_unique_ids(
            "skeleton",
            self.skeletons
                .iter()
                .map(|definition| definition.id.as_str()),
        )?;
        validate_unique_ids(
            "model",
            self.models.iter().map(|definition| definition.id.as_str()),
        )?;
        validate_unique_ids(
            "animation bank",
            self.animation_banks
                .iter()
                .map(|definition| definition.id.as_str()),
        )?;
        validate_unique_ids(
            "animation clip",
            self.clips.iter().map(|definition| definition.id.as_str()),
        )?;
        validate_unique_ids(
            "movement set",
            self.movement_sets
                .iter()
                .map(|definition| definition.id.as_str()),
        )?;
        validate_unique_ids(
            "presentation profile",
            self.profiles
                .iter()
                .map(|definition| definition.id.as_str()),
        )?;

        let skeleton_ids = self
            .skeletons
            .iter()
            .map(|definition| definition.id.as_str())
            .collect::<HashSet<_>>();
        for skeleton in &self.skeletons {
            validate_name("skeleton", &skeleton.id, &skeleton.name)?;
        }
        for model in &self.models {
            validate_name("model", &model.id, &model.name)?;
            if !skeleton_ids.contains(model.skeleton.as_str()) {
                return Err(format!(
                    "model {:?} references unknown skeleton {:?}",
                    model.id, model.skeleton
                ));
            }
            validate_asset_path("model", &model.id, &model.visual.asset)?;
            if model
                .visual
                .translation_m
                .iter()
                .any(|value| !value.is_finite())
                || !model.visual.yaw_degrees.is_finite()
                || !model.visual.uniform_scale.is_finite()
                || model.visual.uniform_scale <= 0.0
            {
                return Err(format!(
                    "model {:?} has an invalid visual transform",
                    model.id
                ));
            }
        }
        for bank in &self.animation_banks {
            validate_name("animation bank", &bank.id, &bank.name)?;
            if !skeleton_ids.contains(bank.skeleton.as_str()) {
                return Err(format!(
                    "animation bank {:?} references unknown skeleton {:?}",
                    bank.id, bank.skeleton
                ));
            }
            validate_asset_path("animation bank", &bank.id, &bank.asset)?;
        }
        for clip in &self.clips {
            validate_name("animation clip", &clip.id, &clip.name)?;
            if self.bank(&clip.bank).is_none() {
                return Err(format!(
                    "animation clip {:?} references unknown bank {:?}",
                    clip.id, clip.bank
                ));
            }
            if clip.animation.trim().is_empty() {
                return Err(format!(
                    "animation clip {:?} has an empty GLTF name",
                    clip.id
                ));
            }
        }
        for set in &self.movement_sets {
            validate_name("movement set", &set.id, &set.name)?;
            if !skeleton_ids.contains(set.skeleton.as_str()) {
                return Err(format!(
                    "movement set {:?} references unknown skeleton {:?}",
                    set.id, set.skeleton
                ));
            }
            self.validate_movement_set_clips(set)?;
            validate_movement_tuning(set)?;
        }
        for profile in &self.profiles {
            validate_name("presentation profile", &profile.id, &profile.name)?;
            let Some(model) = self.model(&profile.model) else {
                return Err(format!(
                    "presentation profile {:?} references unknown model {:?}",
                    profile.id, profile.model
                ));
            };
            if profile.movement_sets.is_empty() {
                return Err(format!(
                    "presentation profile {:?} has no movement-set bindings",
                    profile.id
                ));
            }
            validate_semantic_id("movement context", &profile.default_movement_context)?;
            let mut contexts = HashSet::new();
            for binding in &profile.movement_sets {
                validate_semantic_id("movement context", &binding.context)?;
                if !contexts.insert(binding.context.as_str()) {
                    return Err(format!(
                        "presentation profile {:?} repeats movement context {:?}",
                        profile.id, binding.context
                    ));
                }
                let Some(set) = self.movement_set(&binding.movement_set) else {
                    return Err(format!(
                        "presentation profile {:?} references unknown movement set {:?}",
                        profile.id, binding.movement_set
                    ));
                };
                if model.skeleton != set.skeleton {
                    return Err(format!(
                        "presentation profile {:?} combines model {:?} and movement set {:?} with different skeletons",
                        profile.id, model.id, set.id
                    ));
                }
            }
            if !contexts.contains(profile.default_movement_context.as_str()) {
                return Err(format!(
                    "presentation profile {:?} has no binding for default movement context {:?}",
                    profile.id, profile.default_movement_context
                ));
            }
        }
        Ok(())
    }

    fn validate_movement_set_clips(&self, set: &MovementSetDefinition) -> Result<(), String> {
        let roles = [
            ("idle", set.idle_clip.as_str()),
            ("walk", set.walk.clip.as_str()),
            ("jog", set.jog.clip.as_str()),
        ];
        let mut expected_bank = None;
        for (role, clip_id) in roles {
            let Some(clip) = self.clip(clip_id) else {
                return Err(format!(
                    "movement set {:?} references unknown {role} clip {clip_id:?}",
                    set.id
                ));
            };
            if clip.playback != ClipPlayback::Loop {
                return Err(format!(
                    "movement set {:?} requires looping {role} clip {clip_id:?}",
                    set.id
                ));
            }
            let bank = self
                .bank(&clip.bank)
                .expect("animation clip banks were validated before movement sets");
            if bank.skeleton != set.skeleton {
                return Err(format!(
                    "movement set {:?} and {role} clip {clip_id:?} use different skeletons",
                    set.id
                ));
            }
            if let Some(expected_bank) = expected_bank {
                if expected_bank != bank.id {
                    return Err(format!(
                        "movement set {:?} mixes animation banks {:?} and {:?}",
                        set.id, expected_bank, bank.id
                    ));
                }
            } else {
                expected_bank = Some(bank.id.as_str());
            }
        }
        Ok(())
    }
}

fn validate_unique_ids<'a>(
    kind: &str,
    ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let mut unique = HashSet::new();
    for id in ids {
        validate_semantic_id(kind, id)?;
        if !unique.insert(id) {
            return Err(format!("duplicate {kind} ID {id:?}"));
        }
    }
    Ok(())
}

fn validate_semantic_id(kind: &str, id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.split('/').any(|segment| segment.is_empty())
        || !id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '/' | '_')
        })
    {
        return Err(format!("invalid {kind} ID {id:?}"));
    }
    Ok(())
}

fn validate_name(kind: &str, id: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!("{kind} {id:?} has an empty display name"));
    }
    Ok(())
}

fn validate_asset_path(kind: &str, id: &str, path: &str) -> Result<(), String> {
    if path.trim().is_empty() || path.starts_with('/') || path.contains("..") {
        return Err(format!("{kind} {id:?} has an invalid asset path {path:?}"));
    }
    Ok(())
}

fn validate_movement_tuning(set: &MovementSetDefinition) -> Result<(), String> {
    let tuning = set.tuning;
    if !set.walk.speed_mps.is_finite()
        || set.walk.speed_mps <= 0.0
        || !set.jog.speed_mps.is_finite()
        || set.jog.speed_mps <= set.walk.speed_mps
    {
        return Err(format!("movement set {:?} has invalid gait speeds", set.id));
    }
    if !tuning.input_dead_zone.is_finite()
        || !(0.0..1.0).contains(&tuning.input_dead_zone)
        || !tuning.input_response_exponent.is_finite()
        || tuning.input_response_exponent <= 0.0
        || !tuning.jog_to_walk_threshold.is_finite()
        || !tuning.walk_to_jog_threshold.is_finite()
        || !(0.0..tuning.walk_to_jog_threshold).contains(&tuning.jog_to_walk_threshold)
        || tuning.walk_to_jog_threshold >= 1.0
    {
        return Err(format!(
            "movement set {:?} has invalid input thresholds",
            set.id
        ));
    }
    let positive_durations = [
        tuning.walk_acceleration_seconds,
        tuning.jog_acceleration_seconds,
        tuning.release_seconds,
        tuning.gait_blend_seconds,
        tuning.start_blend_seconds,
        tuning.stop_blend_seconds,
    ];
    if positive_durations
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
        || !tuning.arrival_radius_m.is_finite()
        || tuning.arrival_radius_m <= 0.0
        || !tuning.walk_turn_rate_radians.is_finite()
        || tuning.walk_turn_rate_radians <= 0.0
        || !tuning.jog_turn_rate_radians.is_finite()
        || tuning.jog_turn_rate_radians <= 0.0
    {
        return Err(format!(
            "movement set {:?} has invalid timing or movement tuning",
            set.id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_catalog_resolves_default_profile() {
        let catalog = CharacterPresentationCatalog::load().expect("catalog should be valid");
        let resolved = catalog
            .resolve_default(DEFAULT_CHARACTER_PRESENTATION_ID)
            .expect("default profile should resolve");

        assert_eq!(resolved.model.id, "models/female_main");
        assert_eq!(resolved.movement_set.id, "movement/female_main/standing");
        assert_eq!(resolved.animation_bank.id, "banks/female_main/locomotion");
        assert_eq!(
            resolved.movement_set.motor_config().walk_speed_mps,
            1.968_607_5
        );
    }

    #[test]
    fn profile_rejects_incompatible_model_and_movement_skeletons() {
        let mut catalog = CharacterPresentationCatalog::load().expect("catalog should be valid");
        catalog.skeletons.push(SkeletonDefinition {
            id: "skeletons/other".into(),
            name: "Other".into(),
        });
        catalog.models[0].skeleton = "skeletons/other".into();

        let error = catalog
            .validate()
            .expect_err("skeleton mismatch should fail");
        assert!(error.contains("different skeletons"));
    }

    #[test]
    fn profile_rejects_duplicate_movement_contexts() {
        let mut catalog = CharacterPresentationCatalog::load().expect("catalog should be valid");
        let duplicate = catalog.profiles[0].movement_sets[0].clone();
        catalog.profiles[0].movement_sets.push(duplicate);

        let error = catalog
            .validate()
            .expect_err("duplicate context should fail");
        assert!(error.contains("repeats movement context"));
    }
}
