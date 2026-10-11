//! Small, deterministic fixtures shared by contract, compiler, and renderer tests: an even
//! meadow and a tufted grass on two adjacent pages. Worlds take their grass from a pack
//! (`assets/packs/grass/`), never from these.

use crate::{
    GrassClumps, GrassColors, GrassField, GrassType, VegetationCatalog, VegetationFieldPage,
    VegetationScene, VegetationSurfaceField,
};

pub const REFERENCE_PAGE_SIZE: f32 = 16.0;
pub const REFERENCE_FIELD_RESOLUTION: u16 = 16;

/// Fine, fairly even grass.
pub fn meadow() -> GrassType {
    GrassType {
        key: "meadow".into(),
        density: 48.0,
        height: [0.3, 0.7],
        width: [0.005, 0.009],
        tilt: [0.1, 0.5],
        bend: [0.1, 0.4],
        clumps: GrassClumps {
            size: 0.8,
            pull: 0.2,
            height: 0.4,
            facing: 0.5,
            splay: 0.3,
            patches: 0.1,
        },
        colors: GrassColors {
            root: [0.03, 0.06, 0.015],
            tip: [0.18, 0.32, 0.07],
            dry_tip: [0.42, 0.36, 0.14],
            dry: 0.15,
            variation: 0.15,
        },
        gloss: 0.6,
        roughness: 0.4,
        translucency: 0.35,
        stiffness: 0.4,
        twin: false,
    }
}

/// Stiff grass gathered into tufts.
pub fn tufts() -> GrassType {
    GrassType {
        key: "tufts".into(),
        density: 24.0,
        height: [0.4, 0.9],
        width: [0.004, 0.007],
        tilt: [0.15, 0.4],
        bend: [0.0, 0.3],
        clumps: GrassClumps {
            size: 0.9,
            pull: 0.8,
            height: 0.5,
            facing: 0.1,
            splay: 0.8,
            patches: 0.8,
        },
        colors: GrassColors {
            root: [0.05, 0.06, 0.03],
            tip: [0.25, 0.3, 0.14],
            dry_tip: [0.45, 0.4, 0.22],
            dry: 0.3,
            variation: 0.1,
        },
        gloss: 0.4,
        roughness: 0.5,
        translucency: 0.2,
        stiffness: 0.7,
        twin: true,
    }
}

pub fn reference_catalog() -> VegetationCatalog {
    VegetationCatalog {
        grass: vec![meadow(), tufts()],
        details: vec![],
        surfaces: vec![],
    }
}

pub fn reference_scene() -> VegetationScene {
    VegetationScene {
        catalog: reference_catalog(),
        pages: vec![reference_page([0.0, 0.0]), reference_page([16.0, 0.0])],
    }
}

/// Meadow everywhere except a band of tufts that crosses both reference pages.
pub fn reference_page(origin_xz: [f32; 2]) -> VegetationFieldPage {
    let resolution = usize::from(REFERENCE_FIELD_RESOLUTION);
    let mut meadow_coverage = Vec::with_capacity(resolution * resolution);
    let mut tuft_coverage = Vec::with_capacity(resolution * resolution);
    for z in 0..resolution {
        for _ in 0..resolution {
            let world_z = origin_xz[1] + (z as f32 + 0.5) * REFERENCE_PAGE_SIZE / resolution as f32;
            let band = smoothstep(2.0, 6.0, world_z) * (1.0 - smoothstep(10.0, 15.0, world_z));
            meadow_coverage.push(to_unorm8(1.0 - 0.8 * band));
            tuft_coverage.push(to_unorm8(band));
        }
    }
    VegetationFieldPage {
        origin_xz,
        size: REFERENCE_PAGE_SIZE,
        surface: VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
        fields: vec![
            GrassField {
                grass: meadow().id(),
                resolution: REFERENCE_FIELD_RESOLUTION,
                coverage: meadow_coverage,
            },
            GrassField {
                grass: tufts().id(),
                resolution: REFERENCE_FIELD_RESOLUTION,
                coverage: tuft_coverage,
            },
        ],
    }
}

pub fn full_coverage_page(
    origin_xz: [f32; 2],
    size: f32,
    grass: crate::GrassTypeId,
) -> VegetationFieldPage {
    VegetationFieldPage {
        origin_xz,
        size,
        surface: VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
        fields: vec![GrassField {
            grass,
            resolution: 1,
            coverage: vec![u8::MAX],
        }],
    }
}

fn smoothstep(minimum: f32, maximum: f32, value: f32) -> f32 {
    let t = ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn to_unorm8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
