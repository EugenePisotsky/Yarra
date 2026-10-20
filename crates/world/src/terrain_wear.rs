//! How worn a terrain page's ground is, finer than its weights. The cook draws roads, junctions
//! and (later) painted wear into one field per page; the page's weights hold the natural ground,
//! and the ground wears down where the field says, each surface to what its world names
//! (`environment::EnvironmentDefinition::worn`). Near terrain detail and the distant bake lay
//! worn ground the same way (`shaders/terrain_near.wesl` mirrors [`TerrainWear::lay`]).
use serde::{Deserialize, Serialize};

/// Texels per side of a page's wear field: 12.5 cm on 32 m cells.
pub const WEAR_RESOLUTION: u16 = 256;

/// Rut depth at a texel's full value, metres.
pub const RUT_DEPTH_SCALE: f32 = 0.25;

/// A worn slot entry that names no surface.
pub const NO_WORN_SLOT: u8 = u8::MAX;

/// A page's wear field and what its surfaces wear down to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainWear {
    pub resolution: u16,
    /// Row-major RGBA8 at texel centres over the page: wear 0..1, rut depth (of
    /// [`RUT_DEPTH_SCALE`]), and the traffic direction as a doubled angle (its cosine and sine,
    /// each mapped to 0..1), so opposite directions blend instead of cancelling.
    pub texels: Vec<[u8; 4]>,
    /// Per page surface (slot), up to two page surfaces it wears bare to with their shares
    /// (0..255, summing to 255); [`NO_WORN_SLOT`] marks none. A surface naming none wears to
    /// itself.
    pub worn: Vec<[(u8, u8); 2]>,
    /// Per page surface (slot), the page surface it is trampled to where wear is light, or
    /// [`NO_WORN_SLOT`] (it goes straight to its bare ground).
    pub trampled: Vec<u8>,
}

/// How much of the ground `wear` wears down goes bare (dirt, gravel) rather than trampled (dry,
/// flattened grass), at `world` (XZ): light wear tramples, heavy wear bares, the line between
/// broken up in patches about two metres across.
pub fn bare_share(wear: f64, world: [f64; 2]) -> f64 {
    let t = (wear + 0.3 * (crate::road_noise::road_patch(world, 1.8, 0x7a1) - 0.5) - 0.3) / 0.45;
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The field at a point: wear, rut depth (metres) and the traffic direction (unit, XZ; its
/// sign is arbitrary).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WearSample {
    pub wear: f64,
    pub rut: f64,
    pub direction: [f64; 2],
}

impl TerrainWear {
    pub fn is_valid(&self, surfaces: usize) -> bool {
        let n = usize::from(self.resolution);
        (2..=WEAR_RESOLUTION).contains(&self.resolution)
            && self.texels.len() == n * n
            && self.worn.len() == surfaces
            && self.trampled.len() == surfaces
            && self.worn.iter().all(|pair| {
                pair.iter()
                    .all(|&(slot, _)| slot == NO_WORN_SLOT || usize::from(slot) < surfaces)
            })
            && self
                .trampled
                .iter()
                .all(|&slot| slot == NO_WORN_SLOT || usize::from(slot) < surfaces)
    }

    /// Bilinear RGBA at `uv` (0..1 over the page), each 0..1.
    pub fn texel(&self, uv: [f64; 2]) -> [f64; 4] {
        let n = usize::from(self.resolution);
        let at = |i: usize, j: usize| self.texels[j * n + i].map(|v| f64::from(v) / 255.0);
        let p = uv.map(|v| (v * n as f64 - 0.5).clamp(0.0, (n - 1) as f64));
        let (i, j) = (p[0].floor() as usize, p[1].floor() as usize);
        let (i1, j1) = ((i + 1).min(n - 1), (j + 1).min(n - 1));
        let (u, v) = (p[0] - i as f64, p[1] - j as f64);
        let (a, b, c, d) = (at(i, j), at(i1, j), at(i, j1), at(i1, j1));
        std::array::from_fn(|k| {
            (a[k] * (1.0 - u) + b[k] * u) * (1.0 - v) + (c[k] * (1.0 - u) + d[k] * u) * v
        })
    }

    pub fn sample(&self, uv: [f64; 2]) -> WearSample {
        let t = self.texel(uv);
        let doubled = [t[2] * 2.0 - 1.0, t[3] * 2.0 - 1.0];
        let angle = doubled[1].atan2(doubled[0]) * 0.5;
        WearSample {
            wear: t[0],
            rut: t[1] * f64::from(RUT_DEPTH_SCALE),
            direction: [angle.cos(), angle.sin()],
        }
    }

    /// The natural ground's `weights` (per page surface, summing to one) worn by `wear`: that
    /// share of each surface gives way, `bare` of it ([`bare_share`]) to what it wears bare to
    /// and the rest to what it is trampled to.
    pub fn lay(&self, weights: &mut [f64], wear: f64, bare: f64) {
        if wear <= 0.0 {
            return;
        }

        let natural = weights.to_vec();

        for (slot, (&w, pair)) in natural.iter().zip(&self.worn).enumerate() {
            if w <= 0.0 {
                continue;
            }

            let given = w * wear;
            let trampled = self.trampled[slot];
            let to_trampled = if trampled == NO_WORN_SLOT {
                0.0
            } else {
                given * (1.0 - bare)
            };
            let mut to_bare = given - to_trampled;

            if pair.iter().all(|&(s, _)| s == NO_WORN_SLOT) {
                to_bare = 0.0;
            }

            weights[slot] -= to_trampled + to_bare;

            if to_trampled > 0.0 {
                weights[usize::from(trampled)] += to_trampled;
            }

            for &(s, share) in pair {
                if s != NO_WORN_SLOT {
                    weights[usize::from(s)] += to_bare * f64::from(share) / 255.0;
                }
            }
        }
    }

    /// Encodes one texel: wear 0..1, rut depth in metres, and a traffic direction (any length;
    /// zero for none).
    pub fn encode(wear: f64, rut: f64, direction: [f64; 2]) -> [u8; 4] {
        let unit = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let length = direction[0].hypot(direction[1]);
        let (c, s) = if length > 1e-9 {
            let d = direction.map(|v| v / length);
            (d[0] * d[0] - d[1] * d[1], 2.0 * d[0] * d[1])
        } else {
            (0.0, 0.0)
        };
        [
            unit(wear),
            unit(rut / f64::from(RUT_DEPTH_SCALE)),
            unit(c * 0.5 + 0.5),
            unit(s * 0.5 + 0.5),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worn_ground_replaces_its_share_of_the_natural_ground() {
        // Slot 0 (meadow) wears to slots 1 and 2 (dirt, gravel); slot 1 to itself.
        let wear = TerrainWear {
            resolution: 2,
            texels: vec![TerrainWear::encode(1.0, 0.0, [1.0, 0.0]); 4],
            worn: vec![
                [(1, 153), (2, 102)],
                [(NO_WORN_SLOT, 0), (NO_WORN_SLOT, 0)],
                [(NO_WORN_SLOT, 0), (NO_WORN_SLOT, 0)],
                [(NO_WORN_SLOT, 0), (NO_WORN_SLOT, 0)],
            ],
            // Slot 0 (meadow) is trampled to slot 3 (dry grass).
            trampled: vec![3, NO_WORN_SLOT, NO_WORN_SLOT, NO_WORN_SLOT],
        };
        assert!(wear.is_valid(4));

        // All bare: as without trampling.
        let mut w = [0.8, 0.2, 0.0, 0.0];
        wear.lay(&mut w, 0.5, 1.0);
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((w[0] - 0.4).abs() < 1e-9);
        assert!((w[1] - (0.2 + 0.4 * 0.6)).abs() < 1e-9);
        assert!((w[2] - 0.4 * 0.4).abs() < 1e-9);

        // A quarter bare: three quarters of the worn meadow trampled.
        let mut w = [0.8, 0.2, 0.0, 0.0];
        wear.lay(&mut w, 0.5, 0.25);
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!((w[3] - 0.3).abs() < 1e-9);
        assert!((w[1] - (0.2 + 0.1 * 0.6)).abs() < 1e-9);
    }

    #[test]
    fn light_wear_tramples_and_heavy_wear_bares() {
        let p = [12.3, 45.6];
        assert!(bare_share(0.05, p) < 0.1);
        assert!(bare_share(1.0, p) > 0.9);
        let mut last = 0.0;

        for i in 0..=20 {
            let b = bare_share(f64::from(i) / 20.0, p);
            assert!(b >= last);
            last = b;
        }
    }

    #[test]
    fn directions_survive_encoding_and_opposite_ones_agree() {
        let one = TerrainWear::encode(0.5, 0.1, [0.6, 0.8]);
        let other = TerrainWear::encode(0.5, 0.1, [-0.6, -0.8]);
        assert_eq!(one, other);
        let wear = TerrainWear {
            resolution: 2,
            texels: vec![one; 4],
            worn: vec![],
            trampled: vec![],
        };
        let s = wear.sample([0.3, 0.7]);
        assert!((s.wear - 0.5).abs() < 0.003);
        assert!((s.rut - 0.1).abs() < 0.002);
        let d = s.direction;
        assert!((d[0] * 0.8 - d[1] * 0.6).abs() < 0.01, "{d:?}");
    }
}
