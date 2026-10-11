//! Zenoh subscription loop that converts frames into BMP bytes.
//!
//! Two subscribers run concurrently inside a single `select!` loop:
//!
//! - **meta subscriber** — listens on `image/meta`, updates the shared
//!   `Arc<RwLock<ImageMeta>>` and the stream info the viewer reads.
//! - **frame subscriber** — receives framed payloads (`16-byte FrameHeader + raw
//!   pixels`), decodes the inline header, and draws the pixels with
//!   [`FrameEncoder`] — the same encoder embedded mode uses — in the PFNC
//!   format the wire's names ([`PixelFormat::to_pfnc`]), as often as
//!   [`FramePacer`] allows.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{RwLock, watch};
use tracing::{info, warn};
use viva_zenoh_api::{FrameHeader, ImageMeta, PixelFormat};

use crate::error::StreamerError;
use crate::frame::{FrameEncoder, FramePacer};

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
    let mut pacer = FramePacer::new(source.fps_limit.map_or(Duration::ZERO, |fps| {
        Duration::from_secs_f64(1.0 / fps as f64)
    }));

    let frame_sub = session.declare_subscriber(&source.key_expr).await?;
    let meta_sub = session.declare_subscriber(&source.meta_key).await?;
    let mut encoder = FrameEncoder::new();
    // Warned about once, not on every frame.
    let mut unsupported: Option<PixelFormat> = None;

    info!(
        "Subscribed to '{}' (meta: '{}')",
        source.key_expr, source.meta_key
    );

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

                if !pacer.is_due(frame_tx.receiver_count() > 0, Instant::now()) {
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

                // The converter checks the payload against the geometry and
                // ignores trailing bytes (e.g. GVSP padding).
                let Some(format) = header
                    .pixel_format
                    .to_pfnc()
                    .filter(|format| FrameEncoder::can_draw(*format))
                else {
                    if unsupported.as_ref() != Some(&header.pixel_format) {
                        warn!(
                            "Pixel format {:?} cannot be drawn; dropping its frames",
                            header.pixel_format
                        );
                        unsupported = Some(header.pixel_format.clone());
                    }
                    continue;
                };
                let frame = match encoder.encode(format, header.width, header.height, None, pixel_data) {
                    Ok(frame) => frame,
                    Err(err) => {
                        warn!("Dropped frame seq={}: {err}", header.seq);
                        continue;
                    }
                };

                let bmp_len = frame.len();
                // Stored even with no viewer: the next one to connect is sent it.
                frame_tx.send_replace(frame);
                pacer.sent(Instant::now());
                if !logged_first_bmp {
                    info!(
                        seq = header.seq,
                        bytes = bmp_len,
                        "First BMP frame published to WebSocket broadcaster"
                    );
                    logged_first_bmp = true;
                }
            }
        }
    }

    info!("Zenoh source loop exited");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame that arrives while no viewer is connected is kept for the one
    /// that connects next. In SingleFrame mode it is the only frame, and the
    /// viewer connects after the acquisition starts (Codex review of #173).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_frame_nobody_watches_is_kept_for_the_next_viewer() {
        let session = Arc::new(zenoh::open(zenoh::Config::default()).await.unwrap());
        let key = format!("viva-streamer-test/{}/image", std::process::id());
        let source = ZenohSourceConfig {
            key_expr: key.clone(),
            meta_key: format!("{key}/meta"),
            fps_limit: None,
        };
        let meta = ImageMeta::new(PixelFormat::Mono8, 2, 2, 4);
        let (frame_tx, _) = watch::channel(Bytes::new());
        let (info_tx, _) = watch::channel(crate::ws::StreamInfo::from_image_meta(&meta));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let source_task = tokio::spawn(run_with_session(
            session.clone(),
            source,
            Arc::new(RwLock::new(meta)),
            frame_tx.clone(),
            info_tx,
            shutdown_rx,
        ));

        let header = FrameHeader {
            pixel_format: PixelFormat::Mono8,
            width: 2,
            height: 2,
            seq: 1,
        };
        let mut payload = header.encode();
        payload.extend_from_slice(&[0, 64, 128, 255]);

        // Publish until the source has subscribed and stored a frame.
        let deadline = Instant::now() + Duration::from_secs(5);
        while frame_tx.borrow().is_empty() && Instant::now() < deadline {
            session.put(&key, payload.clone()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let stored = frame_tx.borrow().clone();
        shutdown_tx.send(true).unwrap();
        source_task.await.unwrap().unwrap();

        assert!(
            stored.starts_with(b"BM"),
            "no frame was kept for a later viewer"
        );
    }
}
