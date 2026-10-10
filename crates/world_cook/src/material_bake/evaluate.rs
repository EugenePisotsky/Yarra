//! Detailed material's low-frequency response in canonical coordinates.
use super::{
    filter::{Core, N, Pixel},
    inputs::Inputs,
};
use anyhow::{Context, Result, bail};
use world::*;
use world_db::TerrainRenderResources;

pub(super) fn leaf(
    key: TerrainMaterialKey,
    size: f32,
    page: &TerrainHeightfieldPage,
    resources: &TerrainRenderResources,
    inputs: &Inputs,
) -> Result<Core> {
    Ok(LeafPlan::new(key, size, page, resources, inputs)?.evaluate(inputs))
}

/// A validated leaf and the fingerprint of its core, which hashes everything the evaluation
/// reads. Equal fingerprints mean identical cores, so the fingerprint can stand in for the
/// core before evaluating it.
pub(super) struct LeafPlan<'a> {
    key: TerrainMaterialKey,
    size: f32,
    page: &'a TerrainHeightfieldPage,
    surfaces: Vec<&'a world_db::RuntimeTerrainSurface>,
    profile: &'a TerrainProfile,
    pub fingerprint: [u8; 32],
}

impl<'a> LeafPlan<'a> {
    pub fn new(
        key: TerrainMaterialKey,
        size: f32,
        page: &'a TerrainHeightfieldPage,
        resources: &'a TerrainRenderResources,
        inputs: &Inputs,
    ) -> Result<Self> {
        page.heightfield.validate()?;
        let count = page.surfaces.len();
        if key.0.level != 0
            || !(1..=MAX_TERRAIN_SURFACES_PER_CELL).contains(&count)
            || !size.is_finite()
            || size <= 0.
        {
            bail!("composite baker supports one to eight surfaces on valid leaf terrain");
        }
        let surfaces = page
            .surfaces
            .iter()
            .map(|id| {
                resources
                    .surfaces
                    .iter()
                    .find(|s| s.surface.id == *id)
                    .context("missing composite surface")
            })
            .collect::<Result<Vec<_>>>()?;
        if surfaces.iter().any(|s| s.layer as usize >= inputs.layers) {
            bail!("composite surface array layer out of range");
        }
        let resolution = page.weight_pages.first().map(|w| w.resolution);
        if count > 1
            && (page.weight_pages.len() != count.div_ceil(4)
                || page.weight_pages.iter().any(|w| {
                    Some(w.resolution) != resolution
                        || w.resolution < 2
                        || w.rgba.len() != (w.resolution as usize).pow(2) * 4
                }))
        {
            bail!("invalid composite ground weights");
        }
        let profile = &resources.profile;
        let mut hash = blake3::Hasher::new();
        // v3: up to eight surfaces, height-blended three at a time.
        hash.update(b"terrain-composite-leaf-v3");
        hash.update(&inputs.hash);
        hash.update(&bincode::serde::encode_to_vec(
            (
                key,
                &page.surfaces,
                &page.weight_pages,
                page.heightfield.resolution,
                &page.heightfield.normals_oct,
                &page.heightfield.heights,
            ),
            bincode::config::standard(),
        )?);
        for s in &surfaces {
            hash.update(&s.layer.to_le_bytes());
            hash.update(&[s.surface.anti_tiling as u8]);
            for v in [
                s.surface.tile_size,
                s.surface.roughness_min,
                s.surface.roughness_max,
            ] {
                hash.update(&v.to_bits().to_le_bytes());
            }
        }
        for v in [
            size,
            profile.macro_scales[0],
            profile.macro_scales[1],
            profile.macro_scales[2],
            profile.macro_contrast,
            profile.macro_albedo_strength,
        ] {
            hash.update(&v.to_bits().to_le_bytes());
        }
        Ok(Self {
            key,
            size,
            page,
            surfaces,
            profile,
            fingerprint: *hash.finalize().as_bytes(),
        })
    }

    pub fn evaluate(&self, inputs: &Inputs) -> Core {
        let Self {
            key,
            size,
            page,
            profile,
            ..
        } = *self;
        let surfaces = &self.surfaces;
        let count = surfaces.len();
        let footprint = size as f64 / N as f64;
        let mut pixels = Vec::with_capacity(N * N);
        for y in 0..N {
            for x in 0..N {
                // Stratified coverage/macro filtering, with source texture mips selecting
                // the full output footprint. Narrow roads fade by area instead of aliasing.
                let samples: [Pixel; 4] = std::array::from_fn(|q| {
                    let uv = [
                        (x as f64 + 0.25 + (q % 2) as f64 * 0.5) / N as f64,
                        (y as f64 + 0.25 + (q / 2) as f64 * 0.5) / N as f64,
                    ];
                    let world = [
                        (key.0.x as f64 + uv[0]) * size as f64,
                        (key.0.z as f64 + uv[1]) * size as f64,
                    ];
                    let mut ground = [0.; MAX_TERRAIN_SURFACES_PER_CELL];
                    if count == 1 {
                        ground[0] = 1.;
                    } else {
                        weights(&page.weight_pages, uv, &mut ground[..count]);
                    }
                    let local = [uv[0] as f32 * size, uv[1] as f32 * size];
                    let mut p = Pixel {
                        normal: page.heightfield.sample(local, size).normal,
                        hollow: hollowness(&page.heightfield, local, size),
                        valid: true,
                        ..Default::default()
                    };
                    // The three largest weights, blended by surface height as the GPU does.
                    let candidates = terrain_blend_candidates(&ground[..count]);
                    let shaded = candidates.map(|(i, weight)| {
                        (weight > 0.).then(|| {
                            let s = surfaces[i];
                            let tile = s.surface.tile_size.max(0.001) as f64;
                            let layer = s.layer as usize;
                            let u = world.map(|x| x / tile);
                            let color = if s.surface.anti_tiling {
                                stochastic(inputs, layer, u, footprint / tile)
                            } else {
                                inputs.sample(inputs.base_color(layer), u, footprint / tile, true)
                            };
                            let material = inputs.sample(
                                inputs.normal_material(layer),
                                u,
                                footprint / tile,
                                false,
                            );
                            (s, color, material)
                        })
                    });
                    let blend = terrain_height_blend(
                        candidates.map(|c| c.1),
                        shaded.map(|s| s.map_or(0., |(_, color, _)| color[3])),
                    );
                    for ((s, color, material), weight) in shaded
                        .into_iter()
                        .zip(blend)
                        .filter_map(|(s, w)| Some((s?, w)))
                    {
                        for (channel, value) in p.color.iter_mut().zip(color) {
                            *channel += value * weight;
                        }
                        p.ao += material[2] * weight;
                        p.roughness += (s.surface.roughness_min
                            + (s.surface.roughness_max - s.surface.roughness_min) * material[3])
                            * weight;
                    }
                    let transforms = [
                        world,
                        [
                            world[0] * 0.819 - world[1] * 0.574,
                            world[0] * 0.574 + world[1] * 0.819,
                        ],
                        [
                            world[0] * -0.342 - world[1] * 0.940,
                            world[0] * 0.940 - world[1] * 0.342,
                        ],
                    ];
                    let offsets = [[0.11, 0.73], [0.37, 0.61], [0.83, 0.19]];
                    let strengths = [0.28, 0.36, 0.36];
                    let mut signal = 0.;
                    for i in 0..3 {
                        let scale = profile.macro_scales[i].max(0.001) as f64;
                        let v = inputs.sample(
                            inputs.macro_variation(),
                            [
                                transforms[i][0] / scale + offsets[i][0],
                                transforms[i][1] / scale + offsets[i][1],
                            ],
                            footprint / scale,
                            false,
                        )[0];
                        signal += (v - 0.5) * 2. * strengths[i];
                    }
                    let response = ((signal * profile.macro_contrast).clamp(-1., 1.)
                        * profile.macro_albedo_strength)
                        .exp2();
                    p.color = p.color.map(|x| (x * response).clamp(0., 1.));
                    p.roughness = p.roughness.clamp(0.08, 1.);
                    p
                });
                pixels.push(Pixel::mean(&samples));
            }
        }
        Core {
            fingerprint: self.fingerprint,
            pixels,
        }
    }
}
/// Depth of `local` below the surrounding ground, where rain water would collect: the mean of
/// two rings of heights minus the centre, normalised by `TERRAIN_HOLLOW_DEPTH_METRES`. It
/// finds carved road ruts and terrain dips alike. Rings outside this page clamp to its edge,
/// which can only weaken hollows touching the edge; puddles also require flat ground.
pub(super) fn hollowness(heightfield: &TerrainHeightfield, local: [f32; 2], size: f32) -> f32 {
    let height =
        |x: f32, z: f32| heightfield.sample_height([x.clamp(0., size), z.clamp(0., size)], size);
    let centre = height(local[0], local[1]);
    let mut sum = 0.;
    for [dx, dz] in hollow_rings() {
        sum += height(local[0] + dx, local[1] + dz);
    }
    ((sum / hollow_rings().len() as f32 - centre) / TERRAIN_HOLLOW_DEPTH_METRES).clamp(0., 1.)
}

/// Offsets of the two rings of 6 and 12 samples, computed once: a leaf takes 16,384 samples.
fn hollow_rings() -> &'static [[f32; 2]; 18] {
    static RINGS: std::sync::OnceLock<[[f32; 2]; 18]> = std::sync::OnceLock::new();
    RINGS.get_or_init(|| {
        let mut rings = [[0.; 2]; 18];
        let mut next = rings.iter_mut();
        for (radius, samples, twist) in [
            (TERRAIN_HOLLOW_RADIUS_METRES * 0.5, 6, 0.),
            (TERRAIN_HOLLOW_RADIUS_METRES, 12, 0.5),
        ] {
            for i in 0..samples {
                let angle = std::f32::consts::TAU * (i as f32 + twist) / samples as f32;
                *next.next().unwrap() = [radius * angle.cos(), radius * angle.sin()];
            }
        }
        rings
    })
}

/// Bilinear ground weights of the page's surfaces at `uv`, four per RGBA page, normalised.
fn weights(pages: &[TerrainWeightPage], uv: [f64; 2], result: &mut [f32]) {
    let n = pages[0].resolution as usize;
    let p = uv.map(|x| x * (n - 1) as f64);
    let a = p.map(|x| x.floor() as usize);
    let f = [p[0] - a[0] as f64, p[1] - a[1] as f64];
    for y in 0..2 {
        for x in 0..2 {
            let i = ((a[1] + y).min(n - 1) * n + (a[0] + x).min(n - 1)) * 4;
            let w = ((if x == 0 { 1. - f[0] } else { f[0] })
                * (if y == 0 { 1. - f[1] } else { f[1] })) as f32;
            for (surface, value) in result.iter_mut().enumerate() {
                *value += pages[surface / 4].rgba[i + surface % 4] as f32 / 255. * w;
            }
        }
    }
    let sum = result.iter().sum::<f32>().max(0.000001);
    for value in result {
        *value /= sum;
    }
}

// Reference stochastic path for a mixed plain/anti-tiling page; the usual all-
// anti-tiling case uses the prepared torus above. Constants match terrain_stochastic.wesl.
fn stochastic(inputs: &Inputs, layer: usize, uv: [f64; 2], footprint: f64) -> [f32; 4] {
    let lattice = [uv[0] + uv[1] * 0.5773502692, uv[1] * 1.1547005384];
    let cell = lattice.map(f64::floor);
    let local = [lattice[0] - cell[0], lattice[1] - cell[1]];
    let (vertices, weights) = if local[0] + local[1] <= 1. {
        (
            [
                [cell[0], cell[1]],
                [cell[0] + 1., cell[1]],
                [cell[0], cell[1] + 1.],
            ],
            [1. - local[0] - local[1], local[0], local[1]],
        )
    } else {
        (
            [
                [cell[0] + 1., cell[1] + 1.],
                [cell[0], cell[1] + 1.],
                [cell[0] + 1., cell[1]],
            ],
            [local[0] + local[1] - 1., 1. - local[0], 1. - local[1]],
        )
    };
    let weights = weights.map(|x| (x * x).powi(2));
    let sum = weights.iter().sum::<f64>().max(0.000001);
    let mut result = [0.; 4];
    for i in 0..3 {
        let vertex = vertices[i].map(|v| v as f32);
        let seed = layer as f32 * 19.19;
        let turn = (hash_2d([vertex[0] + seed, vertex[1] + seed * 0.37]) * 4.).floor() as u32;
        let rotated = match turn {
            0 => uv,
            1 => [-uv[1], uv[0]],
            2 => [-uv[0], -uv[1]],
            _ => [uv[1], -uv[0]],
        };
        let seed = layer as f32 * 23.71;
        let v = [vertex[0] + seed, vertex[1] - seed * 0.41];
        let offset = [
            hash_2d([v[0] + 17., v[1] + 3.]) * 31.,
            hash_2d([v[0] + 5., v[1] + 29.]) * 31.,
        ];
        let color = inputs.sample(
            inputs.base_color(layer),
            [rotated[0] + offset[0] as f64, rotated[1] + offset[1] as f64],
            footprint,
            true,
        );
        for c in 0..4 {
            result[c] += color[c] * (weights[i] / sum) as f32;
        }
    }
    result
}
fn hash_2d(p: [f32; 2]) -> f32 {
    let fract = |v: f32| v - v.floor();
    let mut v = [p[0], p[1], p[0]].map(|x| fract(x * 0.1031));
    let dot = v[0] * (v[1] + 33.33) + v[1] * (v[2] + 33.33) + v[2] * (v[0] + 33.33);
    v = v.map(|x| x + dot);
    fract((v[0] + v[1]) * v[2])
}
