//! Compact serialized form of [`TerrainHeightfield`].
//!
//! Heights are whole multiples of [`TERRAIN_HEIGHT_STEP`] and normal axes keep
//! [`TERRAIN_NORMAL_BITS`], so every constructed field round-trips exactly. Each value plane
//! stores planar-predicted residuals as zigzag LEB128 bytes: smooth ground needs about one byte
//! per value before the page or node codec compresses it, instead of four uncompressible float
//! bytes.
use crate::{
    MAX_TERRAIN_HEIGHTFIELD_RESOLUTION, TERRAIN_HEIGHT_STEP, TERRAIN_NORMAL_BITS,
    TerrainHeightfield,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};

/// About ±2,000 km: far beyond any world, and small enough that predictions cannot overflow.
const MAX_HEIGHT_STEPS: i64 = 1 << 31;
const NORMAL_SHIFT: u32 = 16 - TERRAIN_NORMAL_BITS;
const MAX_NORMAL_VALUE: i64 = (1 << (TERRAIN_NORMAL_BITS - 1)) - 1;

#[derive(Serialize, Deserialize)]
struct PackedTerrainHeightfield {
    resolution: u16,
    heights: Vec<u8>,
    /// Octahedral X plane followed by the Z plane.
    normals: Vec<u8>,
}

impl Serialize for TerrainHeightfield {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(ser::Error::custom)?;
        let n = usize::from(self.resolution);
        let heights = self
            .heights
            .iter()
            .map(|&height| {
                let steps = (f64::from(height) / f64::from(TERRAIN_HEIGHT_STEP)).round();
                (steps.abs() <= MAX_HEIGHT_STEPS as f64).then_some(steps as i64)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| ser::Error::custom("terrain height outside the encodable range"))?;
        let normal_plane = |axis: usize| -> Vec<i64> {
            self.normals_oct
                .iter()
                .map(|normal| {
                    (f64::from(normal[axis]) / f64::from(1_u16 << NORMAL_SHIFT))
                        .round()
                        .clamp(-MAX_NORMAL_VALUE as f64, MAX_NORMAL_VALUE as f64)
                        as i64
                })
                .collect()
        };
        let mut packed = PackedTerrainHeightfield {
            resolution: self.resolution,
            heights: Vec::with_capacity(n * n * 2),
            normals: Vec::with_capacity(n * n * 3),
        };
        pack_plane(&heights, n, &mut packed.heights);
        pack_plane(&normal_plane(0), n, &mut packed.normals);
        pack_plane(&normal_plane(1), n, &mut packed.normals);
        packed.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TerrainHeightfield {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let packed = PackedTerrainHeightfield::deserialize(deserializer)?;
        let n = usize::from(packed.resolution);
        if !(2..=usize::from(MAX_TERRAIN_HEIGHTFIELD_RESOLUTION)).contains(&n) {
            return Err(de::Error::custom("terrain heightfield resolution"));
        }
        let mut heights = packed.heights.as_slice();
        let steps = unpack_plane(&mut heights, n, MAX_HEIGHT_STEPS).map_err(de::Error::custom)?;
        let mut normals = packed.normals.as_slice();
        let x = unpack_plane(&mut normals, n, MAX_NORMAL_VALUE).map_err(de::Error::custom)?;
        let z = unpack_plane(&mut normals, n, MAX_NORMAL_VALUE).map_err(de::Error::custom)?;
        if !heights.is_empty() || !normals.is_empty() {
            return Err(de::Error::custom("terrain heightfield trailing samples"));
        }
        let field = Self {
            resolution: packed.resolution,
            heights: steps
                .into_iter()
                .map(|steps| steps as f32 * TERRAIN_HEIGHT_STEP)
                .collect(),
            normals_oct: x
                .into_iter()
                .zip(z)
                .map(|(x, z)| [(x as i16) << NORMAL_SHIFT, (z as i16) << NORMAL_SHIFT])
                .collect(),
        };
        field.validate().map_err(de::Error::custom)?;
        Ok(field)
    }
}

/// Planar prediction from the left, lower and lower-left neighbours; edges use one neighbour.
fn predict(plane: &[i64], n: usize, index: usize) -> i64 {
    match (index % n, index / n) {
        (0, 0) => 0,
        (_, 0) => plane[index - 1],
        (0, _) => plane[index - n],
        _ => plane[index - 1] + plane[index - n] - plane[index - n - 1],
    }
}

fn pack_plane(plane: &[i64], n: usize, output: &mut Vec<u8>) {
    for index in 0..plane.len() {
        let residual = plane[index] - predict(plane, n, index);
        let mut value = ((residual << 1) ^ (residual >> 63)) as u64;
        while value >= 0x80 {
            output.push(value as u8 | 0x80);
            value >>= 7;
        }
        output.push(value as u8);
    }
}

fn unpack_plane(bytes: &mut &[u8], n: usize, limit: i64) -> Result<Vec<i64>, &'static str> {
    let mut plane = Vec::with_capacity(n * n);
    for index in 0..n * n {
        let mut value = 0_u64;
        let mut shift = 0;
        loop {
            let (&byte, rest) = bytes
                .split_first()
                .ok_or("terrain heightfield samples are truncated")?;
            *bytes = rest;
            if shift == 63 && byte > 1 {
                return Err("terrain heightfield residual overflows");
            }
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 63 {
                return Err("terrain heightfield residual overflows");
            }
        }
        let residual = (value >> 1) as i64 ^ -((value & 1) as i64);
        let sample = predict(&plane, n, index)
            .checked_add(residual)
            .filter(|sample| sample.abs() <= limit)
            .ok_or("terrain heightfield sample outside the encodable range")?;
        plane.push(sample);
    }
    Ok(plane)
}

#[cfg(test)]
mod tests {
    use crate::{
        PagePayload, TERRAIN_HEIGHT_STEP, TerrainHeightfield, TerrainHeightfieldPage,
        decode_page_payload, encode_page_payload,
    };

    fn round_trip(field: &TerrainHeightfield) -> TerrainHeightfield {
        let payload = PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
            heightfield: field.clone(),
            surfaces: Vec::new(),
            weight_pages: Vec::new(),
        });
        let PagePayload::TerrainHeightfield(page) =
            decode_page_payload(&encode_page_payload(&payload).unwrap()).unwrap()
        else {
            unreachable!()
        };
        page.heightfield
    }

    /// Rolling hills at 1.5 km, optionally with up to 36 mm of uncorrelated roughness.
    fn relief(n: usize, cell_size: f32, roughness: f32) -> TerrainHeightfield {
        let spacing = cell_size / (n - 1) as f32;
        let heights: Vec<f32> = (0..n * n)
            .map(|i| {
                let (x, z) = ((i % n) as f32 * spacing, (i / n) as f32 * spacing);
                1500.0
                    + 40.0 * (x * 0.021).sin() * (z * 0.017).cos()
                    + roughness * 0.003 * ((i * 7919) % 13) as f32
            })
            .collect();
        TerrainHeightfield::from_heights(n as u16, &heights, 1000.0, 2000.0, cell_size).unwrap()
    }

    #[test]
    fn constructed_fields_round_trip_exactly() {
        for field in [
            relief(2, 8.0, 1.0),
            relief(33, 8.0, 1.0),
            relief(257, 512.0, 1.0),
            // Vertical cliffs use the full octahedral range, including the lower hemisphere fold.
            TerrainHeightfield::from_heights_and_normals(
                2,
                &[-12.3456, 0.0, 7.0001, 65_000.25],
                &[
                    [1.0, 0.0, 0.0],
                    [0.0, 0.0, -1.0],
                    [0.6, -0.8, 0.0],
                    [0.0, 1.0, 0.0],
                ],
                -100.0,
                70_000.0,
            )
            .unwrap(),
        ] {
            assert_eq!(round_trip(&field), field);
            for height in &field.heights {
                assert_eq!((height / TERRAIN_HEIGHT_STEP).fract(), 0.0);
            }
        }
    }

    #[test]
    fn smooth_ground_takes_about_one_byte_per_value() {
        let field = relief(33, 8.0, 0.0);
        let bytes = bincode::serde::encode_to_vec(
            &field,
            bincode::config::standard()
                .with_little_endian()
                .with_fixed_int_encoding(),
        )
        .unwrap();
        // One height and two normal axes per sample. Float heights and 16-bit normals took
        // 8 bytes per sample (8,712 here).
        assert!(bytes.len() < 33 * 33 * 4, "{} bytes", bytes.len());
    }

    #[test]
    fn corrupt_samples_are_rejected() {
        let field = relief(3, 8.0, 1.0);
        let config = bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding();
        let valid = bincode::serde::encode_to_vec(&field, config).unwrap();
        let decode = |bytes: &[u8]| {
            bincode::serde::decode_from_slice::<TerrainHeightfield, _>(bytes, config).map(|v| v.0)
        };
        assert_eq!(decode(&valid).unwrap(), field);
        // Resolution (u16), then the height plane's u64 length and bytes.
        let heights_len = u64::from_le_bytes(valid[2..10].try_into().unwrap()) as usize;
        let mut truncated = valid.clone();
        truncated[2..10].copy_from_slice(&(heights_len as u64 - 1).to_le_bytes());
        truncated.remove(10 + heights_len - 1);
        assert!(decode(&truncated).is_err());
        let mut extended = valid.clone();
        extended[2..10].copy_from_slice(&(heights_len as u64 + 1).to_le_bytes());
        extended.insert(10 + heights_len, 0);
        assert!(decode(&extended).is_err());
        let mut overlong = valid.clone();
        overlong[2..10].copy_from_slice(&(heights_len as u64 + 10).to_le_bytes());
        for _ in 0..10 {
            overlong.insert(10, 0xff);
        }
        assert!(decode(&overlong).is_err());
        let mut resolution = valid;
        resolution[0..2].copy_from_slice(&1_u16.to_le_bytes());
        assert!(decode(&resolution).is_err());
    }

    #[test]
    fn non_finite_heights_are_not_encoded() {
        let mut field = relief(2, 8.0, 1.0);
        field.heights[1] = f32::INFINITY;
        let payload = PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
            heightfield: field,
            surfaces: Vec::new(),
            weight_pages: Vec::new(),
        });
        assert!(encode_page_payload(&payload).is_err());
    }
}
