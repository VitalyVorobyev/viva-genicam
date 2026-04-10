//! Decode GVSP chunk payloads into typed values.

use std::collections::HashMap;

use bytes::{Buf, Bytes};
use thiserror::Error;
use tracing::{debug, warn};
use viva_gige::gvsp::{self, ChunkRaw};

/// Known chunk identifiers defined by the GenICam SFNC.
const KNOWN_CHUNKS: &[KnownChunk] = &[
    KnownChunk {
        id: 0x0001,
        kind: ChunkKind::Timestamp,
        decoder: ValueDecoder::U64,
    },
    KnownChunk {
        id: 0x1002,
        kind: ChunkKind::ExposureTime,
        decoder: ValueDecoder::F64,
    },
    KnownChunk {
        id: 0x1003,
        kind: ChunkKind::Gain,
        decoder: ValueDecoder::F64,
    },
    KnownChunk {
        id: 0x0201,
        kind: ChunkKind::LineStatusAll,
        decoder: ValueDecoder::U32,
    },
];

#[derive(Copy, Clone)]
struct KnownChunk {
    id: u16,
    kind: ChunkKind,
    decoder: ValueDecoder,
}

#[derive(Copy, Clone)]
enum ValueDecoder {
    U64,
    F64,
    U32,
}

/// Typed representation of known chunk kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkKind {
    Timestamp,
    ExposureTime,
    Gain,
    LineStatusAll,
    Unknown(u16),
}

/// Decoded value of a chunk entry.
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkValue {
    U64(u64),
    F64(f64),
    U32(u32),
    Bytes(Bytes),
}

pub type ChunkMap = HashMap<ChunkKind, ChunkValue>;

/// Errors that can occur while decoding chunk payloads.
#[derive(Debug, Error)]
pub enum ChunkError {
    #[error("chunk {id:#06x} payload length {actual} shorter than required {expected} bytes")]
    InvalidLength {
        id: u16,
        expected: usize,
        actual: usize,
    },
}

fn decode_known(raw: &ChunkRaw, entry: &KnownChunk) -> Result<(ChunkKind, ChunkValue), ChunkError> {
    let expected = match entry.decoder {
        ValueDecoder::U64 | ValueDecoder::F64 => 8,
        ValueDecoder::U32 => 4,
    };
    if raw.data.len() < expected {
        warn!(
            chunk_id = format_args!("{:#06x}", raw.id),
            len = raw.data.len(),
            expected,
            "truncated chunk payload"
        );
        return Err(ChunkError::InvalidLength {
            id: raw.id,
            expected,
            actual: raw.data.len(),
        });
    }
    let mut cursor = raw.data.clone();
    let value = match entry.decoder {
        ValueDecoder::U64 => ChunkValue::U64(cursor.get_u64_le()),
        ValueDecoder::F64 => ChunkValue::F64(cursor.get_f64_le()),
        ValueDecoder::U32 => ChunkValue::U32(cursor.get_u32_le()),
    };
    debug!(
        chunk_id = format_args!("{:#06x}", raw.id),
        len = raw.data.len(),
        kind = ?entry.kind,
        "decoded known chunk"
    );
    Ok((entry.kind, value))
}

/// Decode raw chunk entries into typed values.
pub fn decode_raw_chunks(chunks: &[ChunkRaw]) -> Result<ChunkMap, ChunkError> {
    let mut map = HashMap::new();
    for chunk in chunks {
        if let Some(entry) = KNOWN_CHUNKS
            .iter()
            .find(|candidate| candidate.id == chunk.id)
        {
            let (kind, value) = decode_known(chunk, entry)?;
            map.insert(kind, value);
        } else {
            debug!(
                chunk_id = format_args!("{:#06x}", chunk.id),
                len = chunk.data.len(),
                "storing unknown chunk"
            );
            map.insert(
                ChunkKind::Unknown(chunk.id),
                ChunkValue::Bytes(chunk.data.clone()),
            );
        }
    }
    Ok(map)
}

/// Parse raw bytes into chunks and decode known values.
pub fn parse_chunk_bytes(data: &[u8]) -> Result<ChunkMap, ChunkError> {
    let raw = gvsp::parse_chunks(data);
    decode_raw_chunks(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_buffer(id: u16, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&id.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn decode_known_chunks() {
        let mut data = Vec::new();
        data.extend_from_slice(&chunk_buffer(
            0x0001,
            &0x1234_5678_9ABC_DEF0u64.to_le_bytes(),
        ));
        data.extend_from_slice(&chunk_buffer(0x1002, &1234.5f64.to_le_bytes()));
        let map = parse_chunk_bytes(&data).expect("decode");
        assert!(matches!(
            map.get(&ChunkKind::Timestamp),
            Some(ChunkValue::U64(0x1234_5678_9ABC_DEF0))
        ));
        assert!(matches!(
            map.get(&ChunkKind::ExposureTime),
            Some(ChunkValue::F64(v)) if (*v - 1234.5).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn invalid_length_errors() {
        let data = chunk_buffer(0x0001, &[0x12, 0x34]);
        let err = parse_chunk_bytes(&data).unwrap_err();
        assert!(matches!(err, ChunkError::InvalidLength { id: 0x0001, .. }));
    }

    #[test]
    fn unknown_chunk_kept_as_bytes() {
        let payload = [0xAA, 0xBB, 0xCC];
        let data = chunk_buffer(0xDEAD, &payload);
        let map = parse_chunk_bytes(&data).expect("decode");
        assert!(matches!(
            map.get(&ChunkKind::Unknown(0xDEAD)),
            Some(ChunkValue::Bytes(bytes)) if bytes.as_ref() == payload
        ));
    }

    const KNOWN_IDS: &[u16] = &[0x0001, 0x1002, 0x1003, 0x0201];

    #[test]
    fn random_unknown_chunks_are_stored() {
        for _ in 0..128 {
            let mut id = fastrand::u16(..);
            while KNOWN_IDS.contains(&id) {
                id = fastrand::u16(..);
            }
            let len = fastrand::usize(..16);
            let mut payload = vec![0u8; len];
            for byte in &mut payload {
                *byte = fastrand::u8(..);
            }
            let mut buffer = chunk_buffer(id, &payload);
            let padding_len = fastrand::usize(..8);
            for _ in 0..padding_len {
                buffer.push(fastrand::u8(..));
            }
            let raw = gvsp::parse_chunks(&buffer);
            let map = decode_raw_chunks(&raw).expect("decode");
            match map.get(&ChunkKind::Unknown(id)) {
                Some(ChunkValue::Bytes(bytes)) => assert_eq!(bytes.as_ref(), payload.as_slice()),
                Some(other) => panic!("expected raw bytes for unknown chunk, found {other:?}"),
                None => panic!("unknown chunk missing from map"),
            }
        }
    }
}
