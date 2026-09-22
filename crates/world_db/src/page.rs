//! Bounded page decoding and payload integrity checks; no database connection.
use crate::WorldDbError;
use std::io::{Cursor, Read};
use world::{MAX_DECODED_PAGE_BYTES, PageCodec, PageKey, PagePayload, decode_page_payload};

#[derive(Debug, Clone)]
pub struct EncodedPage {
    pub key: PageKey,
    pub codec: PageCodec,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
    pub checksum: [u8; 32],
    pub payload: Vec<u8>,
}

impl EncodedPage {
    pub fn decode(self) -> Result<DecodedPage, WorldDbError> {
        if self.decoded_bytes > MAX_DECODED_PAGE_BYTES {
            return Err(WorldDbError::PageTooLarge {
                actual: self.decoded_bytes,
                maximum: MAX_DECODED_PAGE_BYTES,
            });
        }

        let bytes = match self.codec {
            PageCodec::Raw => self.payload,
            PageCodec::Zstd => {
                let decoder = zstd::stream::read::Decoder::new(Cursor::new(self.payload))?;
                let mut bytes = Vec::with_capacity(self.decoded_bytes as usize);
                decoder
                    .take(MAX_DECODED_PAGE_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_DECODED_PAGE_BYTES {
                    return Err(WorldDbError::PageTooLarge {
                        actual: bytes.len() as u64,
                        maximum: MAX_DECODED_PAGE_BYTES,
                    });
                }
                bytes
            }
        };

        if bytes.len() as u64 != self.decoded_bytes {
            return Err(WorldDbError::DecodedSizeMismatch {
                expected: self.decoded_bytes,
                actual: bytes.len() as u64,
            });
        }
        let actual_checksum = *blake3::hash(&bytes).as_bytes();
        if actual_checksum != self.checksum {
            return Err(WorldDbError::ChecksumMismatch(self.key));
        }
        let payload = decode_page_payload(&bytes)?;
        if payload.domain() != self.key.domain {
            return Err(WorldDbError::DomainMismatch {
                expected: self.key.domain,
                actual: payload.domain(),
            });
        }

        Ok(DecodedPage {
            key: self.key,
            payload,
            decoded_bytes: self.decoded_bytes,
            gpu_bytes_estimate: self.gpu_bytes_estimate,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DecodedPage {
    pub key: PageKey,
    pub payload: PagePayload,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
}
