use super::inputs::srgb;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use world::*;

pub(super) const N: usize = TERRAIN_COMPOSITE_INTERIOR;
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub(super) struct Pixel {
    pub color: [f32; 3],
    pub normal: [f32; 3],
    pub roughness: f32,
    pub ao: f32,
    pub valid: bool,
}
impl Pixel {
    pub fn mean(samples: &[Self]) -> Self {
        let mut p = Self::default();
        let mut count = 0.;
        for s in samples.iter().filter(|p| p.valid) {
            count += 1.;
            for i in 0..3 {
                p.color[i] += s.color[i];
                p.normal[i] += s.normal[i];
            }
            p.roughness += s.roughness;
            p.ao += s.ao;
        }
        if count > 0. {
            p.color = p.color.map(|x| x / count);
            p.normal = normalize(p.normal);
            p.roughness /= count;
            p.ao /= count;
            p.valid = true;
        }
        p
    }
}
pub(super) fn normalize(n: [f32; 3]) -> [f32; 3] {
    let length = n.iter().map(|x| x * x).sum::<f32>().sqrt();
    if length < 0.00001 {
        [0., 1., 0.]
    } else {
        n.map(|x| x / length)
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Core {
    pub fingerprint: [u8; 32],
    pub pixels: Vec<Pixel>,
}
impl Core {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let (value, read): (Self, usize) = bincode::serde::decode_from_slice(
            bytes,
            bincode::config::standard().with_limit::<262144>(),
        )?;
        if read != bytes.len()
            || value.pixels.len() != N * N
            || value.pixels.iter().any(|p| {
                p.color
                    .iter()
                    .chain(&p.normal)
                    .chain([&p.roughness, &p.ao])
                    .any(|v| !v.is_finite())
            })
        {
            bail!("invalid staged composite core");
        }
        Ok(value)
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(bincode::serde::encode_to_vec(
            self,
            bincode::config::standard(),
        )?)
    }
}
pub(super) fn parent(children: &[Option<Core>; 4]) -> Core {
    let mut hash = blake3::Hasher::new();
    hash.update(b"terrain-composite-parent-v1");
    for c in children {
        hash.update(&c.as_ref().map_or([0; 32], |c| c.fingerprint));
    }
    let pixels = (0..N * N)
        .map(|i| {
            let x = i % N;
            let y = i / N;
            let Some(c) = &children[(x / (N / 2)) + 2 * (y / (N / 2))] else {
                return Pixel::default();
            };
            let x = x % (N / 2) * 2;
            let y = y % (N / 2) * 2;
            Pixel::mean(&[
                c.pixels[y * N + x],
                c.pixels[y * N + x + 1],
                c.pixels[(y + 1) * N + x],
                c.pixels[(y + 1) * N + x + 1],
            ])
        })
        .collect();
    Core {
        fingerprint: *hash.finalize().as_bytes(),
        pixels,
    }
}
/// Nine same-level cores, row-major around the tile. Missing/partial exterior
/// samples clamp to its own valid edge. No missing authored tile is filled in.
pub(super) fn finish(
    key: TerrainMaterialKey,
    neighbors: &[Option<Core>; 9],
) -> Result<TerrainComposite> {
    let center = neighbors[4]
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing composite core"))?;
    if center.pixels.iter().any(|p| !p.valid) {
        bail!("cannot draw a partial composite tile");
    }
    let mut hash = blake3::Hasher::new();
    hash.update(b"terrain-composite-halo-v1");
    for c in neighbors {
        hash.update(&c.as_ref().map_or([0; 32], |c| c.fingerprint));
    }
    let width = TerrainComposite::mip_size(0);
    let mut pixels: Vec<_> = (0..width * width)
        .map(|i| {
            let x = (i % width) as i32 - TERRAIN_COMPOSITE_GUTTER as i32;
            let y = (i / width) as i32 - TERRAIN_COMPOSITE_GUTTER as i32;
            let index = (x.div_euclid(N as i32) + 1 + 3 * (y.div_euclid(N as i32) + 1)) as usize;
            neighbors[index]
                .as_ref()
                .map(|c| {
                    c.pixels[y.rem_euclid(N as i32) as usize * N + x.rem_euclid(N as i32) as usize]
                })
                .filter(|p| p.valid)
                .unwrap_or(
                    center.pixels
                        [y.clamp(0, N as i32 - 1) as usize * N + x.clamp(0, N as i32 - 1) as usize],
                )
        })
        .collect();
    let mut mips = Vec::new();
    for level in 0..TERRAIN_COMPOSITE_MIPS {
        let mut color = Vec::with_capacity(pixels.len() * 4);
        let mut response = Vec::with_capacity(pixels.len() * 4);
        for p in &pixels {
            color.extend(p.color.map(srgb));
            color.push(255);
            let n = p.normal;
            let inverse = (n[0].abs() + n[1].abs() + n[2].abs()).recip();
            let mut oct = [n[0] * inverse, n[2] * inverse];
            if n[1] < 0. {
                oct = [
                    (1. - oct[1].abs()).copysign(oct[0]),
                    (1. - oct[0].abs()).copysign(oct[1]),
                ];
            }
            response.extend(oct.map(|x| unorm(x * 0.5 + 0.5)));
            response.extend([unorm(p.roughness), unorm(p.ao)]);
        }
        mips.push(TerrainCompositeMip { color, response });
        if level + 1 < TERRAIN_COMPOSITE_MIPS {
            let width = TerrainComposite::mip_size(level);
            let half = width / 2;
            pixels = (0..half * half)
                .map(|i| {
                    let j = (i / half * 2) * width + i % half * 2;
                    Pixel::mean(&[
                        pixels[j],
                        pixels[j + 1],
                        pixels[j + width],
                        pixels[j + width + 1],
                    ])
                })
                .collect();
        }
    }
    Ok(TerrainComposite {
        key,
        fingerprint: *hash.finalize().as_bytes(),
        mips,
    })
}
fn unorm(v: f32) -> u8 {
    (v.clamp(0., 1.) * 255.).round() as u8
}
