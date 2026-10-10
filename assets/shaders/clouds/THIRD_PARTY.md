`pbr_lighting.wesl` contains the forward `apply_pbr_lighting` function from
Bevy 0.20.0 (`crates/bevy_pbr/src/render/pbr_functions.wesl`). `material.wesl`
is adapted from Bevy 0.20.0 `pbr.wesl`. Copyright Bevy contributors.
Upstream: https://github.com/bevyengine/bevy/tree/v0.20.0/crates/bevy_pbr/src/render

Licensed under MIT OR Apache-2.0. The MIT license is reproduced in LICENSE-MIT.
The local changes (marked `Yarra:` or commented in place) multiply direct directional
lighting and its diffuse transmission by the matching cloud transmittance, hand distant
shadows over to the forest shadow map, darken wet surfaces, shade tree crowns (wrap light,
crown occlusion) and split ambient light into sky and ground bounce. `material.wesl` adds
the crossfade sample mask, instanced trees and two-layer bark. Review these adapters when
upgrading Bevy: diff them against the new upstream files and take upstream changes for
everything not marked; ambient, emissive and point/spot-light behavior must stay intact.
