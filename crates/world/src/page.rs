//! Cooked pages: what a cell holds per domain, keyed by cell and domain, and their encoding.
use serde::{Deserialize, Serialize};
use vegetation::VegetationFieldPageData;

use crate::{
    AssetId, CellCoord, DecodeError, MAX_DECODED_PAGE_BYTES, ObjectDefinitionId, StableObjectId,
    TerrainHeightfieldPage, WorldSpaceId, codec,
};

/// An integer stored for a value of one of the enums below that names none of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} {value}")]
pub struct UnknownValue {
    pub kind: &'static str,
    pub value: i64,
}

/// An enum stored as its integer value, read back with `TryFrom<i64>`.
macro_rules! stored_enum {
    ($kind:literal, $name:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[repr(i64)]
        pub enum $name {
            $($variant = $value),+
        }

        impl TryFrom<i64> for $name {
            type Error = UnknownValue;

            fn try_from(value: i64) -> Result<Self, UnknownValue> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(UnknownValue { kind: $kind, value }),
                }
            }
        }
    };
}

stored_enum!("page domain", PageDomain {
    Terrain = 1,
    StaticObjects = 2,
    GameplayObjects = 3,
    Vegetation = 4,
});

impl PageDomain {
    pub const ALL: [Self; 4] = [
        Self::Terrain,
        Self::StaticObjects,
        Self::GameplayObjects,
        Self::Vegetation,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PageKey {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub domain: PageDomain,
}

stored_enum!("page codec", PageCodec {
    Raw = 0,
    Zstd = 1,
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticObjectInstance {
    /// Derived environment placement, never an editable manual source object.
    pub generated: bool,
    pub id: StableObjectId,
    pub asset: AssetId,
    pub translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StaticObjectsPage {
    pub instances: Vec<StaticObjectInstance>,
}

stored_enum!("object activation policy", ObjectActivationPolicy {
    RenderOnly = 0,
    Proximity = 1,
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameplayObjectInstance {
    pub id: StableObjectId,
    pub definition: ObjectDefinitionId,
    pub translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameplayObjectsPage {
    pub instances: Vec<GameplayObjectInstance>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PagePayload {
    TerrainHeightfield(TerrainHeightfieldPage),
    StaticObjects(StaticObjectsPage),
    GameplayObjects(GameplayObjectsPage),
    /// Terrain-independent vegetation coverage fields, joined to the resident terrain page at runtime.
    Vegetation(VegetationFieldPageData),
}

impl PagePayload {
    pub fn domain(&self) -> PageDomain {
        match self {
            Self::TerrainHeightfield(_) => PageDomain::Terrain,
            Self::StaticObjects(_) => PageDomain::StaticObjects,
            Self::GameplayObjects(_) => PageDomain::GameplayObjects,
            Self::Vegetation(_) => PageDomain::Vegetation,
        }
    }
}

pub fn encode_page_payload(payload: &PagePayload) -> Result<Vec<u8>, bincode::error::EncodeError> {
    codec::encode(payload)
}

pub fn decode_page_payload(bytes: &[u8]) -> Result<PagePayload, DecodeError> {
    codec::decode::<_, { MAX_DECODED_PAGE_BYTES as usize }>(bytes)
}
