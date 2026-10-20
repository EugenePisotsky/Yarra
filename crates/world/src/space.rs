//! World spaces, cells and positions in them, and the identities world data names.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorldSpaceId(pub i64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct CellCoord {
    pub x: i32,
    pub z: i32,
}

impl CellCoord {
    pub const ZERO: Self = Self { x: 0, z: 0 };

    pub fn containing(world_x: f64, world_z: f64, cell_size: f32) -> Self {
        assert!(cell_size.is_finite() && cell_size > 0.0);
        let cell_size = f64::from(cell_size);
        Self {
            x: floor_to_i32(world_x / cell_size),
            z: floor_to_i32(world_z / cell_size),
        }
    }

    pub fn origin(self, cell_size: f32) -> [f64; 2] {
        [
            f64::from(self.x) * f64::from(cell_size),
            f64::from(self.z) * f64::from(cell_size),
        ]
    }

    /// This cell's corner relative to `origin`'s, in metres: where it lies in render space
    /// while `origin` is the render origin.
    pub fn offset_from(self, origin: CellCoord, cell_size: f32) -> [f64; 2] {
        [
            (i64::from(self.x) - i64::from(origin.x)) as f64 * f64::from(cell_size),
            (i64::from(self.z) - i64::from(origin.z)) as f64 * f64::from(cell_size),
        ]
    }

    pub fn center(self, cell_size: f32) -> [f32; 2] {
        [
            (self.x as f32 + 0.5) * cell_size,
            (self.z as f32 + 0.5) * cell_size,
        ]
    }

    pub fn chebyshev_distance(self, other: Self) -> u32 {
        self.x.abs_diff(other.x).max(self.z.abs_diff(other.z))
    }
}

fn floor_to_i32(value: f64) -> i32 {
    let value = value.floor();
    assert!(
        value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX),
        "world coordinate is outside the supported cell range"
    );
    value as i32
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorldPosition {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub local: [f32; 3],
}

impl WorldPosition {
    pub fn from_world(space: WorldSpaceId, world: [f64; 3], cell_size: f32) -> Self {
        let cell = CellCoord::containing(world[0], world[2], cell_size);
        let origin = cell.origin(cell_size);
        Self {
            space,
            cell,
            local: [
                (world[0] - origin[0]) as f32,
                world[1] as f32,
                (world[2] - origin[1]) as f32,
            ],
        }
    }

    pub fn relative_to(self, origin_cell: CellCoord, cell_size: f32) -> [f32; 3] {
        let [x, z] = self.cell.offset_from(origin_cell, cell_size);
        [
            x as f32 + self.local[0],
            self.local[1],
            z as f32 + self.local[2],
        ]
    }

    pub fn world(self, cell_size: f32) -> [f64; 3] {
        let origin = self.cell.origin(cell_size);
        [
            origin[0] + f64::from(self.local[0]),
            f64::from(self.local[1]),
            origin[1] + f64::from(self.local[2]),
        ]
    }

    pub fn translated(self, delta: [f32; 3], cell_size: f32) -> Self {
        let world = self.world(cell_size);
        Self::from_world(
            self.space,
            [
                world[0] + f64::from(delta[0]),
                world[1] + f64::from(delta[1]),
                world[2] + f64::from(delta[2]),
            ],
            cell_size,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StableObjectId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ObjectDefinitionId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerrainSurfaceId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerrainTextureSetId(pub [u8; 16]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AssetId(pub [u8; 32]);
