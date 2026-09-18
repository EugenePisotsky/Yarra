//! A separate, physically scaled road-authoring fixture. Ordinary demo/performance grids stay intact.
use super::*;
use environment::{ChannelId, roads::*};
use world_db::{RoadSourceRecord as R, SourceRoad, SourceRoadKnot, SourceRoadSpan};
pub fn create_road_demo_project(path: &Path) -> Result<()> {
    write_project_database(path, &document())
        .with_context(|| format!("failed to create road project at {}", path.display()))
}
pub(super) fn document() -> ProjectDocument {
    let mut project = demo_project_document();
    for world in &mut project.world_spaces {
        world.cell_size = 8.0;
        world.minimum_y *= 0.25;
        world.maximum_y *= 0.25;
    }
    for d in &mut project.environments {
        d.cell_size = 8.0;
    }
    for p in &mut project.terrain_profiles {
        p.weight_resolution = 65;
    }
    for h in &mut project.terrain_cell_heightfields {
        for y in &mut h.heights {
            *y *= 0.25;
        }
    }
    for o in &mut project.objects {
        for v in &mut o.local_translation {
            *v *= 0.25;
        }
    }
    let p = CartTrackProfile {
        id: RoadProfileId(stable_id("road-demo-cart-style")),
        revision: 1,
        name: "Worn meadow cart track".into(),
        ground: environment::fixtures::DRY_GROUND,
        vegetation_channel: ChannelId(stable_id("ground-grass-channel")),
        track_spacing: 1.55,
        track_width: 0.65,
        edge_softness: 0.15,
        center_ground: 0.0,
        center_retention: 1.0,
        shoulder_ground: 0.15,
        shoulder_retention: 0.55,
        track_retention: 0.0,
        edge_variation: 0.08,
        edge_patch_size: 2.0,
        breakup: 0.7,
        breakup_patch_size: 1.2,
        relief: Default::default(),
    };
    let road = Road {
        id: RoadId(stable_id("road-demo-route")),
        revision: 1,
        name: "Meadow cart road".into(),
        profile: p.id,
        seed: 481,
        enabled: true,
        order: 0,
        direction: TravelDirection::Bidirectional,
        travel_modes: vec![
            RoadTravelMode::Foot,
            RoadTravelMode::Mounted,
            RoadTravelMode::Cart,
        ],
    };
    let knot = |name: &str, x, z, incoming, outgoing| RoadKnot {
        id: RoadKnotId(stable_id(name)),
        revision: 1,
        position: RoadPoint::from_relative(CellCoord::ZERO, [x, z], 8.0).unwrap(),
        incoming,
        outgoing,
        width: 3.5,
    };
    let a = knot("road-demo-a", -18.0, -4.0, [0.0; 2], [12.0, 0.0]);
    let b = knot("road-demo-b", 18.0, 4.0, [-12.0, 0.0], [0.0; 2]);
    project.roads.records = vec![
        R::Profile(p),
        R::Road(SourceRoad {
            space: project.default_world_space,
            road: road.clone(),
        }),
        R::Knot(SourceRoadKnot {
            road: road.id,
            knot: a.clone(),
        }),
        R::Knot(SourceRoadKnot {
            road: road.id,
            knot: b.clone(),
        }),
        R::Span(SourceRoadSpan {
            id: RoadSpanId(stable_id("road-demo-span")),
            revision: 1,
            road: road.id,
            start: a.id,
            end: b.id,
        }),
    ];
    project
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn road_fixture_cooks_narrow_tracks_on_its_own_grid() {
        let project = document();
        let build = build_runtime(project).unwrap();
        assert!(!build.pages.is_empty());
    }
}
