//! The active world space, requested moves between spaces, and the space's authored
//! atmosphere.
use super::*;

#[derive(Resource, Debug, Default)]
pub struct ActiveWorldSpace {
    pub(super) current: Option<WorldSpaceId>,
    pub(super) requested: Option<WorldSpaceTransition>,
    pub(super) transition_error: Option<String>,
}

impl ActiveWorldSpace {
    pub fn current(&self) -> Option<WorldSpaceId> {
        self.current
    }

    pub fn request(&mut self, space: WorldSpaceId, local_position: [f32; 3]) {
        self.transition_error = None;
        self.requested = Some(WorldSpaceTransition {
            space,
            local_position,
        });
    }

    pub fn transition_error(&self) -> Option<&str> {
        self.transition_error.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WorldSpaceTransition {
    pub(super) space: WorldSpaceId,
    pub(super) local_position: [f32; 3],
}

pub(super) fn request_world_space_from_keyboard(
    debug: Res<WorldDebugControls>,
    keys: Res<ButtonInput<KeyCode>>,
    config: Res<WorldStreamingConfig>,
    stream: Res<WorldStream>,
    mut active_space: ResMut<ActiveWorldSpace>,
) {
    if !config.keyboard_world_space_cycle || !debug.world_switch || !keys.just_pressed(KeyCode::Tab)
    {
        return;
    }
    let Some(manifest) = stream.manifest.as_ref() else {
        return;
    };
    let Some(current) = active_space.current else {
        return;
    };
    let Some(current_index) = manifest
        .world_spaces
        .iter()
        .position(|space| space.id == current)
    else {
        return;
    };
    if manifest.world_spaces.len() < 2 {
        return;
    }
    let next = &manifest.world_spaces[(current_index + 1) % manifest.world_spaces.len()];
    active_space.request(next.id, [0.0, 0.0, 0.0]);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_world_space_transition(
    mut commands: Commands,
    mut terrain_meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut terrain_images: ResMut<Assets<Image>>,
    mut active_space: ResMut<ActiveWorldSpace>,
    config: Res<WorldStreamingConfig>,
    mut viewpoint: ResMut<WorldViewpoint>,
    mut origin: ResMut<WorldOrigin>,
    mut stream: ResMut<WorldStream>,
    mut residency: ResMut<SourceResidency>,
    mut focuses: Query<
        (
            &mut Transform,
            &mut MoveIntent,
            &mut CharacterMotor,
            &mut CharacterMotion,
        ),
        With<WorldStreamFocus>,
    >,
    mut entry: ResMut<terrain_lod::entry::TerrainEntry>,
    mut terrain: ResMut<terrain_lod::TerrainLodStream>,
    tracker: Res<terrain_lod::UploadTracker>,
    lod_config: Res<TerrainHierarchy>,
    reload: Res<WorldGenerationReload>,
) {
    if reload.active() {
        return;
    }
    if lod_config.enabled
        && active_space
            .requested
            .is_some_and(|t| Some(t.space) != active_space.current)
        && !active_space.requested.is_some_and(|t| {
            stream
                .manifest
                .as_ref()
                .is_some_and(|m| entry.ready_for(&m.generation_id, t))
        })
    {
        return;
    }
    let Some(transition) = active_space.requested.take() else {
        return;
    };
    if !transition.local_position.iter().all(|v| v.is_finite()) {
        active_space.transition_error = Some("destination coordinates must be finite".into());
        return;
    }
    let Some(space) = stream
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.world_space(transition.space))
        .cloned()
    else {
        error!(
            "ignored transition to unknown world space {:?}",
            transition.space
        );
        return;
    };

    let changed_space = active_space.current != Some(transition.space);
    if changed_space {
        clear_streamed_pages(
            &mut commands,
            &mut terrain_meshes,
            &mut terrain_materials,
            &mut terrain_images,
            &mut stream,
            &mut residency,
        );
    }

    active_space.current = Some(transition.space);
    let position = WorldPosition::from_world(
        transition.space,
        [
            f64::from(transition.local_position[0]),
            f64::from(transition.local_position[1]),
            f64::from(transition.local_position[2]),
        ],
        space.cell_size,
    );
    viewpoint.set(position);
    if changed_space {
        origin.space = Some(transition.space);
        origin.cell = if rebase::effective_config(*config, lod_config.enabled)
            .floating_origin_threshold_cells
            .is_some()
        {
            position.cell
        } else {
            CellCoord::ZERO
        };
    }
    if changed_space && lod_config.enabled {
        entry.commit(
            &mut terrain,
            &mut commands,
            &mut terrain_meshes,
            &tracker,
            origin.cell,
            space.cell_size,
        );
    }
    for (mut transform, mut intent, mut motor, mut motion) in &mut focuses {
        transform.translation =
            Vec3::from_array(position.relative_to(origin.cell, space.cell_size));
        intent.clear();
        motor.reset();
        *motion = CharacterMotion::default();
    }
    info!(
        "entered world space {} ({:?}) at {:?}",
        space.name, transition.space, transition.local_position
    );
}

#[cfg(test)]
impl ActiveWorldSpace {
    pub(crate) fn current_for_tests(space: WorldSpaceId) -> Self {
        Self {
            current: Some(space),
            ..default()
        }
    }
}

pub(crate) fn sync_world_atmosphere(
    catalog: Res<WorldCatalog>,
    active: Res<ActiveWorldSpace>,
    mut atmosphere: Option<ResMut<crate::AtmosphereState>>,
    mut initialized: Local<bool>,
) {
    let Some(state) = atmosphere.as_mut() else {
        return;
    };
    if state.owner != crate::AtmosphereOwner::Game {
        return;
    }
    let Some(space) = active.current.and_then(|id| catalog.world_space(id)) else {
        return;
    };
    if !*initialized {
        state.phase = space.atmosphere.initial_phase;
        *initialized = true;
    }
    if state.profile != space.atmosphere {
        state.profile = space.atmosphere.clone();
    }
}

#[cfg(test)]
mod atmosphere_tests {
    use super::*;
    #[test]
    fn world_changes_and_publication_keep_time_and_editor_ownership() {
        let mut app = App::new();
        let first = world::atmosphere::AtmosphereProfile::default();
        let second = world::atmosphere::AtmosphereProfile {
            initial_phase: 0.1,
            outdoor: false,
            ..first.clone()
        };
        app.insert_resource(WorldCatalog {
            world_spaces: vec![
                WorldSpaceInfo {
                    id: WorldSpaceId(1),
                    name: "outdoor".into(),
                    cell_size: 32.0,
                    minimum_y: 0.0,
                    maximum_y: 1.0,
                    sea_level: None,
                    atmosphere: first.clone(),
                },
                WorldSpaceInfo {
                    id: WorldSpaceId(2),
                    name: "inside".into(),
                    cell_size: 32.0,
                    minimum_y: 0.0,
                    maximum_y: 1.0,
                    sea_level: None,
                    atmosphere: second,
                },
            ],
            ..default()
        })
        .init_resource::<ActiveWorldSpace>()
        .init_resource::<crate::AtmosphereState>()
        .add_systems(Update, sync_world_atmosphere);
        app.world_mut().resource_mut::<ActiveWorldSpace>().current = Some(WorldSpaceId(1));
        app.update();
        assert_eq!(
            app.world().resource::<crate::AtmosphereState>().phase,
            first.initial_phase
        );
        app.world_mut()
            .resource_mut::<crate::AtmosphereState>()
            .phase = 0.7;
        app.world_mut().resource_mut::<ActiveWorldSpace>().current = Some(WorldSpaceId(2));
        app.update();
        let state = app.world().resource::<crate::AtmosphereState>();
        assert_eq!(state.phase, 0.7);
        assert!(!state.profile.outdoor);
        app.world_mut()
            .resource_mut::<crate::AtmosphereState>()
            .owner = crate::AtmosphereOwner::Editor;
        app.world_mut().resource_mut::<ActiveWorldSpace>().current = Some(WorldSpaceId(1));
        app.update();
        assert!(
            !app.world()
                .resource::<crate::AtmosphereState>()
                .profile
                .outdoor
        );
    }
}
