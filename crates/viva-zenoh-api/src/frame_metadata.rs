//! Per-frame facts kept in the same packet as their pixels.

use serde::{Deserialize, Serialize};

/// Maximum encoded metadata bytes in an image packet.
pub const MAX_FRAME_METADATA_BYTES: usize = 64 * 1024;
/// Maximum chunk descriptions or evaluated features retained per frame.
pub const MAX_FRAME_METADATA_ITEMS: usize = 128;
/// Maximum Unicode characters in one metadata name, value or error.
pub const MAX_FRAME_METADATA_TEXT: usize = 1024;

/// One chunk carried by this frame; its contents are interpreted by GenApi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FrameChunk {
    /// Decimal chunk identifier, kept exact in JavaScript.
    pub id: String,
    /// Bytes in this chunk, without its trailer.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes: u64,
}

/// A chunk feature evaluated from this frame, or why it could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FrameFeature {
    /// Feature name from the camera XML.
    pub name: String,
    /// GenApi's textual value, without a lossy numeric conversion.
    pub value: Option<String>,
    /// The per-feature read error, when there is no value.
    pub error: Option<String>,
}

/// Facts from one frame, never paired with pixels through a separate topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FrameMetadata {
    /// Decimal transport block identifier, not a rounded JavaScript number.
    pub block_id: String,
    /// Decimal device ticks, absent when the leader supplied none.
    pub ts_dev: Option<String>,
    /// Calibrated Unix nanoseconds; absent without a fitted device clock.
    /// This is not the host reception time. Negative values precede the epoch.
    pub ts_host_ns: Option<String>,
    /// Whether every packet arrived and the device reported success.
    pub complete: bool,
    /// Chunk identifiers and sizes in this frame.
    pub chunks: Vec<FrameChunk>,
    /// Chunk values evaluated through the camera's own XML.
    pub features: Vec<FrameFeature>,
    /// Why the payload's chunks could not be decoded, when applicable.
    pub chunk_error: Option<String>,
    /// Some metadata was omitted to keep the image packet bounded.
    pub truncated: bool,
}

impl FrameMetadata {
    /// Bound evaluated values before publishing them. Chunk mode is an
    /// explicit camera setting; callers need no camera lock for plain images.
    pub fn with_features<E: std::fmt::Display>(
        mut self,
        features: impl IntoIterator<Item = (String, Result<String, E>)>,
    ) -> Self {
        for (index, (name, value)) in features.into_iter().enumerate() {
            if index == MAX_FRAME_METADATA_ITEMS {
                self.truncated = true;
                break;
            }
            let name = clipped(name, &mut self.truncated);
            let (value, error) = match value {
                Ok(value) => (Some(clipped(value, &mut self.truncated)), None),
                Err(error) => (None, Some(clipped(error.to_string(), &mut self.truncated))),
            };
            self.features.push(FrameFeature { name, value, error });
        }
        self.bound();
        self
    }

    /// Encode a bounded metadata object. Excess descriptions and text are
    /// omitted with `truncated=true`; an oversized metadata set never needs
    /// to suppress its image. Also used by the local preview/capture envelope.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut value = self.clone();
        value.bound();
        value
            .validate()
            .map_err(<serde_json::Error as serde::ser::Error>::custom)?;
        loop {
            let bytes = serde_json::to_vec(&value)?;
            if bytes.len() <= MAX_FRAME_METADATA_BYTES {
                return Ok(bytes);
            }
            value.truncated = true;
            // Reduce geometrically: even unusually verbose camera XML must
            // not cause one full serialization per omitted feature.
            if !value.features.is_empty() {
                value.features.truncate(value.features.len() / 2);
                continue;
            }
            if !value.chunks.is_empty() {
                value.chunks.truncate(value.chunks.len() / 2);
                continue;
            }
            return Err(<serde_json::Error as serde::ser::Error>::custom(
                "fixed frame metadata exceeds the packet limit",
            ));
        }
    }

    /// Decode untrusted metadata only within the packet's allocation bound.
    /// Limits also apply to counts and individual text fields.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FRAME_METADATA_BYTES {
            return Err("frame metadata exceeds 64 KiB".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), String> {
        let text_ok = |s: &str| s.chars().count() <= MAX_FRAME_METADATA_TEXT;
        let decimal_u64 = |s: &str| {
            !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u64>().is_ok()
        };
        let signed_ns = |s: &str| {
            let digits = s.strip_prefix('-').unwrap_or(s);
            !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && s.parse::<i128>().is_ok()
        };
        if !decimal_u64(&self.block_id)
            || self.ts_dev.as_ref().is_some_and(|s| !decimal_u64(s))
            || self.ts_host_ns.as_ref().is_some_and(|s| !signed_ns(s))
        {
            return Err("invalid decimal frame identifier or timestamp".into());
        }
        if self.chunks.len() > MAX_FRAME_METADATA_ITEMS
            || self.features.len() > MAX_FRAME_METADATA_ITEMS
            || self.chunk_error.as_ref().is_some_and(|s| !text_ok(s))
            || self
                .chunks
                .iter()
                .any(|c| !decimal_u64(&c.id) || c.bytes > 9_007_199_254_740_991)
            || self.features.iter().any(|f| {
                !text_ok(&f.name)
                    || f.value.as_ref().is_some_and(|s| !text_ok(s))
                    || f.error.as_ref().is_some_and(|s| !text_ok(s))
                    || f.value.is_some() == f.error.is_some()
            })
        {
            return Err("frame metadata exceeds item/text limits or has an invalid feature".into());
        }
        Ok(())
    }

    fn bound(&mut self) {
        if self.chunks.len() > MAX_FRAME_METADATA_ITEMS {
            self.chunks.truncate(MAX_FRAME_METADATA_ITEMS);
            self.truncated = true;
        }
        if self.features.len() > MAX_FRAME_METADATA_ITEMS {
            self.features.truncate(MAX_FRAME_METADATA_ITEMS);
            self.truncated = true;
        }
        for feature in &mut self.features {
            feature.name = clipped(std::mem::take(&mut feature.name), &mut self.truncated);
            feature.value = feature
                .value
                .take()
                .map(|s| clipped(s, &mut self.truncated));
            feature.error = feature
                .error
                .take()
                .map(|s| clipped(s, &mut self.truncated));
        }
        self.chunk_error = self
            .chunk_error
            .take()
            .map(|s| clipped(s, &mut self.truncated));
    }
}

fn clipped(mut text: String, truncated: &mut bool) -> String {
    if let Some((offset, _)) = text.char_indices().nth(MAX_FRAME_METADATA_TEXT) {
        text.truncate(offset);
        *truncated = true;
    }
    text
}

#[cfg(feature = "genicam")]
impl From<&viva_genicam::Frame> for FrameMetadata {
    fn from(frame: &viva_genicam::Frame) -> Self {
        let ts_host_ns = frame
            .ts_host
            .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_nanos().to_string(),
                Err(e) => format!("-{}", e.duration().as_nanos()),
            });
        let mut metadata = Self {
            block_id: frame.block_id.to_string(),
            ts_dev: frame.ts_dev.map(|t| t.to_string()),
            ts_host_ns,
            complete: frame.is_complete(),
            chunks: frame.chunks.as_ref().map_or_else(Vec::new, |chunks| {
                chunks
                    .iter()
                    .take(MAX_FRAME_METADATA_ITEMS)
                    .map(|(id, data)| FrameChunk {
                        id: id.to_string(),
                        bytes: data.len() as u64,
                    })
                    .collect()
            }),
            features: Vec::new(),
            chunk_error: None,
            truncated: frame
                .chunks
                .as_ref()
                .is_some_and(|c| c.len() > MAX_FRAME_METADATA_ITEMS),
        };
        let kind = frame.payload_type & 0x3FFF;
        let carries_chunks = frame.payload_type & 0x4000 != 0 || kind == 4 || kind == 5;
        if carries_chunks && frame.chunks.is_none() {
            metadata.chunk_error = Some(
                if frame.is_complete() {
                    "The frame's chunk layout could not be decoded"
                } else {
                    "Chunk data is unavailable for an incomplete frame"
                }
                .into(),
            );
        }
        metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> FrameMetadata {
        FrameMetadata {
            block_id: u64::MAX.to_string(),
            ts_dev: Some(u64::MAX.to_string()),
            ts_host_ns: Some("-1".into()),
            complete: true,
            chunks: vec![FrameChunk {
                id: "1".into(),
                bytes: 8,
            }],
            features: Vec::new(),
            chunk_error: None,
            truncated: false,
        }
    }

    #[test]
    fn exact_identifiers_nulls_and_per_feature_errors_survive_json() {
        let meta = metadata().with_features([
            ("ChunkTimestamp".into(), Ok("18446744073709551615".into())),
            ("ChunkGain".into(), Err("chunk too short")),
        ]);
        let bytes = meta.encode().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["block_id"], "18446744073709551615");
        assert_eq!(json["ts_host_ns"], "-1");
        assert!(json["features"][0]["error"].is_null());
        assert_eq!(json["features"][1]["error"], "chunk too short");
        assert_eq!(FrameMetadata::decode(&bytes).unwrap(), meta);
    }

    #[test]
    fn metadata_is_bounded_without_losing_the_image_or_corrupting_unicode() {
        let mut meta = metadata().with_features(
            (0..200).map(|n| (format!("Feature{n}"), Ok::<_, String>("🦀\n".repeat(2048)))),
        );
        meta.chunks = (0..200)
            .map(|n| FrameChunk {
                id: n.to_string(),
                bytes: 8,
            })
            .collect();
        let bytes = meta.encode().unwrap();
        assert!(bytes.len() <= MAX_FRAME_METADATA_BYTES);
        let bounded = FrameMetadata::decode(&bytes).unwrap();
        assert!(bounded.truncated);
        assert!(bounded.features.len() < MAX_FRAME_METADATA_ITEMS);
        assert!(bounded.chunks.len() <= MAX_FRAME_METADATA_ITEMS);
        assert!(
            bounded
                .features
                .iter()
                .all(|f| f.value.as_ref().unwrap().chars().count() <= MAX_FRAME_METADATA_TEXT)
        );
    }

    #[test]
    fn untrusted_metadata_rejects_bad_decimals_counts_and_feature_shapes() {
        assert!(FrameMetadata::decode(&vec![b' '; MAX_FRAME_METADATA_BYTES + 1]).is_err());
        assert!(FrameMetadata::decode(b"{}").is_err());
        let mut meta = metadata();
        meta.block_id = "18446744073709551616".into();
        assert!(FrameMetadata::decode(&serde_json::to_vec(&meta).unwrap()).is_err());
        meta = metadata();
        meta.features = vec![FrameFeature {
            name: "x".into(),
            value: Some("1".into()),
            error: Some("bad".into()),
        }];
        assert!(FrameMetadata::decode(&serde_json::to_vec(&meta).unwrap()).is_err());
        meta.features = vec![
            FrameFeature {
                name: "x".into(),
                value: Some("1".into()),
                error: None
            };
            129
        ];
        assert!(FrameMetadata::decode(&serde_json::to_vec(&meta).unwrap()).is_err());
    }

    #[cfg(feature = "genicam")]
    #[test]
    fn frame_mapper_keeps_missing_clocks_and_chunk_errors_explicit() {
        use viva_genicam::{Frame, FrameStatus};
        let mut frame = Frame::new(Vec::new().into(), 1, 1, viva_pfnc::PixelFormat::Mono8);
        frame.block_id = u64::MAX;
        let plain = FrameMetadata::from(&frame);
        assert_eq!(plain.block_id, u64::MAX.to_string());
        assert_eq!(plain.ts_dev, None);
        assert_eq!(plain.ts_host_ns, None);
        assert!(plain.complete);
        assert_eq!(plain.chunk_error, None);
        frame.payload_type = 0x4001;
        assert!(
            FrameMetadata::from(&frame)
                .chunk_error
                .unwrap()
                .contains("could not be decoded")
        );
        frame.status = FrameStatus::Incomplete { missing_packets: 1 };
        assert!(!FrameMetadata::from(&frame).complete);
        assert!(
            FrameMetadata::from(&frame)
                .chunk_error
                .unwrap()
                .contains("incomplete")
        );
    }
}
