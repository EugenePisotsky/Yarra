//! Opt-in demo-world traversal/residency regression. Never installed by world streaming.
use super::{
    ActiveWorldSpace, SourceResidency, StreamedPageEntity, StreamingStats, WorldOrigin,
    WorldStream, WorldStreamingSystems, WorldViewpoint,
};
use crate::{
    GameplaySystems, actor::WorldStreamFocus, character::CharacterPresentationReady,
    object_lod::ScreenSpaceLod,
};
use bevy::prelude::*;
use std::collections::BTreeSet;
use world::{CellCoord, PageKey, WorldPosition, WorldSpaceId};

/// Scripted diagnostic for the cooked demo: traverses five cells, enters the second world,
/// checks page/entity ownership and gameplay activation, then exits the application.
/// Requires game streaming and gameplay; omit this plugin for ordinary launches.
pub struct StreamingSmokePlugin;
impl Plugin for StreamingSmokePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StreamingSmokeState>().add_systems(
            Update,
            report_streaming_smoke
                .after(WorldStreamingSystems)
                .after(GameplaySystems::CameraFollow),
        );
    }
}

fn report_streaming_smoke(
    stats: Res<StreamingStats>,
    time: Res<Time>,
    stream: Res<WorldStream>,
    residency: Res<SourceResidency>,
    origin: Res<WorldOrigin>,
    viewpoint: Res<WorldViewpoint>,
    asset_server: Res<AssetServer>,
    mut active_space: ResMut<ActiveWorldSpace>,
    mut object: Single<&mut Transform, With<WorldStreamFocus>>,
    character: Query<(), (With<WorldStreamFocus>, With<CharacterPresentationReady>)>,
    streamed_entities: Query<&StreamedPageEntity>,
    lod_objects: Query<&ScreenSpaceLod>,
    mut smoke: ResMut<StreamingSmokeState>,
    mut app_exit: MessageWriter<AppExit>,
) {
    if matches!(*smoke, StreamingSmokeState::Initial) && time.elapsed_secs() >= 3.0 {
        assert_streaming_is_healthy(&stats);
        assert!(
            !character.is_empty(),
            "the controlled character scene or Idle animation did not become ready"
        );
        assert!(
            !lod_objects.is_empty(),
            "the smoke-test camera did not stream any LOD object"
        );
        for lod_object in &lod_objects {
            for variant in lod_object.variants() {
                assert!(
                    asset_server.is_loaded_with_dependencies(&variant.scene),
                    "LOD{} and its dependencies did not finish loading",
                    variant.lod
                );
            }
        }
        assert_eq!(
            stats.gameplay_objects, 0,
            "distant gameplay objects were activated in the overworld"
        );
        assert_eq!(
            stats.cached_definitions, 0,
            "the distant interior definition was fetched before entering its proximity set"
        );
        println!(
            "YARRA_STREAMING_SMOKE initial status={:?} demanded={} resident={} failed={} \
             owned_entities={} decoded_bytes={} gpu_bytes_estimate={} lods={:?}",
            stats.status,
            stats.demanded,
            stats.resident,
            stats.failed,
            stats.owned_entities,
            stats.decoded_bytes,
            stats.gpu_bytes_estimate,
            stats.lod_counts,
        );
        let cell_size = active_space
            .current
            .and_then(|id| stream.manifest.as_ref()?.world_space(id))
            .map(|space| space.cell_size)
            .expect("smoke test has no active world-space record");
        object.translation.x += 5.0 * cell_size;
        let origin_xz = origin.cell().origin(cell_size);
        *smoke = StreamingSmokeState::Traversal {
            expected_space: active_space
                .current
                .expect("smoke test has no active world"),
            expected_cell: CellCoord::containing(
                origin_xz[0] + f64::from(object.translation.x),
                origin_xz[1] + f64::from(object.translation.z),
                cell_size,
            ),
        };
        return;
    }
    // Leave a small integration-frame margin beyond the two-second cooling
    // deadline. Stats are sampled after bounded page attachment/removal and
    // can otherwise report the just-expired cooling set for one final frame.
    if let StreamingSmokeState::Traversal {
        expected_space,
        expected_cell,
    } = *smoke
        && time.elapsed_secs() >= 7.5
    {
        assert_streaming_is_healthy(&stats);
        assert_eq!(
            stats.cooling, 0,
            "old pages remained in cooling after the removal deadline"
        );
        assert_eq!(
            streamed_entities.iter().count(),
            stats.owned_entities,
            "tracked page ownership does not match live streamed root entities"
        );
        let current_space = active_space
            .current
            .expect("smoke test has no active world space");
        let manifest = stream
            .manifest
            .as_ref()
            .expect("smoke test has no runtime manifest");
        assert_traversal_sources(
            viewpoint
                .position()
                .expect("smoke test has no world viewpoint"),
            expected_space,
            expected_cell,
            &residency.desired,
            streamed_entities.iter().map(|entity| entity.0),
        );
        println!(
            "YARRA_STREAMING_SMOKE traversal passed demanded={} resident={} cooling={} \
             owned_entities={} failed={}",
            stats.demanded, stats.resident, stats.cooling, stats.owned_entities, stats.failed,
        );
        let next_space = manifest
            .world_spaces
            .iter()
            .find(|space| space.id != current_space)
            .expect("multi-world smoke test requires a second world space")
            .id;
        active_space.request(next_space, [0.0, 0.0, 0.0]);
        *smoke = StreamingSmokeState::Transition {
            expected_space: next_space,
        };
        return;
    }
    if let StreamingSmokeState::Transition { expected_space } = *smoke
        && time.elapsed_secs() >= 11.0
    {
        assert_streaming_is_healthy(&stats);
        assert_eq!(
            active_space.current,
            Some(expected_space),
            "world-space transition did not activate its destination"
        );
        assert_eq!(
            stats.cooling, 0,
            "old world-space pages remained in the cooling set"
        );
        assert_eq!(
            stats.gameplay_objects, 1,
            "the nearby interior gameplay object was not activated"
        );
        assert_eq!(
            stats.cached_definitions, 1,
            "the nearby gameplay definition was not fetched exactly once"
        );
        assert_eq!(
            streamed_entities.iter().count(),
            stats.owned_entities,
            "tracked page ownership does not match live streamed root entities"
        );
        for entity in &streamed_entities {
            assert_eq!(
                entity.0.space, expected_space,
                "an entity from the previous world space survived the transition"
            );
        }
        println!(
            "YARRA_STREAMING_SMOKE multi-world passed active_space={} demanded={} resident={} \
             cooling={} owned_entities={} gameplay_objects={} definitions_cached={} failed={}",
            expected_space.0,
            stats.demanded,
            stats.resident,
            stats.cooling,
            stats.owned_entities,
            stats.gameplay_objects,
            stats.cached_definitions,
            stats.failed,
        );
        *smoke = StreamingSmokeState::Complete;
        app_exit.write(AppExit::Success);
    }
}

#[derive(Resource, Default, Clone, Copy)]
enum StreamingSmokeState {
    #[default]
    Initial,
    Traversal {
        expected_space: WorldSpaceId,
        expected_cell: CellCoord,
    },
    Transition {
        expected_space: WorldSpaceId,
    },
    Complete,
}

fn assert_traversal_sources(
    position: WorldPosition,
    expected_space: WorldSpaceId,
    expected_cell: CellCoord,
    desired: &BTreeSet<PageKey>,
    pages: impl IntoIterator<Item = PageKey>,
) {
    assert_eq!(
        position.space, expected_space,
        "traversal changed world space"
    );
    assert_eq!(
        position.cell, expected_cell,
        "traversal did not reach its canonical destination"
    );
    for key in pages {
        assert_eq!(
            key.space, expected_space,
            "an entity from another world survived traversal"
        );
        assert!(
            desired.contains(&key),
            "an obsolete page {key:?} survived traversal and cooling"
        );
    }
}

fn assert_streaming_is_healthy(stats: &StreamingStats) {
    assert!(
        !stats.status.starts_with("failed:"),
        "runtime database worker failed: {}",
        stats.status
    );
    assert!(stats.demanded > 0, "streaming produced no demanded pages");
    assert!(stats.resident > 0, "streaming produced no resident pages");
    assert_eq!(stats.failed, 0, "one or more streamed pages failed");
}

#[cfg(test)]
mod tests;
