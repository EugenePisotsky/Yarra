//! Render-only asset collections. Spatial placements are disposable compiler output.
use super::*;
use world::AssetId;

pub const ASSET_CHANNEL: ChannelId = ChannelId([2; 16]);
pub const MAX_COLLECTION_ASSETS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionAsset {
    pub asset: AssetId,
    pub weight: f32,
    pub scale_min: f32,
    pub scale_max: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetCollection {
    pub id: OutputId,
    pub channel: ChannelId,
    pub assets: Vec<CollectionAsset>,
    /// Minimum root distance within this output, including across cell boundaries.
    pub spacing: f32,
    /// Stable thinning of the maximum distribution; does not move retained instances.
    pub density: f32,
    pub seed: u32,
    pub max_slope_degrees: f32,
    /// Root clearance beyond the entire road corridor and junction area, not wheel tracks.
    pub road_clearance: f32,
}

impl AssetCollection {
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut ids = BTreeSet::new();
        if self.assets.is_empty() || self.assets.len() > MAX_COLLECTION_ASSETS {
            return Err(ValidationError::Preset(
                "choose 1–64 collection assets".into(),
            ));
        }
        if self.assets.iter().any(|a| {
            !ids.insert(a.asset)
                || !a.weight.is_finite()
                || !(0.001..=1000.0).contains(&a.weight)
                || !a.scale_min.is_finite()
                || !a.scale_max.is_finite()
                || !(0.01..=100.0).contains(&a.scale_min)
                || !(a.scale_min..=100.0).contains(&a.scale_max)
        }) {
            return Err(ValidationError::Preset(
                "collection asset weights, scales or duplicate assets".into(),
            ));
        }
        if !self.spacing.is_finite()
            || !(0.5..=64.0).contains(&self.spacing)
            || !unit(self.density)
            || !self.max_slope_degrees.is_finite()
            || !(0.0..=85.0).contains(&self.max_slope_degrees)
            || !self.road_clearance.is_finite()
            || !(0.0..=4.0).contains(&self.road_clearance)
        {
            return Err(ValidationError::Preset(
                "collection spacing, density, slope or road clearance".into(),
            ));
        }
        Ok(())
    }
}
