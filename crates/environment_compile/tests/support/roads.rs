//! Reused by acceptance tests and the offline road preview.
use super::support::*;
use environment::roads::*;
use environment::{CoverageSnapshot, EnvironmentDefinition, PresetLibrary};
use world::CellCoord;
pub fn cart_tracks() -> (
    EnvironmentDefinition,
    PresetLibrary,
    vegetation::VegetationCatalog,
    CoverageSnapshot,
    RoadSnapshot,
) {
    let (mut definition, library, plants, _) = fixture();
    definition.layers.retain(|l| l.id == GREEN);
    definition.base_surface = GREEN_SOIL;
    let coverage = snapshot(&definition, &CELLS, |_, _, _| 1.0);
    let profile = CartTrackProfile {
        id: RoadProfileId([1; 16]),
        revision: 1,
        name: "Worn meadow cart track".into(),
        ground: environment::fixtures::DRY_GROUND,
        vegetation_channel: GRASS,
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
        id: RoadId([1; 16]),
        revision: 1,
        name: "Meadow route".into(),
        profile: profile.id,
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
    let point = |x, z| RoadPoint::from_relative(CellCoord::ZERO, [x, z], 8.0).unwrap();
    let span = RoadSpan {
        id: RoadSpanId([1; 16]),
        revision: 1,
        road: road.id,
        start: RoadKnot {
            id: RoadKnotId([1; 16]),
            revision: 1,
            position: point(-2.0, 3.0),
            incoming: [0.0; 2],
            outgoing: [6.0, 4.0],
            width: 3.5,
        },
        end: RoadKnot {
            id: RoadKnotId([2; 16]),
            revision: 1,
            position: point(18.0, 5.0),
            incoming: [-6.0, -4.0],
            outgoing: [0.0; 2],
            width: 4.5,
        },
    };
    let roads = RoadSnapshot {
        space: definition.space,
        cell_size: definition.cell_size,
        loaded_bounds: RoadCellBounds {
            minimum: CellCoord { x: -1, z: -1 },
            maximum: CellCoord { x: 2, z: 1 },
        },
        truncated: false,
        junctions: vec![],
        roads: vec![road],
        profiles: vec![profile],
        spans: vec![span],
    };
    (definition, library, plants, coverage, roads)
}
pub fn straight(roads: &mut RoadSnapshot) {
    let s = &mut roads.spans[0];
    s.start.position = RoadPoint::from_relative(CellCoord::ZERO, [-2.0, 4.0], 8.0).unwrap();
    s.end.position = RoadPoint::from_relative(CellCoord::ZERO, [18.0, 4.0], 8.0).unwrap();
    s.start.outgoing = [20.0 / 3.0, 0.0];
    s.end.incoming = [-20.0 / 3.0, 0.0];
    s.start.width = 3.5;
    s.end.width = 3.5;
    roads.profiles[0].edge_variation = 0.0;
    roads.profiles[0].breakup = 0.0;
}
