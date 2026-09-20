`pbr_lighting.wgsl` contains the forward `apply_pbr_lighting` function from
Bevy 0.19.1 (`crates/bevy_pbr/src/render/pbr_functions.wgsl`). `material.wgsl`
is adapted from Bevy 0.19.1 `pbr.wgsl`. Copyright Bevy contributors.
Upstream: https://github.com/bevyengine/bevy/tree/v0.19.1/crates/bevy_pbr/src/render

Licensed under MIT OR Apache-2.0. The MIT license is reproduced in LICENSE-MIT.
The local change multiplies direct directional lighting and its diffuse
transmission by the matching cloud transmittance. Review this adapter when
upgrading Bevy; ambient, emissive and point/spot-light behavior must stay intact.
