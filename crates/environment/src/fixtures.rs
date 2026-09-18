//! Reproducible starter treatments shared by the fresh demo and compiler acceptance fixture.
use super::*;

pub const DRY_MEADOW: PresetId = PresetId([1; 16]);
pub const GREEN_MEADOW: PresetId = PresetId([2; 16]);
pub const CLEARING: PresetId = PresetId([3; 16]);
pub const DRY_GROUND: PresetId = PresetId([4; 16]);
pub const GREEN_GROUND: PresetId = PresetId([5; 16]);
pub const DRY_FOLIAGE: PresetId = PresetId([6; 16]);
pub const GREEN_FOLIAGE: PresetId = PresetId([7; 16]);
pub const CLEAR_GRASS: PresetId = PresetId([8; 16]);
pub const GROUND_USE: PresetUseId = PresetUseId([1; 16]);
pub const FOLIAGE_USE: PresetUseId = PresetUseId([2; 16]);

pub fn meadow_library(
    dry: TerrainSurfaceId,
    green: TerrainSurfaceId,
    channel: ChannelId,
) -> PresetLibrary {
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
    let grass = |assemblage| {
        PresetKind::Foliage(VegetationTreatment {
            id: OutputId([2; 16]),
            channel,
            assemblage,
            blend: VegetationBlend::Replace,
            strength: 1.0,
            density: 1.0,
            seed: 0,
        })
    };
    let child = |id, name: &str, preset| PresetUse {
        id,
        name: name.into(),
        preset,
        overrides: vec![],
    };
    let composition = |soil, plants, name| {
        PresetKind::Composition(vec![
            child(GROUND_USE, "Ground", soil),
            child(FOLIAGE_USE, name, plants),
        ])
    };
    PresetLibrary {
        revision: 1,
        presets: vec![
            preset(
                DRY_MEADOW,
                "Dry meadow",
                composition(DRY_GROUND, DRY_FOLIAGE, "Grass"),
            ),
            preset(
                GREEN_MEADOW,
                "Green meadow",
                composition(GREEN_GROUND, GREEN_FOLIAGE, "Grass"),
            ),
            preset(
                CLEARING,
                "Clearing",
                composition(DRY_GROUND, CLEAR_GRASS, "Clear grass"),
            ),
            preset(DRY_GROUND, "Dry ground", ground(dry)),
            preset(GREEN_GROUND, "Green ground", ground(green)),
            preset(
                DRY_FOLIAGE,
                "Dry grass",
                grass(vegetation::fixtures::DRY_FIELD_ASSEMBLAGE_ID),
            ),
            preset(
                GREEN_FOLIAGE,
                "Green grass",
                grass(vegetation::fixtures::MIXED_GREEN_ASSEMBLAGE_ID),
            ),
            preset(
                CLEAR_GRASS,
                "Clear grass",
                PresetKind::Exclusion(Exclusion {
                    id: OutputId([3; 16]),
                    channel,
                    strength: 1.0,
                }),
            ),
        ],
    }
}
