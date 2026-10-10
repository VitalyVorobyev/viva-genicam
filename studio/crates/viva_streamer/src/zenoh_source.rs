//! Zenoh subscription loop that converts frames into BMP bytes.
//!
//! Two subscribers run concurrently inside a single `select!` loop:
//!
//! - **meta subscriber** — listens on `image/meta`, updates the shared
//!   `Arc<RwLock<ImageMeta>>` and the stream info the viewer reads.
//! - **frame subscriber** — receives framed payloads (`16-byte FrameHeader + raw
//!   pixels`), decodes the inline header, and draws the pixels with
//!   [`FrameEncoder`] — the same encoder embedded mode uses, which lists the
//!   formats it can draw ([`PixelLayout::of_wire`]). A frame is not encoded
//!   while no viewer is connected, nor above `fps_limit`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{RwLock, watch};
use tracing::{info, warn};
use viva_zenoh_api::{FrameHeader, ImageMeta, PixelFormat};

use crate::error::StreamerError;
use crate::frame::{FrameEncoder, PixelLayout};

#[derive(Clone, Debug)]
pub struct ZenohSourceConfig {
    pub key_expr: String,
    pub meta_key: String,
    pub fps_limit: Option<u32>,
}

/// Run the Zenoh source by opening a new session from the given config.
pub async fn run(
    config: zenoh::Config,
    source: ZenohSourceConfig,
    shared_meta: Arc<RwLock<ImageMeta>>,
    frame_tx: watch::Sender<Bytes>,
    info_tx: watch::Sender<crate::ws::StreamInfo>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), StreamerError> {
    let session = Arc::new(zenoh::open(config).await?);
    run_inner(session, source, shared_meta, frame_tx, info_tx, shutdown).await
}

/// Run the Zenoh source with a pre-opened session (for embedding).
pub async fn run_with_session(
    session: Arc<zenoh::Session>,
    source: ZenohSourceConfig,
    shared_meta: Arc<RwLock<ImageMeta>>,
    frame_tx: watch::Sender<Bytes>,
    info_tx: watch::Sender<crate::ws::StreamInfo>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), StreamerError> {
    run_inner(session, source, shared_meta, frame_tx, info_tx, shutdown).await
}

async fn run_inner(
    session: Arc<zenoh::Session>,
    source: ZenohSourceConfig,
    shared_meta: Arc<RwLock<ImageMeta>>,
    frame_tx: watch::Sender<Bytes>,
    info_tx: watch::Sender<crate::ws::StreamInfo>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), StreamerError> {
    let min_interval = source
        .fps_limit
        .map(|fps| Duration::from_secs_f64(1.0 / fps as f64));

    let frame_sub = session.declare_subscriber(&source.key_expr).await?;
    let meta_sub = session.declare_subscriber(&source.meta_key).await?;
    let mut encoder = FrameEncoder::new();
    // Warned about once, not on every frame.
    let mut unsupported: Option<PixelFormat> = None;

    info!(
        "Subscribed to '{}' (meta: '{}')",
        source.key_expr, source.meta_key
    );

    let mut last_emit: Option<Instant> = None;
    let mut logged_first_meta = false;
    let mut logged_first_raw_frame = false;
    let mut logged_first_bmp = false;

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }

            // ── Meta subscriber ───────────────────────────────────────────
            sample = meta_sub.recv_async() => {
                let sample = match sample {
                    Ok(s) => s,
                    Err(err) => {
                        warn!("Meta subscriber closed: {err}");
                        break;
                    }
                };

                let payload = sample.payload().to_bytes();
                let meta: ImageMeta = match crate::meta::parse_meta_payload(&payload) {
                    Ok(m) => m,
                    Err(err) => {
                        warn!("Failed to parse image/meta payload: {err}");
                        continue;
                    }
                };

                if !logged_first_meta {
                    info!(
                        key = %source.meta_key,
                        width = meta.width,
                        height = meta.height,
                        pixel_format = ?meta.pixel_format,
                        payload_size = meta.payload_size,
                        "First image/meta received"
                    );
                    logged_first_meta = true;
                }

                let stream_info = crate::ws::StreamInfo::from_image_meta(&meta);
                *shared_meta.write().await = meta;

                if info_tx.send(stream_info).is_err() {
                    warn!("No WS clients subscribed to info updates");
                }
            }

            // ── Frame subscriber ──────────────────────────────────────────
            sample = frame_sub.recv_async() => {
                let sample = match sample {
                    Ok(s) => s,
                    Err(err) => {
                        warn!("Frame subscriber closed: {err}");
                        break;
                    }
                };

                if let Some(interval) = min_interval
                    && let Some(last) = last_emit
                        && last.elapsed() < interval {
                            continue;
                        }
                // Nobody is watching: decoding and encoding would be wasted.
                if frame_tx.receiver_count() == 0 {
                    continue;
                }

                let raw = sample.payload().to_bytes();

                let (header, pixel_data) = match FrameHeader::decode(raw.as_ref()) {
                    Ok(pair) => pair,
                    Err(err) => {
                        warn!("Dropped frame: invalid frame header: {err}");
                        continue;
                    }
                };

                if !logged_first_raw_frame {
                    info!(
                        key = %source.key_expr,
                        seq = header.seq,
                        width = header.width,
                        height = header.height,
                        pixel_format = ?header.pixel_format,
                        bytes = raw.len(),
                        "First raw image frame received"
                    );
                    logged_first_raw_frame = true;
                }

                // Validate pixel data size against the declared format.
                let bpp = header.pixel_format.bytes_per_pixel();
                let expected_pixels = match (header.width as usize).checked_mul(header.height as usize) {
                    Some(n) => n,
                    None => {
                        warn!("Dropped frame seq={}: width×height overflows usize", header.seq);
                        continue;
                    }
                };
                let expected_bytes = (expected_pixels as f32 * bpp) as usize;

                if pixel_data.len() < expected_bytes {
                    warn!(
                        "Dropped frame seq={}: pixel data too small (expected {}, got {})",
                        header.seq, expected_bytes, pixel_data.len()
                    );
                    continue;
                }
                // Truncate trailing bytes (e.g. GVSP row padding) if payload is oversized.
                let pixel_data = &pixel_data[..expected_bytes];

                let Some(layout) = PixelLayout::of_wire(&header.pixel_format) else {
                    if unsupported.as_ref() != Some(&header.pixel_format) {
                        warn!(
                            "Pixel format {:?} cannot be drawn; dropping its frames",
                            header.pixel_format
                        );
                        unsupported = Some(header.pixel_format.clone());
                    }
                    continue;
                };
                let frame = match encoder.encode(layout, header.width, header.height, pixel_data) {
                    Ok(frame) => frame,
                    Err(err) => {
                        warn!("BMP encode error for seq={}: {err}", header.seq);
                        continue;
                    }
                };

                let bmp_len = frame.len();
                if frame_tx.send(frame).is_err() {
                    warn!("No WebSocket clients are listening for frames");
                } else if !logged_first_bmp {
                    info!(
                        seq = header.seq,
                        bytes = bmp_len,
                        "First BMP frame published to WebSocket broadcaster"
                    );
                    logged_first_bmp = true;
                }
                last_emit = Some(Instant::now());
            }
        }
    }

    info!("Zenoh source loop exited");
    Ok(())
}
