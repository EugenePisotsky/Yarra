// The sky composite's bindings (crates/atmosphere/src/sky.rs) and what its modules pass between
// them. Without Bevy's atmosphere the view is bound here; with it, the atmosphere's bindings hold
// it.
import package::shaders::environment::params::EnvironmentParams;
@if(!ATMOSPHERE) import bevy_render::view::View;

@if(!ATMOSPHERE)
@group(0) @binding(3) var<uniform> view: View;

@if(MULTISAMPLED)
@group(1) @binding(0) var depth: texture_depth_multisampled_2d;
@else
@group(1) @binding(0) var depth: texture_depth_2d;

@group(1) @binding(1) var<storage, read> environment: EnvironmentParams;
// Cloud images: the newest and previous complete cache refreshes.
@group(1) @binding(2) var cloud_newer: texture_2d<f32>;
@group(1) @binding(3) var cloud_older: texture_2d<f32>;
@group(1) @binding(4) var cloud_sampler: sampler;
// x: cloud cross-fade progress; y: light shafts drawn; z: main-pass pixels per shaft texel; w: sun
// rays drawn.
@group(1) @binding(5) var<uniform> composite_settings: vec4<f32>;
// Where mist pools (`atmosphere::valley_mist`): floor height, valley share, ground height, land
// share.
@group(1) @binding(6) var mist_map: texture_2d<f32>;
@group(1) @binding(7) var mist_sampler: sampler;
@group(1) @binding(8) var noise: texture_3d<f32>;
@group(1) @binding(9) var noise_sampler: sampler;
// Light shafts (sky/light_shafts.wesl): light to add and the transmittance of the air under
// crowns, and the distance each texel marched to.
@group(1) @binding(10) var shaft_light: texture_2d<f32>;
@group(1) @binding(11) var shaft_distance: texture_2d<f32>;
// The sun as the camera sees it (sky/sun_occlusion.wesl): x share seen, y share the cloud layer
// lets through, zw its main-pass uv.
@group(1) @binding(12) var<storage, read> sun_state: vec4<f32>;
// Share of the glare veil reaching each texel (sky/sun_rays.wesl), at the light shafts' scale;
// drawn when composite_settings.w is 1.
@group(1) @binding(13) var sun_rays: texture_2d<f32>;
@group(1) @binding(14) var cloud_shadow: texture_2d<f32>;
@group(1) @binding(15) var cloud_shadow_sampler: sampler;
// How waves come ashore (`atmosphere::shore`): depth below the sea level, seconds a crest takes
// to get here from deep water and that time's gradient along X and Z. Read with `mist_sampler`.
@group(1) @binding(16) var shore_map: texture_2d<f32>;

// Light added along part of a view ray, and how much of whatever lies beyond it shows through.
struct Path {
    light: vec3<f32>,
    transmittance: vec3<f32>,
}

fn in_front(light: vec3<f32>, transmittance: f32, behind: Path) -> Path {
    return Path(light + behind.light * transmittance, behind.transmittance * transmittance);
}

// Distance along a ray that meets no sea.
const NO_SEA: f32 = 1.0e30;

// Lightning (`atmosphere::lightning`): the flash's colour, on the clouds and in the channel.
const LIGHTNING_COLOR: vec3<f32> = vec3(0.85, 0.9, 1.0);
