//! The one binary encoding of stored world data: bincode with little-endian, fixed-size
//! integers. Blobs carry no format version of their own. The database schema versions cover
//! everything stored in them, so changing any stored type bumps
//! [`PROJECT_SCHEMA_VERSION`](crate::PROJECT_SCHEMA_VERSION) or
//! [`RUNTIME_SCHEMA_VERSION`](crate::RUNTIME_SCHEMA_VERSION).
use bincode::config::{Configuration, Fixint, LittleEndian, NoLimit};
use serde::{Serialize, de::DeserializeOwned};

const CONFIG: Configuration<LittleEndian, Fixint, NoLimit> =
    bincode::config::standard().with_fixed_int_encoding();

pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, bincode::error::EncodeError> {
    bincode::serde::encode_to_vec(value, CONFIG)
}

/// Decodes all of `bytes`, allocating at most `LIMIT` bytes on the way.
pub fn decode<T: DeserializeOwned, const LIMIT: usize>(bytes: &[u8]) -> Result<T, DecodeError> {
    let (value, consumed) = bincode::serde::decode_from_slice(bytes, CONFIG.with_limit::<LIMIT>())?;

    if consumed != bytes.len() {
        return Err(DecodeError::TrailingBytes {
            consumed,
            total: bytes.len(),
        });
    }

    Ok(value)
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("{0}")]
    Bincode(#[from] bincode::error::DecodeError),
    #[error("trailing bytes: decoded {consumed} of {total}")]
    TrailingBytes { consumed: usize, total: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_trailing_bytes_are_refused() {
        let value = (7_u32, vec![1.5_f32, -2.0], String::from("cell"));
        let mut bytes = encode(&value).unwrap();
        assert_eq!(
            decode::<(u32, Vec<f32>, String), 1024>(&bytes).unwrap(),
            value
        );
        bytes.push(0);
        assert!(matches!(
            decode::<(u32, Vec<f32>, String), 1024>(&bytes),
            Err(DecodeError::TrailingBytes { .. })
        ));
        assert!(decode::<Vec<u8>, 4>(&encode(&vec![0_u8; 8]).unwrap()).is_err());
    }
}
