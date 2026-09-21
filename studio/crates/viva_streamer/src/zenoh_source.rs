//! Zenoh subscription loop that converts frames into BMP bytes.
//!
//! Two subscribers run concurrently inside a single `select!` loop:
//!
//! - **meta subscriber** — listens on `image/meta`; updates the shared `Arc<RwLock<ImageMeta>>`
//!   and rebuilds both BMP encoders when dimensions change. Retained for the Tauri app's
//!   `image-meta-changed` event path.
//! - **frame subscriber** — receives framed payloads (`16-byte FrameHeader + raw pixels`),
//!   decodes the inline header, converts the pixel data to BMP using the appropriate
//!   format-specific encoder, and pushes into the watch channel.
//!
//! ## Supported pixel formats
//!
//! | Format(s)          | Encoding path                                   | BMP output |
//! |--------------------|--------------------------------------------------|------------|
//! | Mono8              | Direct                                           | 8bpp gray  |
//! | Mono10/12/16       | Right-shift to 8-bit gray                        | 8bpp gray  |
//! | BayerRG/GR/BG/GB 8 | Bilinear debayer → RGB8                          | 24bpp RGB  |
//! | BayerRG/GR/BG/GB 10/12/16 | Downscale to 8-bit, then debayer → RGB8  | 24bpp RGB  |
//! | RGB8               | Direct                                           | 24bpp RGB  |
//! | BGR8               | Swap B↔R                                         | 24bpp RGB  |
//! | RGBa8              | Drop alpha                                        | 24bpp RGB  |
//! | Other              | Dropped with a warning                           | —          |

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{RwLock, watch};
use tracing::{info, warn};
use viva_zenoh_api::{FrameHeader, ImageMeta, PixelFormat};

use crate::bmp::{BayerPattern, BmpEncoder, debayer_nn, mono_u16le_to_gray8};
use crate::error::StreamerError;

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

    // Initialise both BMP encoders from the current (default) dimensions.
    let (init_width, init_height) = {
        let m = shared_meta.read().await;
        (m.width, m.height)
    };
    let mut encoder_gray = BmpEncoder::new(init_width, init_height);
    let mut encoder_rgb = BmpEncoder::new_rgb24(init_width, init_height);

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

                // Rebuild encoders only when dimensions change.
                // Safety: the select! loop is single-threaded — no concurrent writers.
                let needs_rebuild = {
                    let old = shared_meta.read().await;
                    old.width != meta.width || old.height != meta.height
                };

                if needs_rebuild {
                    info!(
                        "Dimensions changed to {}x{}; rebuilding BMP encoders",
                        meta.width, meta.height
                    );
                    encoder_gray = BmpEncoder::new(meta.width, meta.height);
                    encoder_rgb = BmpEncoder::new_rgb24(meta.width, meta.height);
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

                // Rebuild encoders when frame-header dimensions differ from cache.
                {
                    let old = shared_meta.read().await;
                    if old.width != header.width || old.height != header.height {
                        info!(
                            "Frame header dimensions changed to {}x{}; rebuilding BMP encoders",
                            header.width, header.height
                        );
                        encoder_gray = BmpEncoder::new(header.width, header.height);
                        encoder_rgb = BmpEncoder::new_rgb24(header.width, header.height);
                    }
                }

                // Encode to BMP based on pixel format.
                let maybe_frame = encode_frame(
                    pixel_data,
                    &header,
                    &encoder_gray,
                    &encoder_rgb,
                );

                let frame = match maybe_frame {
                    Ok(Some(f)) => f,
                    Ok(None) => continue, // unsupported format — warning already logged
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

/// Convert `pixel_data` to a BMP `Bytes` buffer according to `header.pixel_format`.
///
/// Returns `Ok(None)` if the format is not supported (a warning is logged inside).
/// Returns `Err` only on BMP encoding failures (e.g. size mismatch).
fn encode_frame(
    pixel_data: &[u8],
    header: &FrameHeader,
    encoder_gray: &BmpEncoder,
    encoder_rgb: &BmpEncoder,
) -> Result<Option<Bytes>, crate::bmp::BmpError> {
    match &header.pixel_format {
        // ── 8-bit grayscale ───────────────────────────────────────────────
        PixelFormat::Mono8 => Ok(Some(encoder_gray.encode_gray8(pixel_data)?)),

        // ── Downscaled 16-bit grayscale ───────────────────────────────────
        PixelFormat::Mono10 => {
            let gray = mono_u16le_to_gray8(pixel_data, 10);
            Ok(Some(encoder_gray.encode_gray8(&gray)?))
        }
        PixelFormat::Mono12 => {
            let gray = mono_u16le_to_gray8(pixel_data, 12);
            Ok(Some(encoder_gray.encode_gray8(&gray)?))
        }
        PixelFormat::Mono16 => {
            let gray = mono_u16le_to_gray8(pixel_data, 16);
            Ok(Some(encoder_gray.encode_gray8(&gray)?))
        }

        // ── Bayer 8-bit: debayer → 24bpp RGB ─────────────────────────────
        pf @ (PixelFormat::BayerRG8
        | PixelFormat::BayerGR8
        | PixelFormat::BayerBG8
        | PixelFormat::BayerGB8) => {
            let pattern = bayer_pattern(pf);
            let rgb = debayer_nn(pixel_data, header.width, header.height, pattern);
            Ok(Some(encoder_rgb.encode_rgb24(&rgb)?))
        }

        // ── Bayer 10-bit: downscale + debayer → 24bpp RGB ─────────────────
        pf @ (PixelFormat::BayerRG10
        | PixelFormat::BayerGR10
        | PixelFormat::BayerBG10
        | PixelFormat::BayerGB10) => {
            let gray = mono_u16le_to_gray8(pixel_data, 10);
            let pattern = bayer_pattern(pf);
            let rgb = debayer_nn(&gray, header.width, header.height, pattern);
            Ok(Some(encoder_rgb.encode_rgb24(&rgb)?))
        }

        // ── Bayer 12-bit: downscale + debayer → 24bpp RGB ─────────────────
        pf @ (PixelFormat::BayerRG12
        | PixelFormat::BayerGR12
        | PixelFormat::BayerBG12
        | PixelFormat::BayerGB12) => {
            let gray = mono_u16le_to_gray8(pixel_data, 12);
            let pattern = bayer_pattern(pf);
            let rgb = debayer_nn(&gray, header.width, header.height, pattern);
            Ok(Some(encoder_rgb.encode_rgb24(&rgb)?))
        }

        // ── Bayer 16-bit: downscale + debayer → 24bpp RGB ─────────────────
        pf @ (PixelFormat::BayerRG16
        | PixelFormat::BayerGR16
        | PixelFormat::BayerBG16
        | PixelFormat::BayerGB16) => {
            let gray = mono_u16le_to_gray8(pixel_data, 16);
            let pattern = bayer_pattern(pf);
            let rgb = debayer_nn(&gray, header.width, header.height, pattern);
            Ok(Some(encoder_rgb.encode_rgb24(&rgb)?))
        }

        // ── Packed colour ─────────────────────────────────────────────────
        PixelFormat::RGB8 => Ok(Some(encoder_rgb.encode_rgb24(pixel_data)?)),

        PixelFormat::BGR8 => {
            // Swap B↔R channels.
            let rgb: Vec<u8> = pixel_data
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|c| [c[2], c[1], c[0]])
                .collect();
            Ok(Some(encoder_rgb.encode_rgb24(&rgb)?))
        }

        PixelFormat::RGBa8 => {
            // Strip alpha (4th byte).
            let rgb: Vec<u8> = pixel_data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect();
            Ok(Some(encoder_rgb.encode_rgb24(&rgb)?))
        }

        // ── Unsupported ───────────────────────────────────────────────────
        pf => {
            warn!(
                "Unsupported pixel format {:?} for frame seq={}; dropping",
                pf, header.seq
            );
            Ok(None)
        }
    }
}

/// Map a Bayer `PixelFormat` to its `BayerPattern`.
///
/// Callers are responsible for only passing Bayer variants.
fn bayer_pattern(pf: &PixelFormat) -> BayerPattern {
    match pf {
        PixelFormat::BayerRG8
        | PixelFormat::BayerRG10
        | PixelFormat::BayerRG12
        | PixelFormat::BayerRG16 => BayerPattern::Rggb,
        PixelFormat::BayerGR8
        | PixelFormat::BayerGR10
        | PixelFormat::BayerGR12
        | PixelFormat::BayerGR16 => BayerPattern::Grbg,
        PixelFormat::BayerBG8
        | PixelFormat::BayerBG10
        | PixelFormat::BayerBG12
        | PixelFormat::BayerBG16 => BayerPattern::Bggr,
        PixelFormat::BayerGB8
        | PixelFormat::BayerGB10
        | PixelFormat::BayerGB12
        | PixelFormat::BayerGB16 => BayerPattern::Gbrg,
        _ => BayerPattern::Rggb, // unreachable for valid callers
    }
}
