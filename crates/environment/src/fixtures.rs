//! Reproducible starter treatments shared by the fresh demo and compiler acceptance fixture.
//! Meadows paint ground; grass grows on it as the world's grass catalog says.
use super::*;

pub const DRY_MEADOW: PresetId = PresetId([1; 16]);
pub const GREEN_MEADOW: PresetId = PresetId([2; 16]);
pub const CLEARING: PresetId = PresetId([3; 16]);
pub const DRY_GROUND: PresetId = PresetId([4; 16]);
pub const GREEN_GROUND: PresetId = PresetId([5; 16]);
pub const CLEAR_GRASS: PresetId = PresetId([8; 16]);
pub const GROUND_USE: PresetUseId = PresetUseId([1; 16]);
pub const CLEAR_USE: PresetUseId = PresetUseId([2; 16]);

pub fn meadow_library(dry: TerrainSurfaceId, green: TerrainSurfaceId) -> PresetLibrary {
    let preset = |id, name: &str, kind| Preset {
        id,
        revision: 1,
        name: name.into(),
        kind,
    };
    let ground = |surface| {
        PresetKind::Ground(GroundTreatment {
            id: OutputId([1; 16]),
            strength: 1.0,
            surfaces: vec![SurfaceWeight {
                surface,
                weight: 1.0,
            }],
        })
    };
    let child = |id, name: &str, preset| PresetUse {
        id,
        name: name.into(),
        preset,
        overrides: vec![],
    };
    PresetLibrary {
        revision: 1,
        presets: vec![
            preset(
                DRY_MEADOW,
                "Dry meadow",
                PresetKind::Composition(vec![child(GROUND_USE, "Ground", DRY_GROUND)]),
            ),
            preset(
                GREEN_MEADOW,
                "Green meadow",
                PresetKind::Composition(vec![child(GROUND_USE, "Ground", GREEN_GROUND)]),
            ),
            preset(
                CLEARING,
                "Clearing",
                PresetKind::Composition(vec![
                    child(GROUND_USE, "Ground", DRY_GROUND),
                    child(CLEAR_USE, "Clear grass", CLEAR_GRASS),
                ]),
            ),
            preset(DRY_GROUND, "Dry ground", ground(dry)),
            preset(GREEN_GROUND, "Green ground", ground(green)),
            preset(
                CLEAR_GRASS,
                "Clear grass",
                PresetKind::Exclusion(Exclusion {
                    id: OutputId([3; 16]),
                    channel: GRASS_CHANNEL,
                    strength: 1.0,
                }),
            ),
        ],
    }
}
