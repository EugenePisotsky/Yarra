// Height-aware blending of overlapping ground surfaces. Mirrors
// world::terrain_material::{TERRAIN_BLEND_HEIGHT, TERRAIN_BLEND_DEPTH, terrain_height_blend},
// which the composite cooker uses, so near and distant ground agree.
const BLEND_HEIGHT: f32 = 0.5;
const BLEND_DEPTH: f32 = 0.12;

// `weights` are the three largest ground weights, `heights` their surfaces' blend heights
// (base colour alpha). Zero weights stay zero; the result sums to one.
fn height_blend(weights: vec3<f32>, heights: vec3<f32>) -> vec3<f32> {
    let sum = weights.x + weights.y + weights.z;
    if sum <= 0.0 { return weights; }
    let score = select(vec3(-1.0e9), weights / sum + heights * BLEND_HEIGHT, weights > vec3(0.0));
    let top = max(score.x, max(score.y, score.z));
    let raw = max(score - vec3(top - BLEND_DEPTH), vec3(0.0));
    return raw / (raw.x + raw.y + raw.z);
}
