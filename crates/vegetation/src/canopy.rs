//! Adjustable artistic canopy integration. This is a shared shading envelope, not traced shadows.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanopyShading {
    pub enabled: bool,
    pub strength: f32,
    pub ground_amount: f32,
    pub blade_amount: f32,
    pub height_metres: f32,
    pub softness: f32,
    pub patch_metres: f32,
    pub patchiness: f32,
    pub openings: f32,
    /// Fraction of the maximum shade at close range; zero leaves nearby surfaces unchanged.
    pub near_strength: f32,
    pub patch_growth: f32,
    pub edge_width: f32,
    pub distance_start: f32,
    pub distance_end: f32,
}
impl Default for CanopyShading {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 0.78,
            ground_amount: 1.0,
            blade_amount: 1.0,
            height_metres: 0.34,
            softness: 1.0,
            patch_metres: 1.2,
            patchiness: 1.0,
            openings: 0.3,
            near_strength: 0.0,
            patch_growth: 0.65,
            edge_width: 1.25,
            distance_start: 3.0,
            distance_end: 20.0,
        }
    }
}
impl CanopyShading {
    pub fn experiment() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }
    /// Identical layout for terrain and grass. Origin keeps the pattern fixed during rebasing.
    pub fn packed(self, origin: [f32; 2]) -> [[f32; 4]; 4] {
        [
            [
                if self.enabled {
                    self.strength.clamp(0.0, 0.98)
                } else {
                    0.0
                },
                self.ground_amount.clamp(0.0, 1.0),
                self.blade_amount.clamp(0.0, 1.0),
                self.near_strength.clamp(0.0, 1.0),
            ],
            [
                self.height_metres.clamp(0.02, 1.5),
                self.softness.clamp(0.1, 1.0),
                self.patch_metres.clamp(0.2, 8.0),
                self.patchiness.clamp(0.0, 1.0),
            ],
            [
                self.distance_start.max(0.0),
                self.distance_end.max(self.distance_start + 0.1),
                self.patch_growth.clamp(0.0, 1.0),
                self.openings.clamp(0.0, 0.8),
            ],
            [origin[0], origin[1], self.edge_width.clamp(0.05, 4.0), 0.0],
        ]
    }
}
