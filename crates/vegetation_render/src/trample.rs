//! Characters pressing grass down. Each frame the application lists who stands in the grass
//! ([`GrassBenders`]); the renderer stamps them into a trample map around the camera, from which
//! blades read how far they are pressed and which way they lie, and in which they slowly rise.
use bevy::{prelude::*, render::extract_resource::ExtractResource};

/// Most characters pressing grass at once; the application lists the nearest.
pub const MAX_GRASS_BENDERS: usize = 16;

/// One character pressing grass: a disc under it, laying blades along its motion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GrassBender {
    /// Render-space position of the character's feet.
    pub position: Vec3,
    /// Horizontal velocity in metres per second.
    pub velocity: Vec2,
    /// Radius of the pressed disc in metres.
    pub radius: f32,
}

#[derive(Resource, ExtractResource, Debug, Clone, Default)]
#[extract_app(bevy::render::RenderApp)]
pub struct GrassBenders {
    pub benders: Vec<GrassBender>,
    /// Seconds since the previous frame: pressed grass recovers over time.
    pub delta_seconds: f32,
}
