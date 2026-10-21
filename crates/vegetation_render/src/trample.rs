//! Characters among the plants. Each frame the application lists who stands among them
//! ([`VegetationBenders`]); the grass renderer stamps them into a trample map around the camera,
//! from which blades read how far they are pressed and which way they lie, and in which they
//! slowly rise. Trees push their branches aside from the same list.
use bevy::{prelude::*, render::extract_resource::ExtractResource};

/// Most characters among the plants at once; the application lists the nearest.
pub const MAX_VEGETATION_BENDERS: usize = 16;

/// One character among the plants: grass presses as a disc under its feet, laying blades along
/// its motion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VegetationBender {
    /// Render-space position of the character's feet.
    pub position: Vec3,
    /// Horizontal velocity in metres per second.
    pub velocity: Vec2,
    /// Radius of the pressed disc in metres.
    pub radius: f32,
}

#[derive(Resource, ExtractResource, Debug, Clone, Default)]
#[extract_app(bevy::render::RenderApp)]
pub struct VegetationBenders {
    pub benders: Vec<VegetationBender>,
    /// Seconds since the previous frame: pressed grass recovers over time.
    pub delta_seconds: f32,
}
