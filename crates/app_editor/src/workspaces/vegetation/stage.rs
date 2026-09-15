//! Metric stage context, independent of the authored grass catalog.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct CharacterPlacement {
    pub xz: [f32; 2],
    pub yaw: f32,
}

impl Default for CharacterPlacement {
    fn default() -> Self {
        // Original studies used this fixed placement.
        Self {
            xz: [0.85, -0.2],
            yaw: 0.0,
        }
    }
}

impl CharacterPlacement {
    pub fn transform(&self) -> Transform {
        Transform::from_xyz(self.xz[0], 0.0, self.xz[1])
            .with_rotation(Quat::from_rotation_y(self.yaw.to_radians()))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self
            .xz
            .into_iter()
            .any(|v| !v.is_finite() || v.abs() > 64.0)
            || !self.yaw.is_finite()
        {
            return Err("Invalid scale character placement".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(super) enum GroundMode {
    #[default]
    Neutral,
    Meadow,
    Dried,
    OriginalStudy,
    DarkenedStudy,
    UnderstoryStudy,
    CoverageStudy,
    CanopyGroundStudy,
}

impl GroundMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Neutral => "Neutral",
            Self::Meadow => "Uncut grass",
            Self::Dried => "Dried grass",
            Self::OriginalStudy => "Ground test: original",
            Self::DarkenedStudy => "Ground test: darken only",
            Self::UnderstoryStudy => "Ground test: understory",
            Self::CoverageStudy => "Ground test: coverage",
            Self::CanopyGroundStudy => "Canopy integration",
        }
    }

    pub fn treatment(self) -> Option<u32> {
        match self {
            Self::OriginalStudy => Some(0),
            Self::DarkenedStudy => Some(1),
            Self::UnderstoryStudy => Some(2),
            Self::CoverageStudy => Some(3),
            Self::CanopyGroundStudy => Some(4),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct ReferenceSetup {
    pub camera: super::study::StudyCamera,
    pub character: CharacterPlacement,
    pub field_size: f32,
    pub ground: GroundMode,
    #[serde(default)]
    pub edge: Option<GrassEdge>,
}

impl ReferenceSetup {
    pub fn validate(&self) -> Result<(), String> {
        self.camera.validate()?;
        self.character.validate()?;
        if let Some(edge) = self.edge {
            edge.validate()?;
        }
        validate_field_size(self.field_size)
    }
}

/// World-space root coverage boundary. Complete blades may overhang the clear side.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub(super) struct GrassEdge {
    pub offset: f32,
    pub angle: f32,
    pub softness: f32,
}

impl Default for GrassEdge {
    fn default() -> Self {
        Self {
            offset: -0.3,
            angle: 33.0,
            softness: 0.2,
        }
    }
}

impl GrassEdge {
    pub fn validate(self) -> Result<(), String> {
        if !self.offset.is_finite()
            || self.offset.abs() > 64.0
            || !self.angle.is_finite()
            || self.angle.abs() > 180.0
            || !self.softness.is_finite()
            || !(0.05..=2.0).contains(&self.softness)
        {
            return Err("Invalid grass edge position, angle or transition width".into());
        }
        Ok(())
    }

    pub fn signature(self) -> [u32; 3] {
        [
            self.offset.to_bits(),
            self.angle.to_bits(),
            self.softness.to_bits(),
        ]
    }

    fn distance(self, p: Vec2) -> f32 {
        let (s, c) = self.angle.to_radians().sin_cos();
        p.dot(Vec2::new(s, c)) - self.offset
    }

    fn coverage(self, p: Vec2) -> u8 {
        let (s, c) = self.angle.to_radians().sin_cos();
        let along = p.dot(Vec2::new(c, -s));
        let irregularity = 0.10 * (along * 2.1).sin() + 0.05 * (along * 5.3 + 0.9).sin();
        let t = (0.5 - (self.distance(p) + irregularity) / self.softness).clamp(0.0, 1.0);
        (t * t * (3.0 - 2.0 * t) * 255.0).round() as u8
    }

    pub fn apply(self, page: &mut vegetation::VegetationFieldPage) {
        let origin = Vec2::from_array(page.origin_xz);
        let corners = [
            origin,
            origin + Vec2::X * page.size,
            origin + Vec2::Y * page.size,
            origin + Vec2::splat(page.size),
        ];
        let distances = corners.map(|p| self.distance(p));
        let margin = 0.15 + self.softness * 0.5;
        // Constant pages avoid allocating coverage maps across a large LOD field.
        let constant = if distances.iter().all(|d| *d >= margin) {
            Some(0)
        } else if distances.iter().all(|d| *d <= -margin) {
            Some(255)
        } else {
            None
        };
        let field = &mut page.fields[0];
        if let Some(value) = constant {
            field.resolution = 1;
            field.coverage = vec![value];
            return;
        }
        let resolution = (page.size * 16.0).ceil().clamp(2.0, 256.0) as u16;
        field.resolution = resolution;
        field.coverage = (0..u32::from(resolution).pow(2))
            .map(|index| {
                let p = Vec2::new(
                    (index % u32::from(resolution)) as f32 + 0.5,
                    (index / u32::from(resolution)) as f32 + 0.5,
                );
                self.coverage(origin + p * (page.size / f32::from(resolution)))
            })
            .collect();
    }
}

pub(super) const FIELD_SIZES: [f32; 4] = [4.0, 16.0, 64.0, 128.0];

pub(super) fn validate_field_size(size: f32) -> Result<(), String> {
    if !FIELD_SIZES.contains(&size) {
        return Err("Field size must be 4, 16, 64, or 128 metres".into());
    }
    Ok(())
}

pub(super) fn legacy_ruler() -> bool {
    true
}
